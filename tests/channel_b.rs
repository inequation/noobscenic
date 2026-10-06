//! End-to-end check of the phase-3 gateway: a socket that speaks the robot's
//! framing gets the handshake ack and the mandatory pong, the registry tracks the
//! device for as long as the connection lives, and an unhandled or unparsable
//! frame is persisted and survived rather than fatal.

use std::future::pending;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use noobscenic::AppState;
use noobscenic::channel_b::codec::{self, Decoder};
use noobscenic::config::{Config, Logging, Wire};

const SN: &str = "LSLDSM7PROTEST01";

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("noobscenic-b-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

async fn gateway(tag: &str) -> (SocketAddr, AppState, JoinHandle<()>) {
    let config = Config {
        data_dir: scratch(tag),
        logging: Logging {
            file_enabled: false,
            wire: Wire {
                enabled: false,
                ..Wire::default()
            },
            ..Logging::default()
        },
        ..Config::default()
    };

    let state = noobscenic::start(config).await.expect("state");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");

    let serving = state.clone();
    let server = tokio::spawn(async move {
        noobscenic::channel_b::serve(listener, serving, pending::<()>())
            .await
            .expect("serve");
    });
    (addr, state, server)
}

async fn read_frames(sock: &mut TcpStream, decoder: &mut Decoder, wanted: usize) -> Vec<Vec<u8>> {
    let mut frames = Vec::new();
    let mut buffer = vec![0u8; 4096];
    while frames.len() < wanted {
        let n = timeout(Duration::from_secs(5), sock.read(&mut buffer))
            .await
            .unwrap_or_else(|_| panic!("only {} of {wanted} frames arrived", frames.len()))
            .expect("socket read");
        assert!(
            n > 0,
            "peer closed with {} of {wanted} frames",
            frames.len()
        );
        frames.extend(decoder.push(&buffer[..n]).expect("valid framing"));
    }
    frames
}

fn handshake() -> Vec<u8> {
    codec::encode_json(&json!({
        "infoType": 10001,
        "connectionType": 1,
        "data": {"token": "", "sn": SN},
    }))
}

async fn cleanup(state: AppState, server: JoinHandle<()>) {
    server.abort();
    state.db.close().await;
    let _ = std::fs::remove_dir_all(state.config.data_dir.clone());
}

#[tokio::test]
async fn handshake_is_acked_and_every_ping_is_ponged() {
    let (addr, state, server) = gateway("pair").await;
    let mut sock = TcpStream::connect(addr).await.expect("connect");
    let mut decoder = Decoder::new(1 << 20);

    let mut out = handshake();
    out.extend_from_slice(&codec::encode_json(&json!({"infoType": 21006, "data": {}})));
    sock.write_all(&out).await.expect("write");

    let frames = read_frames(&mut sock, &mut decoder, 2).await;
    let ack: Value = serde_json::from_slice(&frames[0]).expect("ack is JSON");
    assert_eq!(ack["infoType"], 10001);

    let pong: Value = serde_json::from_slice(&frames[1]).expect("pong is JSON");
    assert_eq!(pong["infoType"], 21006);
    // No integer `encrypt` and the device's inbound gate drops the frame; without
    // `isExistConnect:true` the robot never pushes status or maps.
    assert_eq!(pong["encrypt"], 0);
    assert_eq!(pong["data"]["isExistConnect"], true);

    assert!(
        state.registry.is_online(SN),
        "the handshake must put the device online"
    );
    assert!(
        state
            .registry
            .get(SN)
            .unwrap()
            .peer
            .starts_with("127.0.0.1")
    );

    drop(sock);
    let unregistered = timeout(Duration::from_secs(5), async {
        while state.registry.is_online(SN) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(
        unregistered.is_ok(),
        "the registry entry must go away with the socket"
    );

    cleanup(state, server).await;
}

#[tokio::test]
async fn unhandled_frames_are_persisted_and_the_link_survives() {
    let (addr, state, server) = gateway("unhandled").await;
    let mut sock = TcpStream::connect(addr).await.expect("connect");
    let mut decoder = Decoder::new(1 << 20);

    sock.write_all(&handshake()).await.expect("write handshake");
    let frames = read_frames(&mut sock, &mut decoder, 1).await;
    assert_eq!(
        serde_json::from_slice::<Value>(&frames[0]).unwrap()["infoType"],
        10001
    );

    let mut out = codec::encode_json(&json!({"infoType": 31337, "data": {"x": 1}}));
    out.extend_from_slice(&codec::encode(b"not json"));
    // If those two frames were fatal, this ping would never be answered.
    out.extend_from_slice(&codec::encode_json(&json!({"infoType": 21006, "data": {}})));
    sock.write_all(&out).await.expect("write");

    let frames = read_frames(&mut sock, &mut decoder, 1).await;
    let pong: Value = serde_json::from_slice(&frames[0]).expect("pong is JSON");
    assert_eq!(
        pong["infoType"], 21006,
        "an unhandled frame must not close the connection"
    );

    let rows: Vec<(Option<i64>, String)> =
        sqlx::query_as("SELECT info_type, payload FROM events WHERE channel = 'B' ORDER BY id")
            .fetch_all(&state.db)
            .await
            .expect("events rows");
    assert_eq!(rows.len(), 2, "both odd frames must be persisted: {rows:?}");
    assert_eq!(rows[0].0, Some(31337));
    assert!(rows[0].1.contains("\"x\":1"), "{}", rows[0].1);
    assert_eq!(rows[1].0, None);
    assert_eq!(rows[1].1, "not json");

    cleanup(state, server).await;
}
