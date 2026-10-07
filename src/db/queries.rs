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

/// Every semantic message that has no dedicated table lands here verbatim.
#[allow(clippy::too_many_arguments)]
pub async fn insert_event(
    pool: &SqlitePool,
    sn: &str,
    channel: &str,
    endpoint: Option<&str>,
    info_type: Option<i64>,
    event_code: Option<i64>,
    task_id: Option<&str>,
    user_id: Option<&str>,
    device_ts: Option<&str>,
    payload: &str,
    trace_ref: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO events (sn, channel, direction, endpoint, info_type, event_code, task_id,
                             user_id, device_ts, payload, trace_ref, received_ms)
         VALUES (?, ?, 'in', ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(sn)
    .bind(channel)
    .bind(endpoint)
    .bind(info_type)
    .bind(event_code)
    .bind(task_id)
    .bind(user_id)
    .bind(device_ts)
    .bind(payload)
    .bind(trace_ref)
    .bind(now_ms())
    .execute(pool)
    .await?;
    Ok(())
}

/// The 20002 grid, stored as received (LZ4 block); decode is a separate step.
pub async fn insert_map_upload(
    pool: &SqlitePool,
    sn: &str,
    upload: &crate::map::MapUpload,
    compressed: &[u8],
    trace_ref: Option<&str>,
) -> Result<()> {
    let (dock_x, dock_y) = upload
        .charge_handle_pos
        .as_ref()
        .and_then(|pos| Some((*pos.first()?, *pos.get(1)?)))
        .map(|(x, y)| (Some(x), Some(y)))
        .unwrap_or((None, None));
    sqlx::query(
        "INSERT INTO map_uploads (sn, map_id, auto_area_id, path_id, width, height, resolution,
                                  x_min, y_min, lz4_len, cells_lz4, dock_x, dock_y, dock_phi,
                                  dock_state, areas_json, trace_ref, received_ms)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(sn)
    .bind(upload.map_id)
    .bind(upload.auto_area_id)
    .bind(upload.path_id)
    .bind(upload.width)
    .bind(upload.height)
    .bind(upload.resolution)
    .bind(upload.x_min)
    .bind(upload.y_min)
    .bind(upload.lz4_len)
    .bind(compressed)
    .bind(dock_x)
    .bind(dock_y)
    .bind(upload.charge_handle_phi)
    .bind(upload.charge_handle_state.as_deref())
    .bind(serde_json::to_string(&upload.area).unwrap_or_else(|_| "[]".to_string()))
    .bind(trace_ref)
    .bind(now_ms())
    .execute(pool)
    .await?;
    Ok(())
}

/// The stored assembly for `(sn, path_id)`: `(points_json, total_points, complete)`.
pub async fn load_clean_path(
    pool: &SqlitePool,
    sn: &str,
    path_id: i64,
) -> Result<Option<(String, i64, i64)>> {
    Ok(sqlx::query_as(
        "SELECT points_json, total_points, complete FROM clean_paths WHERE sn = ? AND path_id = ?",
    )
    .bind(sn)
    .bind(path_id)
    .fetch_optional(pool)
    .await?)
}

#[allow(clippy::too_many_arguments)]
pub async fn upsert_clean_path(
    pool: &SqlitePool,
    sn: &str,
    path_id: i64,
    user_id: Option<&str>,
    total_points: i64,
    points_json: &str,
    complete: bool,
    trace_ref: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO clean_paths (sn, path_id, user_id, total_points, points_json, complete,
                                  updated_ms, trace_ref)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(sn, path_id) DO UPDATE SET
             user_id      = COALESCE(excluded.user_id, clean_paths.user_id),
             total_points = excluded.total_points,
             points_json  = excluded.points_json,
             complete     = excluded.complete,
             updated_ms   = excluded.updated_ms,
             trace_ref    = excluded.trace_ref",
    )
    .bind(sn)
    .bind(path_id)
    .bind(user_id)
    .bind(total_points)
    .bind(points_json)
    .bind(i64::from(complete))
    .bind(now_ms())
    .bind(trace_ref)
    .execute(pool)
    .await?;
    Ok(())
}

/// `uploadLogs` / `uploadStats` / `uploadSingle`: kept exactly as received.
pub async fn insert_upload_raw(
    pool: &SqlitePool,
    sn: Option<&str>,
    endpoint: &str,
    fields_json: &str,
    trace_ref: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO uploads_raw (sn, endpoint, fields_json, trace_ref, received_ms)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(sn)
    .bind(endpoint)
    .bind(fields_json)
    .bind(trace_ref)
    .bind(now_ms())
    .execute(pool)
    .await?;
    Ok(())
}
