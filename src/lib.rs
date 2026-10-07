//! noobscenic — a clean-room replacement cloud for the Proscenic M7 Pro.
//!
//! See `doc/PLAN.md` for the design and `doc/reverse-engineering/` for the protocol
//! the design is derived from.

pub mod channel_a;
pub mod channel_b;
pub mod config;
pub mod console;
pub mod db;
pub mod error;
pub mod map;
pub mod path;
pub mod proto;
pub mod session;
pub mod wire;

use std::sync::Arc;

use sqlx::SqlitePool;

use crate::config::Config;
use crate::error::Result;
use crate::wire::Tap;

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub tap: Arc<Tap>,
    pub config: Arc<Config>,
    /// Who is connected on channel B right now (doc/PLAN.md §10.2).
    pub registry: Arc<channel_b::Registry>,
    /// Write channel-B frames in the device's own jsoncpp-ish style instead of
    /// compact JSON (console toggle `style`, for protocol experiments).
    pub frame_style: Arc<std::sync::atomic::AtomicBool>,
    /// Whether `21006` pings are ponged (console toggle `pongs`). Turning this off
    /// silences the server without closing the socket, which is the only way to
    /// measure the device's own liveness timeout.
    pub pongs: Arc<std::sync::atomic::AtomicBool>,
}

/// Open everything the server needs. Fails loudly here, so that once we are serving,
/// failures are per-request and survivable.
pub async fn start(config: Config) -> Result<AppState> {
    std::fs::create_dir_all(&config.data_dir)?;

    let url = config.database_url();
    let db = db::connect(&url, config.database.max_connections).await?;
    tracing::info!(database = %url, "schema is up to date");

    let tap = Tap::new(&config.logging.wire, config.wire_dir())?;
    if tap.is_enabled() {
        tracing::info!(dir = %config.wire_dir().display(), format = ?config.logging.wire.format,
            "wire tracing on");
    } else {
        tracing::warn!("wire tracing is off; traces are the point of this phase");
    }

    Ok(AppState {
        db,
        tap: Arc::new(tap),
        config: Arc::new(config),
        registry: Arc::new(channel_b::Registry::new()),
        frame_style: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        pongs: Arc::new(std::sync::atomic::AtomicBool::new(true)),
    })
}

/// Run until Ctrl-C.
pub async fn run(config: Config) -> Result<()> {
    let http_bind = config.http.bind;
    let gateway_bind = config.gateway.bind;
    let state = start(config).await?;

    // Bind before serving: a port clash must be a startup failure, not a server
    // that answers on one channel and silently not the other.
    let http = tokio::net::TcpListener::bind(http_bind).await?;
    let gateway = tokio::net::TcpListener::bind(gateway_bind).await?;

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(());
    console::spawn(state.clone(), shutdown_tx.clone());
    let signal = tokio::spawn(async move {
        shutdown_signal().await;
        let _ = shutdown_tx.send(());
    });
    let wait = |mut rx: tokio::sync::watch::Receiver<()>| async move {
        let _ = rx.changed().await;
    };

    let result = tokio::try_join!(
        channel_a::serve(http, state.clone(), wait(shutdown_rx.clone())),
        channel_b::serve(gateway, state.clone(), wait(shutdown_rx)),
    );
    signal.abort();

    // Give the pool a chance to checkpoint the WAL rather than leaving it to recovery.
    state.db.close().await;
    tracing::info!("stopped");
    result.map(|_| ())
}

async fn shutdown_signal() {
    match tokio::signal::ctrl_c().await {
        Ok(()) => tracing::info!("interrupt received, shutting down"),
        Err(error) => {
            // Never resolve on error: returning here would shut the server down
            // immediately, which is the opposite of what a failed handler should do.
            tracing::error!(%error, "could not listen for Ctrl-C; run until killed");
            std::future::pending::<()>().await
        }
    }
}
