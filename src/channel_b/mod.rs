//! Channel B — the push gateway (doc/PLAN.md §10).
//!
//! The robot's realtime link: raw TCP to the `ip:port` in its
//! `/data/bin/Run/Config/ip_port.json`, `<JSON>#\t#` frames in both directions.
//! This listener must be up *together with* channel A: after `setID` arms the
//! bind, the robot dials here, and pairing only ends once this side answers the
//! `10001` handshake and every `21006` ping while channel A answers the `binding`
//! preBind with `code:0` (PAIRING_LOG_ANALYSIS.md §4, §8).
//!
//! Phase 3 implements the minimum for exactly that: the frame codec, a listener
//! with per-connection read/write tasks, the connection registry, the handshake,
//! the mandatory pong and a ping watchdog. Anything else the robot sends is
//! persisted to `events` and warned about — permissive by default, never fatal
//! (doc/PLAN.md §2). Decoding that traffic is phase 4's job.

pub mod codec;
mod conn;
pub mod registry;

pub use registry::{ConnHandle, Registry};

use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::net::TcpListener;

use crate::error::Result;
use crate::AppState;

/// Process-wide connection numbering; `b-000001`, `b-000002`, … in the traces.
static NEXT_CONN_ID: AtomicU64 = AtomicU64::new(1);

/// Serve the gateway until `shutdown` resolves.
pub async fn serve(
    listener: TcpListener,
    state: AppState,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    tracing::info!(addr = %listener.local_addr()?, "channel B (gateway) listening");
    tokio::pin!(shutdown);

    let mut connections = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => {
                    let conn_id = next_conn_id();
                    let state = state.clone();
                    connections.spawn(conn::handle(state, stream, conn_id, peer));
                }
                Err(error) => {
                    // A failed accept must not spin the loop into the ground, and
                    // must not take the listener with it.
                    tracing::warn!(%error, "channel-B accept failed");
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            },
            finished = connections.join_next(), if !connections.is_empty() => {
                if let Some(Err(error)) = finished {
                    tracing::warn!(%error, "channel-B connection task died");
                }
            }
        }
    }

    // Dropping each connection task closes its write queue; the robot will redial
    // once we are back, which is exactly the behaviour its heartbeat expects.
    connections.abort_all();
    Ok(())
}

fn next_conn_id() -> String {
    format!("b-{:06}", NEXT_CONN_ID.fetch_add(1, Ordering::Relaxed))
}
