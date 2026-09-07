//! noobscenic — a clean-room replacement cloud for the Proscenic M7 Pro.
//!
//! See `doc/PLAN.md` for the design and `doc/reverse-engineering/` for the protocol
//! the design is derived from.

pub mod channel_a;
pub mod config;
pub mod db;
pub mod error;
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

    Ok(AppState { db, tap: Arc::new(tap), config: Arc::new(config) })
}

/// Run until Ctrl-C.
pub async fn run(config: Config) -> Result<()> {
    let http_bind = config.http.bind;
    let state = start(config).await?;

    let result = channel_a::serve(state.clone(), http_bind, shutdown_signal()).await;

    // Give the pool a chance to checkpoint the WAL rather than leaving it to recovery.
    state.db.close().await;
    tracing::info!("stopped");
    result
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
