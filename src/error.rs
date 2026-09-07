//! One error type for the whole server.
//!
//! Nothing in here is allowed to panic a connection task: handlers turn errors into
//! logged warnings and benign responses, because a device that gets an error where it
//! expected an answer falls into a re-register or reconnect loop (doc/PLAN.md §17).

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("database: {0}")]
    Db(#[from] sqlx::Error),

    #[error("migration: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    #[error("config file {path}: {source}")]
    ConfigRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("config file {path}: {source}")]
    ConfigParse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("invalid configuration: {0}")]
    Config(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    pub fn config(message: impl Into<String>) -> Self {
        Error::Config(message.into())
    }
}
