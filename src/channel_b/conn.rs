//! One device connection: a read task and a write task joined by a queue
//! (doc/PLAN.md §10.2), plus the phase-3 message handling.
//!
//! Everything the robot sends is tapped and every failure is logged per
//! connection: a broken socket, an unparsable frame or a database hiccup closes
//! at most this one connection, never the listener.

use std::net::SocketAddr;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::AppState;
use crate::db::now_ms;
use crate::error::Result;
use crate::wire::{Direction, Record, Recorded};

use super::codec::{self, Decoder};
use super::registry::{self, Registration};

/// Outbound frames allowed to queue before we apply backpressure to the device's
/// reader. Frames are tiny; the queue only exists so a pong is never stuck behind
/// a slow write.
const WRITE_QUEUE: usize = 32;
const READ_CHUNK: usize = 16 * 1024;

/// The handshake type: the device announces its serial, and we bind it to this
/// connection (PROTOCOL.md §B).
const HANDSHAKE: i64 = 10001;
/// The keepalive ping; every one must be ponged or the robot reconnects.
const PING: i64 = 21006;

pub(crate) async fn handle(state: AppState, stream: TcpStream, conn_id: String, peer: SocketAddr) {
    let _ = stream.set_nodelay(true);
    let peer = peer.to_string();
    tracing::info!(conn = %conn_id, %peer, "channel-B connection opened");

    let (read_half, write_half) = stream.into_split();
    let (tx, rx) = mpsc::channel(WRITE_QUEUE);
    let writer = tokio::spawn(write_task(
        state.clone(),
        write_half,
        rx,
        conn_id.clone(),
        peer.clone(),
    ));

    if let Err(error) = read_task(state, read_half, tx, conn_id.clone(), peer.clone()).await {
        tracing::warn!(conn = %conn_id, %peer, %error, "channel-B read failed");
    }

    // The reader just returned, so no new frames can be queued; the writer drains
    // what is left and exits.
    match writer.await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::warn!(conn = %conn_id, %peer, %error, "channel-B write failed"),
        Err(join_error) => {
            tracing::warn!(conn = %conn_id, %peer, %join_error, "channel-B write task died");
        }
    }
    tracing::info!(conn = %conn_id, %peer, "channel-B connection closed");
}

/// Read frames until the socket closes, the watchdog trips or a framing error is
/// fatal to this connection.
async fn read_task(
    state: AppState,
    mut reader: OwnedReadHalf,
    tx: mpsc::Sender<Value>,
    conn_id: String,
    peer: String,
) -> Result<()> {
    let ping_timeout = Duration::from_secs(state.config.gateway.ping_timeout_secs);
    let mut decoder = Decoder::new(state.config.gateway.max_frame_bytes);
    let mut conn = Conn {
        state,
        conn_id,
        peer,
        tx,
        sn: None,
        registration: None,
        last_ping: Instant::now(),
    };
    let mut buffer = vec![0u8; READ_CHUNK];

    loop {
        let read = reader.read(&mut buffer);
        tokio::pin!(read);
        // The watchdog measures "no ping", not "no bytes": map/status traffic must
        // not be able to keep a silent-but-alive-looking link up forever.
        let read = if ping_timeout.is_zero() {
            read.await
        } else {
            let deadline = conn.last_ping + ping_timeout;
            tokio::select! {
                result = &mut read => result,
                _ = tokio::time::sleep_until(deadline) => {
                    tracing::warn!(
                        conn = %conn.conn_id,
                        peer = %conn.peer,
                        timeout_secs = ping_timeout.as_secs(),
                        "no channel-B ping within the watchdog window; closing"
                    );
                    return Ok(());
                }
            }
        };
        let n = match read {
            Ok(0) => return Ok(()),
            Ok(n) => n,
            Err(error) => return Err(error.into()),
        };

        let frames = match decoder.push(&buffer[..n]) {
            Ok(frames) => frames,
            Err(error) => {
                tracing::warn!(conn = %conn.conn_id, %error, "channel-B framing error; closing");
                return Ok(());
            }
        };
        for frame in frames {
            if frame.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            conn.handle_frame(&frame).await;
        }
    }
}

/// Write frames as they are queued, so concurrent producers can never interleave
/// mid-frame.
async fn write_task(
    state: AppState,
    mut writer: OwnedWriteHalf,
    mut rx: mpsc::Receiver<Value>,
    conn_id: String,
    peer: String,
) -> Result<()> {
    while let Some(value) = rx.recv().await {
        let info_type = value.get("infoType").and_then(Value::as_i64);
        let bytes = codec::encode_json(&value);

        let mut record = Record::new("B", Direction::Out, "frame", &bytes)
            .conn_id(conn_id.clone())
            .peer(peer.clone());
        if let Some(info_type) = info_type {
            record = record.meta("info_type", info_type);
        }
        state.tap.record(record);

        writer.write_all(&bytes).await?;
        writer.flush().await?;
    }
    Ok(())
}

/// Per-connection state that frame handling needs: who we are, who the device is
/// (once it says so) and the registry entry that keeps it addressable.
struct Conn {
    state: AppState,
    conn_id: String,
    peer: String,
    tx: mpsc::Sender<Value>,
    sn: Option<String>,
    registration: Option<Registration>,
    last_ping: Instant,
}

impl Conn {
    async fn handle_frame(&mut self, frame: &[u8]) {
        let parsed: Option<Value> = serde_json::from_slice(frame).ok();
        let info_type = parsed
            .as_ref()
            .and_then(|value| value.get("infoType"))
            .and_then(Value::as_i64);

        // The tap records frames the way they crossed the socket, delimiter
        // included, in both directions (doc/PLAN.md §8).
        let mut trace_body = Vec::with_capacity(frame.len() + codec::DELIMITER.len());
        trace_body.extend_from_slice(frame);
        trace_body.extend_from_slice(codec::DELIMITER);
        let mut record = Record::new("B", Direction::In, "frame", &trace_body)
            .conn_id(self.conn_id.clone())
            .peer(self.peer.clone());
        if let Some(sn) = &self.sn {
            record = record.sn(sn.clone());
        }
        if let Some(info_type) = info_type {
            record = record.meta("info_type", info_type);
        } else {
            record = record.meta("content", "no infoType");
        }
        let recorded = self.state.tap.record(record);

        let Some(value) = parsed else {
            let text = String::from_utf8_lossy(frame).into_owned();
            tracing::warn!(
                conn = %self.conn_id,
                bytes = frame.len(),
                "channel-B frame is not JSON; persisted"
            );
            self.persist(None, &text, recorded.as_ref()).await;
            return;
        };

        match info_type {
            Some(HANDSHAKE) => self.handshake(&value).await,
            Some(PING) => self.pong().await,
            Some(other) => {
                tracing::warn!(
                    conn = %self.conn_id,
                    info_type = other,
                    "unhandled channel-B infoType; persisted"
                );
                self.persist(Some(other), &value.to_string(), recorded.as_ref())
                    .await;
            }
            None => {
                tracing::warn!(
                    conn = %self.conn_id,
                    "channel-B frame has no integer infoType; persisted"
                );
                self.persist(None, &value.to_string(), recorded.as_ref())
                    .await;
            }
        }
    }

    /// `{"infoType":10001,"connectionType":1,"data":{"token":"","sn":"…"}}`.
    async fn handshake(&mut self, value: &Value) {
        let sn = value
            .get("data")
            .and_then(|data| data.get("sn"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        if sn.is_empty() {
            tracing::warn!(
                conn = %self.conn_id,
                peer = %self.peer,
                "channel-B handshake without data.sn; left unregistered"
            );
        } else {
            self.bind(&sn);
        }

        if self.state.config.gateway.ack_handshake {
            self.send(json!({"infoType": HANDSHAKE, "message": "ok", "data": {}}))
                .await;
        }
    }

    /// Make this connection the one the server addresses `sn` on.
    fn bind(&mut self, sn: &str) {
        if self.sn.as_deref() == Some(sn) {
            return;
        }
        let previous = self.state.registry.register(
            sn,
            registry::handle(&self.conn_id, &self.peer, self.tx.clone()),
        );
        if let Some(previous) = previous {
            tracing::warn!(
                sn = %sn,
                old = %previous.conn_id,
                new = %self.conn_id,
                "replacing an older channel-B connection for this device"
            );
        }

        // Replacing the guard (a second handshake with a different SN) drops the
        // previous binding first.
        self.registration = Some(Registration::new(
            self.state.registry.clone(),
            sn.to_string(),
            self.conn_id.clone(),
        ));
        self.sn = Some(sn.to_string());
        tracing::info!(
            conn = %self.conn_id,
            sn = %sn,
            peer = %self.peer,
            "channel-B handshake; device online"
        );
    }

    /// Answer a `21006` ping. The integer `encrypt` is what gets the frame past the
    /// device's inbound gate, and `isExistConnect:true` is what tells the robot an
    /// app/cloud is online, enabling its status and map pushes (FUNC_STATUS.md §2.3).
    async fn pong(&mut self) {
        self.last_ping = Instant::now();
        let data = if self.state.config.gateway.announce_app_online {
            json!({"isExistConnect": true})
        } else {
            json!({})
        };
        self.send(json!({"infoType": PING, "encrypt": 0, "data": data}))
            .await;
    }

    async fn send(&self, value: Value) {
        if self.tx.send(value).await.is_err() {
            tracing::warn!(conn = %self.conn_id, "channel-B writer is gone; frame dropped");
        }
    }

    /// Record a frame we do not (yet) handle. A database failure is logged and
    /// swallowed: it must never cost the robot its connection.
    async fn persist(&self, info_type: Option<i64>, payload: &str, trace_ref: Option<&Recorded>) {
        let result = sqlx::query(
            "INSERT INTO events (sn, channel, direction, endpoint, info_type, payload, trace_ref, received_ms)
             VALUES (?, 'B', 'in', NULL, ?, ?, ?, ?)",
        )
        .bind(self.sn.as_deref())
        .bind(info_type)
        .bind(payload)
        .bind(trace_ref.map(|recorded| recorded.reference.as_str()))
        .bind(now_ms())
        .execute(&self.state.db)
        .await;

        if let Err(error) = result {
            tracing::error!(conn = %self.conn_id, %error, "could not persist the channel-B frame");
        }
    }
}
