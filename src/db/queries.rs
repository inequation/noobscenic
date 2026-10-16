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
        "INSERT INTO devices (sn, ld_sn, label, bind_state, first_seen_ms, last_seen_ms)
         VALUES (?, ?, ?, 'unbound', ?, ?)
         ON CONFLICT(sn) DO UPDATE SET
             ld_sn = COALESCE(excluded.ld_sn, devices.ld_sn),
             last_seen_ms = excluded.last_seen_ms",
    )
    .bind(sn)
    .bind(ld_sn)
    .bind(sn) // the visible label starts as the serial number
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
                              label, bind_state, first_seen_ms, last_seen_ms)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'unbound', ?, ?)
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
    .bind(sn) // the visible label starts as the serial number
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// The device's recorded bind state, for the binding handler's log line.
pub async fn bind_state(pool: &SqlitePool, sn: &str) -> Result<Option<String>> {
    Ok(
        sqlx::query_scalar("SELECT bind_state FROM devices WHERE sn = ?")
            .bind(sn)
            .fetch_optional(pool)
            .await?,
    )
}

/// Record a successful bind (`bind: true`) or unbind. The device's preBind retries
/// until it sees `code:0`, so an already-bound row is simply re-stamped; unbinding
/// clears the user and leaves the bind timestamp as history.
pub async fn record_bind(
    pool: &SqlitePool,
    sn: &str,
    bind: bool,
    user_id: Option<&str>,
) -> Result<()> {
    let now = now_ms();
    if bind {
        sqlx::query(
            "UPDATE devices SET bind_state = 'bound', bind_user = ?, bound_ms = ?,
                                 last_seen_ms = ?
             WHERE sn = ?",
        )
        .bind(user_id)
        .bind(now)
        .bind(now)
        .bind(sn)
        .execute(pool)
        .await?;
    } else {
        sqlx::query(
            "UPDATE devices SET bind_state = 'unbound', bind_user = NULL, unbound_ms = ?,
                                 last_seen_ms = ?
             WHERE sn = ?",
        )
        .bind(now)
        .bind(now)
        .bind(sn)
        .execute(pool)
        .await?;
    }
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
    let mut tx = pool.begin().await?;
    let id: Option<i64> = sqlx::query_scalar(
        "INSERT INTO sessions (sn, session_key, cookie, sig_raw, created_ms)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(cookie) DO NOTHING
         RETURNING id",
    )
    .bind(sn)
    .bind(session_key)
    .bind(cookie)
    .bind(sig_raw)
    .bind(now_ms())
    .fetch_optional(&mut *tx)
    .await?;
    let Some(id) = id else {
        tx.rollback().await?;
        return Ok(false);
    };
    // Without `history` only the newest session per device survives: the robot
    // re-registers on a stale cookie, and older sessions are dead anyway.
    #[cfg(not(feature = "history"))]
    {
        sqlx::query("DELETE FROM sessions WHERE sn = ? AND id <> ?")
            .bind(sn)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    #[cfg(feature = "history")]
    let _ = id;
    tx.commit().await?;
    Ok(true)
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

/// Without `history`, reduce `sessions` to the newest row per device. Older sessions
/// are dead once the robot re-registers, and only the latest can be live.
#[cfg(not(feature = "history"))]
pub async fn prune_sessions_latest_only(pool: &SqlitePool) -> Result<u64> {
    let result = sqlx::query(
        "DELETE FROM sessions
         WHERE EXISTS (
             SELECT 1 FROM sessions newer
             WHERE newer.sn = sessions.sn
               AND (newer.created_ms > sessions.created_ms
                    OR (newer.created_ms = sessions.created_ms AND newer.id > sessions.id))
         )",
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// The four endpoint spellings the robot uses for its bind request (with and without
/// the extra leading slash). Shared by `bind_events` and the history pruning.
const BIND_ENDPOINTS: [&str; 4] = [
    "/cleanPack/binding",
    "//cleanPack/binding",
    "/cleanPack/unbinding",
    "//cleanPack/unbinding",
];

#[cfg(not(feature = "history"))]
fn is_bind_endpoint(endpoint: &str) -> bool {
    BIND_ENDPOINTS.contains(&endpoint)
}

/// The bind endpoints as a SQL `IN` list — the values are constants, not input.
fn bind_endpoint_list() -> String {
    BIND_ENDPOINTS
        .iter()
        .map(|endpoint| format!("'{endpoint}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Every semantic message that has no dedicated table lands here verbatim.
///
/// With `history` off, only the two rows day-to-day operation reads survive:
/// the newest status (`20001` — the UI and the path poller) and the newest bind/unbind
/// record (the id recovery) per device. Everything else is dropped before it reaches
/// the table, and a database written with the feature on is pruned at startup
/// (`prune_events_latest_only`).
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
    #[cfg(not(feature = "history"))]
    {
        let status = info_type == Some(crate::proto::info_type::STATUS);
        if !status && !endpoint.is_some_and(is_bind_endpoint) {
            return Ok(());
        }
    }

    let mut tx = pool.begin().await?;
    // `id` is only read by the pruning below; a history build has no prune.
    #[allow(unused_variables)]
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO events (sn, channel, direction, endpoint, info_type, event_code, task_id,
                             user_id, device_ts, payload, trace_ref, received_ms)
         VALUES (?, ?, 'in', ?, ?, ?, ?, ?, ?, ?, ?, ?)
         RETURNING id",
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
    .fetch_one(&mut *tx)
    .await?;

    #[cfg(not(feature = "history"))]
    {
        if info_type == Some(crate::proto::info_type::STATUS) {
            sqlx::query("DELETE FROM events WHERE sn = ? AND info_type = ? AND id <> ?")
                .bind(sn)
                .bind(crate::proto::info_type::STATUS)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        } else {
            let sql = format!(
                "DELETE FROM events WHERE sn = ? AND id <> ? AND endpoint IN ({})",
                bind_endpoint_list()
            );
            sqlx::query(sqlx::AssertSqlSafe(sql))
                .bind(sn)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
    }

    tx.commit().await?;
    Ok(())
}

/// The 20002 grid, stored as received (LZ4 block); decode is a separate step.
///
/// Only the newest grid per device is kept: every reader serves the latest row, and
/// the NBMP backup slot (`map_backups`, doc/PLAN.md §20.6) is the archival mechanism.
/// Keeping a device's history here just grew the database by ~5.6 KiB per upload
/// (2026-10-12).
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
    let mut tx = pool.begin().await?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO map_uploads (sn, map_id, auto_area_id, path_id, width, height, resolution,
                                  x_min, y_min, lz4_len, cells_lz4, dock_x, dock_y, dock_phi,
                                  dock_state, areas_json, trace_ref, received_ms)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         RETURNING id",
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
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM map_uploads WHERE sn = ? AND id <> ?")
        .bind(sn)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// With `history` off: reduce `clean_paths` to the newest row per device.
/// Idempotent; `db::connect` runs it once per start.
#[cfg(not(feature = "history"))]
pub async fn prune_clean_paths_latest_only(pool: &SqlitePool) -> Result<u64> {
    let result = sqlx::query(
        "DELETE FROM clean_paths
         WHERE EXISTS (
             SELECT 1 FROM clean_paths newer
             WHERE newer.sn = clean_paths.sn
               AND (newer.updated_ms > clean_paths.updated_ms
                    OR (newer.updated_ms = clean_paths.updated_ms AND newer.id > clean_paths.id))
         )",
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
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
    let mut tx = pool.begin().await?;
    // `id` is only read by the pruning below; a history build has no prune.
    #[allow(unused_variables)]
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO clean_paths (sn, path_id, user_id, total_points, points_json, complete,
                                  updated_ms, trace_ref)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(sn, path_id) DO UPDATE SET
             user_id      = COALESCE(excluded.user_id, clean_paths.user_id),
             total_points = excluded.total_points,
             points_json  = excluded.points_json,
             complete     = excluded.complete,
             updated_ms   = excluded.updated_ms,
             trace_ref    = excluded.trace_ref
         RETURNING id",
    )
    .bind(sn)
    .bind(path_id)
    .bind(user_id)
    .bind(total_points)
    .bind(points_json)
    .bind(i64::from(complete))
    .bind(now_ms())
    .bind(trace_ref)
    .fetch_one(&mut *tx)
    .await?;
    // With `history` off only the newest path per device survives: every reader
    // takes the newest row, and the robot keeps the current path itself.
    #[cfg(not(feature = "history"))]
    {
        sqlx::query("DELETE FROM clean_paths WHERE sn = ? AND id <> ?")
            .bind(sn)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// `uploadLogs` / `uploadStats` / `uploadSingle`: kept exactly as received. This is a
/// debug archive with no reader in the server — the wire traces carry the bytes — so
/// only the `debug-raw` build writes it.
#[cfg(feature = "debug-raw")]
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

/// Without `debug-raw` the upload archive is skipped; the robot still gets its
/// `code:0` and the map-backup staging is independent (`map_backups`).
#[cfg(not(feature = "debug-raw"))]
pub async fn insert_upload_raw(
    _pool: &SqlitePool,
    _sn: Option<&str>,
    _endpoint: &str,
    _fields_json: &str,
    _trace_ref: Option<&str>,
) -> Result<()> {
    Ok(())
}

// ── Phase 5: the outbound command queue (doc/PLAN.md §10.3, §10.4) ─────────────

/// Enqueue one command. The gateway's poller picks it up while the device is
/// online; nothing is sent from here. Returns the row id.
pub async fn insert_command(
    pool: &SqlitePool,
    sn: &str,
    info_type: i64,
    payload: &str,
    encrypt: i64,
) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "INSERT INTO commands (sn, info_type, payload, encrypt, state, created_ms)
         VALUES (?, ?, ?, ?, 'pending', ?)
         RETURNING id",
    )
    .bind(sn)
    .bind(info_type)
    .bind(payload)
    .bind(encrypt)
    .bind(now_ms())
    .fetch_one(pool)
    .await?)
}

/// Expire commands older than `cutoff_ms`: `pending` rows that never went out, and
/// `sent` rows whose ACK never came back. Without the latter, a server restart or a
/// lost reply leaves a row in `sent` forever — and a stale `21011` row blocks the
/// path poller (2026-10-12).
#[cfg(feature = "history")]
pub async fn expire_commands(pool: &SqlitePool, cutoff_ms: i64) -> Result<u64> {
    let result = sqlx::query(
        "UPDATE commands SET state = 'expired', error = 'ttl'
         WHERE (state = 'pending' AND created_ms < ?)
            OR (state = 'sent' AND COALESCE(sent_ms, created_ms) < ?)",
    )
    .bind(cutoff_ms)
    .bind(cutoff_ms)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Without `history` the expired rows are dropped, with one warning each: an expired
/// command means either that the robot was unreachable or that it never answered.
#[cfg(not(feature = "history"))]
pub async fn expire_commands(pool: &SqlitePool, cutoff_ms: i64) -> Result<u64> {
    let rows: Vec<(i64, String, i64, String, i64, Option<i64>)> = sqlx::query_as(
        "DELETE FROM commands
         WHERE (state = 'pending' AND created_ms < ?)
            OR (state = 'sent' AND COALESCE(sent_ms, created_ms) < ?)
         RETURNING id, sn, info_type, state, created_ms, sent_ms",
    )
    .bind(cutoff_ms)
    .bind(cutoff_ms)
    .fetch_all(pool)
    .await?;
    let now = now_ms();
    for (id, sn, info_type, state, created_ms, sent_ms) in &rows {
        if state == "pending" {
            tracing::warn!(
                command_id = id,
                sn = %sn,
                info_type,
                age_ms = now - *created_ms,
                "command expired before the robot was reachable"
            );
        } else {
            tracing::warn!(
                command_id = id,
                sn = %sn,
                info_type,
                age_ms = now - sent_ms.unwrap_or(*created_ms),
                "command expired without an ACK"
            );
        }
    }
    Ok(rows.len() as u64)
}

/// Without `history` terminal command rows are never written; this clears any that a
/// database written with the feature on accumulated. `db::connect` runs it at startup.
#[cfg(not(feature = "history"))]
pub async fn prune_terminal_commands(pool: &SqlitePool) -> Result<u64> {
    let result = sqlx::query("DELETE FROM commands WHERE state IN ('acked', 'expired', 'failed')")
        .execute(pool)
        .await?;
    Ok(result.rows_affected())
}

/// Serial numbers with pending work — what the poller checks against the registry.
pub async fn pending_command_sns(pool: &SqlitePool) -> Result<Vec<String>> {
    Ok(
        sqlx::query_scalar("SELECT DISTINCT sn FROM commands WHERE state = 'pending' ORDER BY sn")
            .fetch_all(pool)
            .await?,
    )
}

/// One claimed command, ready to be framed and written.
#[derive(Debug, sqlx::FromRow)]
pub struct QueuedCommand {
    pub id: i64,
    pub info_type: i64,
    pub payload: String,
    pub encrypt: i64,
}

/// Hand every pending command for `sn` to the caller and mark it `sent` in the
/// same statement, so a poll can never push the same row twice.
pub async fn claim_commands(pool: &SqlitePool, sn: &str, now: i64) -> Result<Vec<QueuedCommand>> {
    Ok(sqlx::query_as(
        "UPDATE commands SET state = 'sent', sent_ms = ?
         WHERE sn = ? AND state = 'pending'
         RETURNING id, info_type, payload, encrypt",
    )
    .bind(now)
    .bind(sn)
    .fetch_all(pool)
    .await?)
}

/// Point a sent command at the tap record of the frame that carried it (doc/PLAN.md §8).
pub async fn set_command_trace(pool: &SqlitePool, id: i64, trace_ref: &str) -> Result<()> {
    sqlx::query("UPDATE commands SET trace_ref = ? WHERE id = ?")
        .bind(trace_ref)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// A command that will never be sent (no session key, writer closed, bad payload).
#[cfg(feature = "history")]
pub async fn fail_command(pool: &SqlitePool, id: i64, error: &str) -> Result<()> {
    // Clear `sent_ms`: the row was claimed, but the frame never crossed the socket,
    // and a `sent` timestamp on a `failed` row is exactly the kind of lie the
    // operator would act on (doc/PLAN.md §10.3).
    sqlx::query(
        "UPDATE commands SET state = 'failed', error = ?, sent_ms = NULL
         WHERE id = ? AND state != 'acked'",
    )
    .bind(error)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Without `history` the row is dropped; both call sites log the reason — a warning
/// from the push path, and one from the path poller's stale-fetch retirement.
#[cfg(not(feature = "history"))]
pub async fn fail_command(pool: &SqlitePool, id: i64, error: &str) -> Result<()> {
    let _ = error;
    sqlx::query("DELETE FROM commands WHERE id = ? AND state != 'acked'")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Best-effort ACK correlation (doc/PLAN.md §10.4): the newest `sent` command for
/// `(sn, info_type)` that was sent within the window becomes `acked` and keeps the
/// response body. No match is a no-op — the response is still persisted as an event.
#[cfg(feature = "history")]
pub async fn ack_command(
    pool: &SqlitePool,
    sn: &str,
    info_type: i64,
    ack_payload: &str,
    window_ms: i64,
) -> Result<Option<i64>> {
    Ok(sqlx::query_scalar(
        "UPDATE commands SET state = 'acked', ack_ms = ?, ack_payload = ?
         WHERE id = (SELECT id FROM commands
                     WHERE sn = ? AND info_type = ? AND state = 'sent'
                       AND sent_ms IS NOT NULL AND sent_ms >= ?
                     ORDER BY sent_ms DESC, id DESC LIMIT 1)
         RETURNING id",
    )
    .bind(now_ms())
    .bind(ack_payload)
    .bind(sn)
    .bind(info_type)
    .bind(now_ms() - window_ms)
    .fetch_optional(pool)
    .await?)
}

/// Without `history` the matched row is dropped: the response itself is dispatched
/// into its own tables, and the `commands` view only needs pending/sent rows.
#[cfg(not(feature = "history"))]
pub async fn ack_command(
    pool: &SqlitePool,
    sn: &str,
    info_type: i64,
    ack_payload: &str,
    window_ms: i64,
) -> Result<Option<i64>> {
    let _ = ack_payload;
    Ok(sqlx::query_scalar(
        "DELETE FROM commands
         WHERE id = (SELECT id FROM commands
                     WHERE sn = ? AND info_type = ? AND state = 'sent'
                       AND sent_ms IS NOT NULL AND sent_ms >= ?
                     ORDER BY sent_ms DESC, id DESC LIMIT 1)
         RETURNING id",
    )
    .bind(sn)
    .bind(info_type)
    .bind(now_ms() - window_ms)
    .fetch_optional(pool)
    .await?)
}

// ── Phase 5: what the console and the one-shot CLI print (doc/PLAN.md §12) ─────

// ── Phase 7: the web UI's API (doc/PLAN.md §19) ────────────────────────────────

/// `(sn, label, bind_user, bind_state, last_seen_ms)` for the robot dropdown.
pub async fn list_robots(
    pool: &SqlitePool,
) -> Result<Vec<(String, Option<String>, Option<String>, String, i64)>> {
    Ok(sqlx::query_as(
        "SELECT sn, label, bind_user, bind_state, last_seen_ms
         FROM devices ORDER BY last_seen_ms DESC",
    )
    .fetch_all(pool)
    .await?)
}

/// Resolve the UI's `{id}`: the editable label first, then the serial number, and the
/// `bind_user` only as a legacy fallback so bookmarks made while the UI used the cloud
/// account id still land somewhere. That id is the account the robot is bound to, not
/// a name for the robot, and is never displayed (operator decision, 2026-10-09).
pub async fn device_sn_by_id(pool: &SqlitePool, id: &str) -> Result<Option<String>> {
    let by_label: Option<String> = sqlx::query_scalar(
        "SELECT sn FROM devices WHERE label = ? ORDER BY last_seen_ms DESC LIMIT 1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    if by_label.is_some() {
        return Ok(by_label);
    }
    let by_sn: Option<String> = sqlx::query_scalar("SELECT sn FROM devices WHERE sn = ? LIMIT 1")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    if by_sn.is_some() {
        return Ok(by_sn);
    }
    let by_user: Option<String> = sqlx::query_scalar(
        "SELECT sn FROM devices WHERE bind_user = ? ORDER BY last_seen_ms DESC LIMIT 1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(by_user)
}

/// The user-visible label. Returns whether a device with that serial exists.
pub async fn set_device_label(pool: &SqlitePool, sn: &str, label: &str) -> Result<bool> {
    let result = sqlx::query("UPDATE devices SET label = ? WHERE sn = ?")
        .bind(label)
        .bind(sn)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() == 1)
}

/// The device's current label, for export filenames.
pub async fn device_label(pool: &SqlitePool, sn: &str) -> Result<Option<String>> {
    Ok(sqlx::query_scalar("SELECT label FROM devices WHERE sn = ?")
        .bind(sn)
        .fetch_optional(pool)
        .await?)
}

/// Store the newest map backup for a device (the one-slot model, doc/PLAN.md §20.6),
/// replacing whatever was staged before. The md5 is computed by the caller from
/// exactly these bytes — it is what the robot will compare on restore.
pub async fn upsert_map_backup(
    pool: &SqlitePool,
    sn: &str,
    record_name: Option<&str>,
    md5: &str,
    token: &str,
    blob: &[u8],
) -> Result<()> {
    sqlx::query(
        "INSERT INTO map_backups (sn, received_ms, record_name, md5, size, token, blob)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(sn) DO UPDATE SET
             received_ms = excluded.received_ms,
             record_name = excluded.record_name,
             md5         = excluded.md5,
             size        = excluded.size,
             token       = excluded.token,
             blob        = excluded.blob",
    )
    .bind(sn)
    .bind(now_ms())
    .bind(record_name)
    .bind(md5)
    .bind(blob.len() as i64)
    .bind(token)
    .bind(blob)
    .execute(pool)
    .await?;
    Ok(())
}

/// `(received_ms, record_name, md5, size)` of the staged backup, for the UI.
#[allow(clippy::type_complexity)]
pub async fn load_map_backup_meta(
    pool: &SqlitePool,
    sn: &str,
) -> Result<Option<(i64, Option<String>, String, i64)>> {
    Ok(
        sqlx::query_as("SELECT received_ms, record_name, md5, size FROM map_backups WHERE sn = ?")
            .bind(sn)
            .fetch_optional(pool)
            .await?,
    )
}

/// `(md5, blob)` of the staged backup, for an export download.
pub async fn load_map_backup(pool: &SqlitePool, sn: &str) -> Result<Option<(String, Vec<u8>)>> {
    Ok(
        sqlx::query_as("SELECT md5, blob FROM map_backups WHERE sn = ?")
            .bind(sn)
            .fetch_optional(pool)
            .await?,
    )
}

/// The exact bytes the robot fetches on restore (`BACKUP_MAP.md` §D1: no auth, no
/// transformation — the md5 must match byte for byte).
pub async fn load_map_backup_by_token(
    pool: &SqlitePool,
    sn: &str,
    token: &str,
) -> Result<Option<Vec<u8>>> {
    Ok(
        sqlx::query_scalar("SELECT blob FROM map_backups WHERE sn = ? AND token = ?")
            .bind(sn)
            .bind(token)
            .fetch_optional(pool)
            .await?,
    )
}

/// Devices whose `setID` id was never recorded — the candidates for recovering it
/// from a stored preBind (doc/PLAN.md §19).
pub async fn unbound_devices(pool: &SqlitePool) -> Result<Vec<String>> {
    Ok(
        sqlx::query_scalar("SELECT sn FROM devices WHERE bind_user IS NULL ORDER BY sn")
            .fetch_all(pool)
            .await?,
    )
}

/// Every stored bind-or-unbind request, oldest first, as
/// `(sn, endpoint, payload, received_ms)` — `sn` is NULL on rows the pre-phase-6
/// catch-all stored, where the body is the only place the serial survives.
pub async fn bind_events(
    pool: &SqlitePool,
) -> Result<Vec<(Option<String>, String, Option<String>, i64)>> {
    let sql = format!(
        "SELECT sn, endpoint, payload, received_ms FROM events
         WHERE endpoint IN ({}) ORDER BY id",
        bind_endpoint_list()
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .fetch_all(pool)
        .await?)
}

/// With `history` off: reduce a database written with it on to what day-to-day
/// operation reads — the newest status and the newest bind/unbind row per device.
/// Idempotent; `db::connect` runs it once per start.
#[cfg(not(feature = "history"))]
pub async fn prune_events_latest_only(pool: &SqlitePool) -> Result<u64> {
    let bind_list = bind_endpoint_list();
    let mut tx = pool.begin().await?;
    let mut removed = 0;

    // Everything that is neither a status nor a bind/unbind row is history.
    let sql = format!(
        "DELETE FROM events
         WHERE (info_type IS NULL OR info_type <> ?)
           AND (endpoint IS NULL OR endpoint NOT IN ({bind_list}))"
    );
    removed += sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(crate::proto::info_type::STATUS)
        .execute(&mut *tx)
        .await?
        .rows_affected();

    // Older status rows per device.
    removed += sqlx::query(
        "DELETE FROM events
         WHERE info_type = ?
           AND id NOT IN (SELECT max(id) FROM events WHERE info_type = ? GROUP BY sn)",
    )
    .bind(crate::proto::info_type::STATUS)
    .bind(crate::proto::info_type::STATUS)
    .execute(&mut *tx)
    .await?
    .rows_affected();

    // Older bind/unbind rows per device. Legacy NULL-sn rows are left alone: their
    // device cannot be told apart, and the recovery walk still needs them.
    let sql = format!(
        "DELETE FROM events
         WHERE endpoint IN ({bind_list}) AND sn IS NOT NULL
           AND id NOT IN (
               SELECT max(id) FROM events
               WHERE endpoint IN ({bind_list}) AND sn IS NOT NULL GROUP BY sn
           )"
    );
    removed += sqlx::query(sqlx::AssertSqlSafe(sql))
        .execute(&mut *tx)
        .await?
        .rows_affected();

    tx.commit().await?;
    Ok(removed)
}

/// Adopt a recovered `setID` id, but only where nothing has been recorded yet.
/// Returns whether the row changed.
pub async fn adopt_bind(pool: &SqlitePool, sn: &str, user_id: &str, bound_ms: i64) -> Result<bool> {
    let result = sqlx::query(
        "UPDATE devices SET bind_user = ?, bind_state = 'bound', bound_ms = ?
         WHERE sn = ? AND bind_user IS NULL",
    )
    .bind(user_id)
    .bind(bound_ms)
    .bind(sn)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// The newest stored payload of one `infoType` — the UI's status line.
pub async fn latest_event_payload(
    pool: &SqlitePool,
    sn: &str,
    info_type: i64,
) -> Result<Option<String>> {
    Ok(sqlx::query_scalar(
        "SELECT payload FROM events
         WHERE sn = ? AND info_type = ?
         ORDER BY received_ms DESC, id DESC LIMIT 1",
    )
    .bind(sn)
    .bind(info_type)
    .fetch_optional(pool)
    .await?)
}

/// Lean `(map_id, width, height, received_ms)` for the polled summary — the grid
/// blob stays in the database until the client actually needs it.
pub async fn latest_map_meta(
    pool: &SqlitePool,
    sn: &str,
) -> Result<Option<(Option<i64>, Option<i64>, Option<i64>, i64)>> {
    Ok(sqlx::query_as(
        "SELECT map_id, width, height, received_ms
         FROM map_uploads WHERE sn = ? ORDER BY received_ms DESC LIMIT 1",
    )
    .bind(sn)
    .fetch_optional(pool)
    .await?)
}

/// The newest map upload's `path_id` — the robot's current job path. The path poller
/// compares it with the stored path to notice that a new job has started.
pub async fn latest_map_path_id(pool: &SqlitePool, sn: &str) -> Result<Option<i64>> {
    Ok(sqlx::query_scalar(
        "SELECT path_id FROM map_uploads WHERE sn = ? ORDER BY received_ms DESC LIMIT 1",
    )
    .bind(sn)
    .fetch_optional(pool)
    .await?
    .flatten())
}

/// The newest map with its compressed grid, for `GET /api/robot/{id}/map`.
#[allow(clippy::type_complexity)]
pub async fn latest_map_row(
    pool: &SqlitePool,
    sn: &str,
) -> Result<
    Option<(
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Vec<u8>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<String>,
        i64,
    )>,
> {
    Ok(sqlx::query_as(
        "SELECT map_id, width, height, resolution, x_min, y_min, cells_lz4,
                dock_x, dock_y, dock_phi, dock_state, received_ms
         FROM map_uploads WHERE sn = ? ORDER BY received_ms DESC LIMIT 1",
    )
    .bind(sn)
    .fetch_optional(pool)
    .await?)
}

/// The oldest live command of one `infoType` for a device, if any: `(id, sent_ms,
/// created_ms)`. The path poller uses this to retire a request that has gone
/// unacked instead of letting it block every later poll.
pub async fn active_command(
    pool: &SqlitePool,
    sn: &str,
    info_type: i64,
) -> Result<Option<(i64, Option<i64>, i64)>> {
    Ok(sqlx::query_as(
        "SELECT id, sent_ms, created_ms FROM commands
         WHERE sn = ? AND info_type = ? AND state IN ('pending', 'sent')
         ORDER BY id LIMIT 1",
    )
    .bind(sn)
    .bind(info_type)
    .fetch_optional(pool)
    .await?)
}

/// The newest known zone list (`AreaSetting`) for a device.
#[derive(Debug, sqlx::FromRow)]
pub struct AreaSettings {
    pub version: i64,
    pub read_ms: Option<i64>,
    pub payload: Option<String>,
    pub trace_ref: Option<String>,
}

pub async fn load_area_settings(pool: &SqlitePool, sn: &str) -> Result<Option<AreaSettings>> {
    Ok(sqlx::query_as(
        "SELECT version, read_ms, payload, trace_ref FROM area_settings WHERE sn = ?",
    )
    .bind(sn)
    .fetch_optional(pool)
    .await?)
}

/// A zone list that just came back from the robot (`21004`): it becomes the known
/// truth and bumps the etag. Returns the new version.
pub async fn store_area_settings(
    pool: &SqlitePool,
    sn: &str,
    payload: &str,
    trace_ref: Option<&str>,
) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "INSERT INTO area_settings (sn, version, read_ms, payload, trace_ref)
         VALUES (?, 1, ?, ?, ?)
         ON CONFLICT(sn) DO UPDATE SET
             version   = area_settings.version + 1,
             read_ms   = excluded.read_ms,
             payload   = excluded.payload,
             trace_ref = excluded.trace_ref
         RETURNING version",
    )
    .bind(sn)
    .bind(now_ms())
    .bind(payload)
    .bind(trace_ref)
    .fetch_one(pool)
    .await?)
}

/// A zone list we just sent to the robot (`21003`): adopt it optimistically, keep the
/// old list as the one-step snapshot, and bump the etag so an editor that read the
/// previous version cannot save it again. Returns the new version.
pub async fn record_zone_write(pool: &SqlitePool, sn: &str, payload: &str) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "INSERT INTO area_settings (sn, version, read_ms, payload, trace_ref)
         VALUES (?, 1, ?, ?, NULL)
         ON CONFLICT(sn) DO UPDATE SET
             previous_payload = area_settings.payload,
             previous_ms      = area_settings.read_ms,
             version          = area_settings.version + 1,
             read_ms          = excluded.read_ms,
             payload          = excluded.payload
         RETURNING version",
    )
    .bind(sn)
    .bind(now_ms())
    .bind(payload)
    .fetch_one(pool)
    .await?)
}

/// The newest `21015` goods reply — the consumables counters, in seconds of
/// cleaning time (FUNC_STATUS.md §7). One row per device, read with `GET
/// /api/robot/{id}/goods`.
pub async fn load_goods(
    pool: &SqlitePool,
    sn: &str,
) -> Result<Option<(Option<i64>, Option<String>)>> {
    Ok(
        sqlx::query_as("SELECT read_ms, payload FROM goods WHERE sn = ?")
            .bind(sn)
            .fetch_optional(pool)
            .await?,
    )
}

/// A goods reply that just came back from the robot (`21015`): it replaces the
/// cached counters. The robot's reply is the only source, so there is no etag —
/// `read_ms` is enough for the page's read-back-and-wait refresh.
pub async fn store_goods(
    pool: &SqlitePool,
    sn: &str,
    payload: &str,
    trace_ref: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO goods (sn, read_ms, payload, trace_ref)
         VALUES (?, ?, ?, ?)
         ON CONFLICT(sn) DO UPDATE SET
             read_ms   = excluded.read_ms,
             payload   = excluded.payload,
             trace_ref = excluded.trace_ref",
    )
    .bind(sn)
    .bind(now_ms())
    .bind(payload)
    .bind(trace_ref)
    .execute(pool)
    .await?;
    Ok(())
}

/// Every device ever seen, for `devices`.
pub async fn list_devices(
    pool: &SqlitePool,
) -> Result<Vec<(String, String, Option<String>, Option<String>, i64)>> {
    Ok(sqlx::query_as(
        "SELECT sn, bind_state, mcu_ver, app_version, last_seen_ms
         FROM devices ORDER BY last_seen_ms DESC",
    )
    .fetch_all(pool)
    .await?)
}

/// The console's/CLI's `device <sn>` line.
pub async fn device_summary(
    pool: &SqlitePool,
    sn: &str,
) -> Result<
    Option<(
        String,
        Option<String>,
        String,
        Option<String>,
        Option<String>,
        i64,
        i64,
    )>,
> {
    Ok(sqlx::query_as(
        "SELECT sn, ld_sn, bind_state, mcu_ver, app_version, first_seen_ms, last_seen_ms
         FROM devices WHERE sn = ?",
    )
    .bind(sn)
    .fetch_optional(pool)
    .await?)
}

/// `(created_ms, last_used_ms, revoked_ms, key_len)` of the newest session.
pub async fn latest_session_summary(
    pool: &SqlitePool,
    sn: &str,
) -> Result<Option<(i64, Option<i64>, Option<i64>, i64)>> {
    Ok(sqlx::query_as(
        "SELECT created_ms, last_used_ms, revoked_ms, length(session_key)
         FROM sessions WHERE sn = ? ORDER BY created_ms DESC, id DESC LIMIT 1",
    )
    .bind(sn)
    .fetch_optional(pool)
    .await?)
}

/// The newest semantic messages, for `events [<sn>] [n]`.
#[derive(Debug, sqlx::FromRow)]
pub struct EventSummary {
    pub id: i64,
    pub channel: String,
    pub info_type: Option<i64>,
    pub endpoint: Option<String>,
    pub payload: Option<String>,
    pub trace_ref: Option<String>,
    pub received_ms: i64,
}

pub async fn recent_events(
    pool: &SqlitePool,
    sn: Option<&str>,
    limit: i64,
) -> Result<Vec<EventSummary>> {
    match sn {
        Some(sn) => Ok(sqlx::query_as(
            "SELECT id, channel, info_type, endpoint, payload, trace_ref, received_ms
             FROM events WHERE sn = ? ORDER BY id DESC LIMIT ?",
        )
        .bind(sn)
        .bind(limit)
        .fetch_all(pool)
        .await?),
        None => Ok(sqlx::query_as(
            "SELECT id, channel, info_type, endpoint, payload, trace_ref, received_ms
             FROM events ORDER BY id DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(pool)
        .await?),
    }
}

/// The newest map's identity and frame, for `map <sn>`.
pub async fn latest_map_summary(
    pool: &SqlitePool,
    sn: &str,
) -> Result<
    Option<(
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Option<i64>,
        Option<i64>,
        Option<String>,
        Option<String>,
        i64,
    )>,
> {
    Ok(sqlx::query_as(
        "SELECT map_id, path_id, width, height, resolution, x_min, y_min, dock_x, dock_y,
                dock_state, areas_json, received_ms
         FROM map_uploads WHERE sn = ? ORDER BY received_ms DESC LIMIT 1",
    )
    .bind(sn)
    .fetch_optional(pool)
    .await?)
}

/// `(path_id, points_json, total_points, complete, updated_ms)` — the newest stored
/// path, or one named row.
pub async fn clean_path_summary(
    pool: &SqlitePool,
    sn: &str,
    path_id: Option<i64>,
) -> Result<Option<(i64, String, i64, i64, i64)>> {
    match path_id {
        Some(path_id) => Ok(sqlx::query_as(
            "SELECT path_id, points_json, total_points, complete, updated_ms
             FROM clean_paths WHERE sn = ? AND path_id = ?",
        )
        .bind(sn)
        .bind(path_id)
        .fetch_optional(pool)
        .await?),
        None => Ok(sqlx::query_as(
            "SELECT path_id, points_json, total_points, complete, updated_ms
             FROM clean_paths WHERE sn = ? ORDER BY updated_ms DESC LIMIT 1",
        )
        .bind(sn)
        .fetch_optional(pool)
        .await?),
    }
}

/// The newest commands, for `commands [<sn>]`.
#[derive(Debug, sqlx::FromRow)]
pub struct CommandSummary {
    pub id: i64,
    pub sn: String,
    pub info_type: i64,
    pub encrypt: i64,
    pub state: String,
    pub created_ms: i64,
    pub sent_ms: Option<i64>,
    pub ack_ms: Option<i64>,
    pub error: Option<String>,
    pub trace_ref: Option<String>,
}

pub async fn recent_commands(
    pool: &SqlitePool,
    sn: Option<&str>,
    limit: i64,
) -> Result<Vec<CommandSummary>> {
    match sn {
        Some(sn) => Ok(sqlx::query_as(
            "SELECT id, sn, info_type, encrypt, state, created_ms, sent_ms, ack_ms, error, trace_ref
             FROM commands WHERE sn = ? ORDER BY id DESC LIMIT ?",
        )
        .bind(sn)
        .bind(limit)
        .fetch_all(pool)
        .await?),
        None => Ok(sqlx::query_as(
            "SELECT id, sn, info_type, encrypt, state, created_ms, sent_ms, ack_ms, error, trace_ref
             FROM commands ORDER BY id DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(pool)
        .await?),
    }
}
