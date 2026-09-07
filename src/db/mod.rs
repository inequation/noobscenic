//! SQLite persistence — see doc/PLAN.md §7.

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use sqlx::migrate::Migrator;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::SqlitePool;

use crate::error::Result;

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// Open the pool, create the file and its directory if needed, and migrate.
pub async fn connect(url: &str, max_connections: u32) -> Result<SqlitePool> {
    if let Some(parent) = database_parent_dir(url) {
        std::fs::create_dir_all(&parent)?;
    }

    let options = SqliteConnectOptions::from_str(url)?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));

    let pool = SqlitePoolOptions::new()
        .max_connections(max_connections)
        .connect_with(options)
        .await?;

    MIGRATOR.run(&pool).await?;
    Ok(pool)
}

/// The directory a `sqlite:` URL points into, if it points at a file at all.
///
/// Parsed by hand rather than via `SqliteConnectOptions`, because `create_if_missing`
/// creates the database file but not the directory holding it.
fn database_parent_dir(url: &str) -> Option<PathBuf> {
    let rest = url
        .strip_prefix("sqlite://")
        .or_else(|| url.strip_prefix("sqlite:"))
        .unwrap_or(url);
    let file = rest.split(['?', '#']).next().unwrap_or(rest);
    if file.is_empty() || file == ":memory:" {
        return None;
    }
    Path::new(file).parent().filter(|p| !p.as_os_str().is_empty()).map(Path::to_path_buf)
}

/// Milliseconds since the Unix epoch, on the server's clock.
pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_database_directories() {
        assert_eq!(database_parent_dir("sqlite://./var/nb.db"), Some(PathBuf::from("./var")));
        assert_eq!(database_parent_dir("sqlite:/srv/x/nb.db"), Some(PathBuf::from("/srv/x")));
        assert_eq!(database_parent_dir("sqlite://nb.db"), None);
        assert_eq!(database_parent_dir("sqlite::memory:"), None);
        assert_eq!(
            database_parent_dir("sqlite://./var/nb.db?mode=rwc"),
            Some(PathBuf::from("./var"))
        );
    }

    #[tokio::test]
    async fn migrations_apply_and_the_schema_is_usable() {
        let dir = std::env::temp_dir().join(format!("noobscenic-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("migrate.db");
        let _ = std::fs::remove_file(&path);

        let pool = connect(&format!("sqlite://{}", path.display()), 1).await.unwrap();
        sqlx::query("INSERT INTO devices (sn, bind_state, first_seen_ms, last_seen_ms) VALUES (?, 'unbound', ?, ?)")
            .bind("TESTSN")
            .bind(now_ms())
            .bind(now_ms())
            .execute(&pool)
            .await
            .unwrap();

        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM devices")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);

        pool.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }
}
