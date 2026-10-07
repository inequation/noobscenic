//! Phase-5 control over channel B: the `commands` table is the queue, the poller
//! pushes `encrypt:0` frames in the documented envelope, stale rows expire instead
//! of firing, and a `cleanPack/response` ACKs the newest matching command.

use std::future::pending;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tower::ServiceExt;

use noobscenic::AppState;
use noobscenic::channel_b::codec::{self, Decoder};
use noobscenic::config::{Config, Logging, Wire};

const SN: &str = "LSLDSM7PROTEST02";

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("noobscenic-ctl-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A live gateway, with the wire tap **on**: the queue's `trace_ref` contract is
/// half of what these tests check.
async fn gateway(tag: &str) -> (SocketAddr, AppState, JoinHandle<()>) {
    let config = Config {
        data_dir: scratch(tag),
        logging: Logging {
            file_enabled: false,
            wire: Wire {
                enabled: true,
                dir: Some(scratch(tag).join("traces")),
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

async fn cleanup(state: AppState) {
    let dir = state.config.data_dir.clone();
    state.db.close().await;
    let _ = std::fs::remove_dir_all(dir);
}

fn handshake() -> Vec<u8> {
    codec::encode_json(&json!({
        "infoType": 10001,
        "connectionType": 1,
        "data": {"token": "", "sn": SN},
    }))
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

/// Handshake and consume the ack, so the registry is bound when we return.
async fn connect(addr: SocketAddr) -> (TcpStream, Decoder) {
    let mut sock = TcpStream::connect(addr).await.expect("connect");
    sock.write_all(&handshake()).await.expect("handshake");
    let mut decoder = Decoder::new(1 << 20);
    let frames = read_frames(&mut sock, &mut decoder, 1).await;
    let ack: Value = serde_json::from_slice(&frames[0]).expect("ack is JSON");
    assert_eq!(ack["encrypt"].as_i64(), Some(0));
    (sock, decoder)
}

fn post(path: &str, body: &str) -> Request<Body> {
    let mut request = Request::builder()
        .method("POST")
        .uri(path)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body.to_string()))
        .expect("request");
    request
        .extensions_mut()
        .insert(ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            45678,
        ))));
    request
}

async fn call(app: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (status, serde_json::from_slice(&bytes).expect("JSON body"))
}

async fn enqueue(state: &AppState, info_type: i64, payload: &str) -> i64 {
    noobscenic::db::queries::insert_command(&state.db, SN, info_type, payload, 0)
        .await
        .expect("enqueue")
}

async fn command_row(
    state: &AppState,
    id: i64,
) -> (String, Option<i64>, Option<String>, Option<String>) {
    sqlx::query_as("SELECT state, sent_ms, trace_ref, ack_payload FROM commands WHERE id = ?")
        .bind(id)
        .fetch_one(&state.db)
        .await
        .expect("command row")
}

#[tokio::test]
async fn a_queued_command_reaches_the_device_in_the_documented_envelope() {
    let (addr, state, server) = gateway("push").await;
    let (mut sock, mut decoder) = connect(addr).await;

    let id = enqueue(&state, 21012, "{\"cmd\":\"start\"}").await;
    noobscenic::commands::drain_once(&state).await;

    let frames = read_frames(&mut sock, &mut decoder, 1).await;
    let frame: Value = serde_json::from_slice(&frames[0]).expect("frame is JSON");
    // The device's inbound gate drops the frame unless `encrypt` is an integer
    // (doc/PLAN.md §10.3).
    assert!(
        frame["encrypt"].is_i64(),
        "encrypt must be an integer, got {frame}"
    );
    assert_eq!(frame["encrypt"].as_i64(), Some(0));
    let inner = &frame["data"];
    assert_eq!(inner["infoType"], 21012);
    assert_eq!(inner["data"], json!({"cmd": "start"}));
    assert!(
        inner["dInfo"]["ts"].is_string(),
        "dInfo.ts must be a string"
    );
    assert!(
        inner["dInfo"]["userId"].is_string(),
        "dInfo.userId must be a string"
    );

    let (row_state, sent_ms, trace_ref, _) = command_row(&state, id).await;
    assert_eq!(row_state, "sent");
    assert!(sent_ms.is_some(), "sent_ms must be stamped");
    let trace_ref = trace_ref.expect("the writer stored the frame's trace reference");
    let (file, seq) = trace_ref.split_once('#').expect("file#seq");
    assert!(
        state.config.wire_dir().join(file).exists() && !seq.is_empty(),
        "trace_ref {trace_ref} must point at a real file"
    );

    server.abort();
    cleanup(state).await;
}

#[tokio::test]
async fn commands_for_an_offline_device_wait_for_the_connection() {
    let (addr, state, server) = gateway("offline").await;

    let id = enqueue(&state, 21012, "{\"cmd\":\"start\"}").await;
    noobscenic::commands::drain_once(&state).await;

    let (row_state, sent_ms, _, _) = command_row(&state, id).await;
    assert_eq!(
        row_state, "pending",
        "an offline device must not lose its queue"
    );
    assert!(sent_ms.is_none());

    let (mut sock, mut decoder) = connect(addr).await;
    noobscenic::commands::drain_once(&state).await;
    let frames = read_frames(&mut sock, &mut decoder, 1).await;
    let frame: Value = serde_json::from_slice(&frames[0]).expect("frame is JSON");
    assert_eq!(frame["data"]["infoType"], 21012);

    let (row_state, sent_ms, _, _) = command_row(&state, id).await;
    assert_eq!(row_state, "sent");
    assert!(sent_ms.is_some());

    server.abort();
    cleanup(state).await;
}

#[tokio::test]
async fn a_stale_pending_command_expires_instead_of_firing() {
    let (addr, state, server) = gateway("ttl").await;
    let (mut sock, decoder) = connect(addr).await;

    let id = enqueue(&state, 21012, "{\"cmd\":\"start\"}").await;
    sqlx::query("UPDATE commands SET created_ms = ? WHERE id = ?")
        .bind(noobscenic::db::now_ms() - 400_000)
        .bind(id)
        .execute(&state.db)
        .await
        .expect("backdate");

    noobscenic::commands::drain_once(&state).await;

    let (row_state, sent_ms, _, _) = command_row(&state, id).await;
    assert_eq!(row_state, "expired");
    assert!(sent_ms.is_none());

    // And nothing crossed the socket: the robot must not start cleaning hours late.
    let mut buffer = [0u8; 64];
    assert!(
        timeout(Duration::from_millis(300), sock.read(&mut buffer))
            .await
            .is_err(),
        "an expired command was still sent"
    );
    drop(decoder);

    server.abort();
    cleanup(state).await;
}

#[tokio::test]
async fn a_response_acks_the_newest_sent_command_and_a_mismatch_acks_nothing() {
    let (addr, state, server) = gateway("ack").await;
    let (mut sock, mut decoder) = connect(addr).await;

    let id = enqueue(&state, 21011, "{\"startPos\":0,\"mask\":0}").await;
    noobscenic::commands::drain_once(&state).await;
    let _ = read_frames(&mut sock, &mut decoder, 1).await;

    let app = noobscenic::channel_a::router(state.clone());
    let message = "{\"message\":\"ok\",\"infoType\":21011,\"data\":{\"pathID\":7,\"posArray\":[]},\"dInfo\":{\"ts\":\"1\",\"userId\":\"u\"}}";
    let body = format!("sn={SN}&infoType=21011&ts=1&userId=u&data={message}");
    let (status, response) = call(&app, post("/cleanPack/response", &body)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["code"], 0);

    let (row_state, _, _, ack_payload) = command_row(&state, id).await;
    assert_eq!(row_state, "acked");
    assert!(
        ack_payload
            .expect("ack payload")
            .contains("\"message\":\"ok\""),
        "the response body must be kept on the row"
    );

    // A response that matches no sent command must not invent one.
    let spurious =
        format!("sn={SN}&infoType=21020&ts=1&userId=u&data={{\"infoType\":21020,\"data\":{{}}}}");
    let (status, _) = call(&app, post("/cleanPack/response", &spurious)).await;
    assert_eq!(status, StatusCode::OK);
    let acks: i64 = sqlx::query_scalar("SELECT count(1) FROM commands WHERE state = 'acked'")
        .fetch_one(&state.db)
        .await
        .expect("count");
    assert_eq!(acks, 1, "an unmatched response invented an ACK");

    server.abort();
    cleanup(state).await;
}
