//! All SQL in one place, runtime-checked (doc/PLAN.md §7). Phase 2 covers the
//! device upserts and the session lifecycle; later phases add their rows here.

use sqlx::SqlitePool;

use crate::db::now_ms;
use crate::error::Result;

pub async fn device_exists(pool: &SqlitePool, sn: &str) -> Result<bool> {
    let found: Option<i64> = sqlx::query_scalar("SELECT 1 FROM devices WHERE sn = ?")
        .bind(sn)
        .fetch_optional(pool)
        .await?;
    Ok(found.is_some())
}

/// Adopt a device on first sight; never clobber what is already known.
pub async fn upsert_device_seen(pool: &SqlitePool, sn: &str, ld_sn: Option<&str>) -> Result<()> {
    let now = now_ms();
    sqlx::query(
        "INSERT INTO devices (sn, ld_sn, bind_state, first_seen_ms, last_seen_ms)
         VALUES (?, ?, 'unbound', ?, ?)
         ON CONFLICT(sn) DO UPDATE SET
             ld_sn = COALESCE(excluded.ld_sn, devices.ld_sn),
             last_seen_ms = excluded.last_seen_ms",
    )
    .bind(sn)
    .bind(ld_sn)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Record the attributes `sync` reported.
#[allow(clippy::too_many_arguments)]
pub async fn upsert_device_sync(
    pool: &SqlitePool,
    sn: &str,
    company_id: Option<i64>,
    mcu_ver: Option<&str>,
    app_version: Option<&str>,
    version_code: Option<i64>,
    git_sha: Option<&str>,
    cloud: Option<&str>,
) -> Result<()> {
    let now = now_ms();
    sqlx::query(
        "INSERT INTO devices (sn, company_id, mcu_ver, app_version, version_code, git_sha, cloud,
                              bind_state, first_seen_ms, last_seen_ms)
         VALUES (?, ?, ?, ?, ?, ?, ?, 'unbound', ?, ?)
         ON CONFLICT(sn) DO UPDATE SET
             company_id   = COALESCE(excluded.company_id, devices.company_id),
             mcu_ver      = COALESCE(excluded.mcu_ver, devices.mcu_ver),
             app_version  = COALESCE(excluded.app_version, devices.app_version),
             version_code = COALESCE(excluded.version_code, devices.version_code),
             git_sha      = COALESCE(excluded.git_sha, devices.git_sha),
             cloud        = COALESCE(excluded.cloud, devices.cloud),
             last_seen_ms = excluded.last_seen_ms",
    )
    .bind(sn)
    .bind(company_id)
    .bind(mcu_ver)
    .bind(app_version)
    .bind(version_code)
    .bind(git_sha)
    .bind(cloud)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Insert a session; `false` means the cookie collided with a live one, so the
/// caller should mint another.
pub async fn insert_session(
    pool: &SqlitePool,
    sn: &str,
    session_key: &str,
    cookie: &str,
    sig_raw: Option<&str>,
) -> Result<bool> {
    let result = sqlx::query(
        "INSERT INTO sessions (sn, session_key, cookie, sig_raw, created_ms)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(cookie) DO NOTHING",
    )
    .bind(sn)
    .bind(session_key)
    .bind(cookie)
    .bind(sig_raw)
    .bind(now_ms())
    .execute(pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// The live session a cookie names, as `(sn, created_ms)`.
pub async fn find_session(pool: &SqlitePool, cookie: &str) -> Result<Option<(String, i64)>> {
    Ok(sqlx::query_as(
        "SELECT sn, created_ms FROM sessions WHERE cookie = ? AND revoked_ms IS NULL",
    )
    .bind(cookie)
    .fetch_optional(pool)
    .await?)
}

pub async fn touch_session(pool: &SqlitePool, cookie: &str) -> Result<()> {
    sqlx::query("UPDATE sessions SET last_used_ms = ? WHERE cookie = ?")
        .bind(now_ms())
        .bind(cookie)
        .execute(pool)
        .await?;
    Ok(())
}

/// The newest live session key for a device — the key `encrypt:1` commands use.
pub async fn latest_session_key(pool: &SqlitePool, sn: &str) -> Result<Option<String>> {
    Ok(sqlx::query_scalar(
        "SELECT session_key FROM sessions
         WHERE sn = ? AND revoked_ms IS NULL
         ORDER BY created_ms DESC, id DESC LIMIT 1",
    )
    .bind(sn)
    .fetch_optional(pool)
    .await?)
}

pub async fn revoke_session(pool: &SqlitePool, cookie: &str) -> Result<()> {
    sqlx::query("UPDATE sessions SET revoked_ms = ? WHERE cookie = ? AND revoked_ms IS NULL")
        .bind(now_ms())
        .bind(cookie)
        .execute(pool)
        .await?;
    Ok(())
}
