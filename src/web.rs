//! The web UI — doc/PLAN.md §19.
//!
//! Served by the channel-A listener, in this process, on purpose: the deployment
//! target is one home and the page's only client is the operator on the same LAN.
//! There is no authentication in the MVP.
//!
//! The page polls `/api/robot/{id}/summary` every ~1.5 s and pulls the (large) map
//! and path only when their revisions change. The same poll is a **presence
//! heartbeat**: the path poller asks the robot for `21011` chunks only while
//! somebody is watching, because the robot pushes maps but never paths.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use axum::Json;
use axum::extract::{Form, Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tokio::sync::watch;

use crate::AppState;
use crate::channel_a::handlers;
use crate::channel_b::codec;
use crate::db::{now_ms, queries};
use crate::error::Error;
use crate::proto::info_type;

/// The page, embedded at compile time.
const INDEX: &str = include_str!("../web/index.html");

/// How long after its last API touch a device still counts as watched. The page's
/// 1.5 s poll keeps this fresh; closing (or hiding) the tab lets it lapse.
const WATCHER_TTL_MS: i64 = 5_000;

/// The presence-gated path poller's cadence while someone is watching.
const PATH_POLL_INTERVAL: Duration = Duration::from_secs(5);

/// The argument-less commands the UI offers, by raw name. Every entry is a fixed
/// frame; the queue, its TTL and the ACK correlation do the rest (doc/PLAN.md §19).
const CATALOG: &[(&str, i64, &str)] = &[
    ("smartClean", 21005, "{\"mode\":\"smartClean\"}"),
    ("pause", 21017, "{\"cmd\":\"pause\"}"),
    ("continue", 21017, "{\"cmd\":\"continue\"}"),
    ("stop", 21017, "{\"cmd\":\"stop\"}"),
    ("findCharge", 21012, "{\"cmd\":\"start\"}"),
    // Pausing the *return* is 21012, not the cleaning pause above (FUNC_COMMANDS §1.3).
    ("pauseReturn", 21012, "{\"cmd\":\"pause\"}"),
];

/// The realtime steering set: 3005–3008 move, 4000 leaves manual mode, 4001 zeroes
/// the speed (FUNC_COMMANDS §2.1). Everything else is refused.
const CONTROL_CODES: [i64; 6] = [3005, 3006, 3007, 3008, 4000, 4001];

/// Which robots' APIs were touched recently enough to count as watched.
#[derive(Default)]
pub struct Watchers {
    last_touch: Mutex<HashMap<String, i64>>,
}

impl Watchers {
    pub fn new() -> Watchers {
        Watchers::default()
    }

    /// The page's poll, or a command send, just happened for `sn`.
    pub fn touch(&self, sn: &str) {
        self.lock().insert(sn.to_string(), now_ms());
    }

    pub fn fresh(&self, sn: &str) -> bool {
        self.lock()
            .get(sn)
            .is_some_and(|last| now_ms() - last < WATCHER_TTL_MS)
    }

    fn watched(&self) -> Vec<String> {
        self.lock().keys().cloned().collect()
    }

    /// A poisoned lock must not take the UI down; the map is only ever inserted
    /// into and read.
    fn lock(&self) -> MutexGuard<'_, HashMap<String, i64>> {
        self.last_touch
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A touch at an arbitrary time, for the freshness test.
    #[cfg(test)]
    fn touch_at(&self, sn: &str, ms: i64) {
        self.lock().insert(sn.to_string(), ms);
    }
}

/// `GET /` — the page, unless this is the device's OTA version check (`version` or
/// `sn` in the query), which keeps its old JSON answer.
pub async fn index(
    State(state): State<AppState>,
    query: Query<HashMap<String, String>>,
) -> Response {
    if query.contains_key("version") || query.contains_key("sn") {
        return handlers::sync::version_check(State(state), query).await;
    }
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], INDEX).into_response()
}

/// `GET /api/robots` — the dropdown's data. `id` is the `setID` id (`bind_user`)
/// with the serial number as fallback.
pub async fn robots(State(state): State<AppState>) -> Response {
    let rows = match queries::list_robots(&state.db).await {
        Ok(rows) => rows,
        Err(error) => return internal(error),
    };
    let robots: Vec<Value> = rows
        .into_iter()
        .map(|(sn, bind_user, bind_state, last_seen_ms)| {
            let online = state.registry.is_online(&sn);
            let id = bind_user.unwrap_or_else(|| sn.clone());
            json!({
                "id": id,
                "sn": sn,
                "online": online,
                "bind_state": bind_state,
                "last_seen_ms": last_seen_ms,
            })
        })
        .collect();
    Json(json!({"robots": robots})).into_response()
}

/// `GET /api/commands` — the catalog's raw names, so page and server agree.
pub async fn commands() -> Response {
    let names: Vec<&'static str> = CATALOG.iter().map(|(name, _, _)| *name).collect();
    Json(json!({"commands": names})).into_response()
}

/// `GET /api/robot/{id}/summary` — everything the page polls: connection, latest
/// status, and the path/map revisions.
pub async fn summary(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Some(sn) = resolve(&state, &id).await else {
        return unknown(&id);
    };
    state.watchers.touch(&sn);

    let status = match queries::latest_event_payload(&state.db, &sn, info_type::STATUS).await {
        Ok(Some(payload)) => serde_json::from_str::<Value>(&payload).ok(),
        Ok(None) => None,
        Err(error) => return internal(error),
    };

    let path = match queries::clean_path_summary(&state.db, &sn, None).await {
        Ok(Some((path_id, points_json, total_points, complete, updated_ms))) => {
            let assembly = crate::path::Assembly::from_json(&points_json);
            json!({
                "path_id": path_id,
                "points": assembly.filled(),
                "span": assembly.len(),
                "total": total_points,
                "complete": complete != 0,
                "updated_ms": updated_ms,
            })
        }
        Ok(None) => Value::Null,
        Err(error) => return internal(error),
    };

    let map = match queries::latest_map_meta(&state.db, &sn).await {
        Ok(Some((map_id, width, height, received_ms))) => json!({
            "map_id": map_id,
            "width": width,
            "height": height,
            "received_ms": received_ms,
        }),
        Ok(None) => Value::Null,
        Err(error) => return internal(error),
    };

    Json(json!({
        "id": id,
        "sn": sn,
        "online": state.registry.is_online(&sn),
        "status": status,
        "path": path,
        "map": map,
    }))
    .into_response()
}

/// `GET /api/robot/{id}/map` — the newest grid, decompressed, in wire order (the
/// client flips the rows for a floor-plan view).
pub async fn map(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Some(sn) = resolve(&state, &id).await else {
        return unknown(&id);
    };
    state.watchers.touch(&sn);

    let row = match queries::latest_map_row(&state.db, &sn).await {
        Ok(Some(row)) => row,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "no map stored for this robot"})),
            )
                .into_response();
        }
        Err(error) => return internal(error),
    };
    let (
        map_id,
        width,
        height,
        resolution,
        x_min,
        y_min,
        cells_lz4,
        dock_x,
        dock_y,
        dock_phi,
        dock_state,
        received_ms,
    ) = row;
    let width = width.unwrap_or(0).max(0) as usize;
    let height = height.unwrap_or(0).max(0) as usize;
    let grid = match crate::map::decompress_stored(&cells_lz4, width, height) {
        Ok(grid) => grid,
        Err(error) => return internal(error),
    };

    Json(json!({
        "map_id": map_id,
        "received_ms": received_ms,
        "width": width,
        "height": height,
        "resolution": resolution,
        "x_min": x_min,
        "y_min": y_min,
        "dock_x": dock_x,
        "dock_y": dock_y,
        "dock_phi": dock_phi,
        "dock_state": dock_state,
        "grid_b64": base64::engine::general_purpose::STANDARD.encode(&grid),
    }))
    .into_response()
}

/// `GET /api/robot/{id}/path` — the newest assembled path with the low 2-bit
/// point-type tags stripped (`v & !3`, FUNC_MAP.md §3).
pub async fn path(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Some(sn) = resolve(&state, &id).await else {
        return unknown(&id);
    };
    state.watchers.touch(&sn);

    let summary = match queries::clean_path_summary(&state.db, &sn, None).await {
        Ok(summary) => summary,
        Err(error) => return internal(error),
    };
    let Some((path_id, points_json, _, complete, updated_ms)) = summary else {
        return Json(json!({
            "path_id": null, "points": [], "complete": false, "updated_ms": null,
        }))
        .into_response();
    };

    let stored: Vec<Option<[f64; 2]>> = serde_json::from_str(&points_json).unwrap_or_default();
    let points: Vec<[i64; 2]> = stored
        .into_iter()
        .flatten()
        .map(|[x, y]| [(x as i64) & !3, (y as i64) & !3])
        .collect();
    Json(json!({
        "path_id": path_id,
        "points": points,
        "complete": complete != 0,
        "updated_ms": updated_ms,
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct CommandForm {
    name: String,
}

#[derive(Deserialize)]
pub struct ControlForm {
    code: i64,
}

/// `POST /api/robot/{id}/control` — one realtime `21020` frame for the steering pad.
///
/// This deliberately bypasses the commands queue (doc/PLAN.md §20.4): the robot
/// zeroes its commanded speed after 400 ms without a fresh frame, so a one-second
/// queue poll could never drive it. Frames go straight to the device's writer the
/// way pongs do — no database row, no ACK to wait for (21020 has no reply).
pub async fn control(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<ControlForm>,
) -> Response {
    let Some(sn) = resolve(&state, &id).await else {
        return unknown(&id);
    };
    state.watchers.touch(&sn);

    if !CONTROL_CODES.contains(&form.code) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("unsupported control code {}", form.code)})),
        )
            .into_response();
    }
    let Some(handle) = state.registry.get(&sn) else {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error": "device is offline"})),
        )
            .into_response();
    };

    let frame = codec::envelope(
        0,
        codec::message(21020, json!({"ctrlCode": form.code}), None),
    );
    match handle
        .tx
        .send(crate::channel_b::registry::Outbound::frame(frame))
        .await
    {
        Ok(()) => Json(json!({"code": form.code})).into_response(),
        Err(_) => (
            StatusCode::CONFLICT,
            Json(json!({"error": "connection is gone"})),
        )
            .into_response(),
    }
}

/// `POST /api/robot/{id}/command` — one catalog name into the queue.
pub async fn command(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<CommandForm>,
) -> Response {
    let Some(sn) = resolve(&state, &id).await else {
        return unknown(&id);
    };
    state.watchers.touch(&sn);

    let Some((_, info_type, payload)) = CATALOG.iter().find(|(name, _, _)| *name == form.name)
    else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("unknown command {}", form.name)})),
        )
            .into_response();
    };

    match queries::insert_command(&state.db, &sn, *info_type, payload, 0).await {
        Ok(command_id) => Json(json!({"queued": command_id, "name": form.name})).into_response(),
        Err(error) => internal(error),
    }
}

/// Spawn the presence-gated path poller.
pub fn spawn_path_tracker(state: AppState, mut shutdown: watch::Receiver<()>) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(PATH_POLL_INTERVAL);
        loop {
            tokio::select! {
                _ = shutdown.changed() => break,
                _ = ticker.tick() => poll_paths_once(&state).await,
            }
        }
    });
}

/// One pass of the path poller: ask each watched, online, not-charging robot for the
/// next `21011` chunk, from the first point we do not have yet. Public so tests can
/// drive it without the timer.
pub async fn poll_paths_once(state: &AppState) {
    for sn in state.watchers.watched() {
        if !state.watchers.fresh(&sn) || !state.registry.is_online(&sn) {
            continue;
        }
        // Charging robots have no growing path; no status at all means we do not
        // know enough to spend a request.
        if latest_mode(state, &sn)
            .await
            .is_none_or(|mode| mode.contains("charge"))
        {
            continue;
        }
        match queries::has_active_command(&state.db, &sn, info_type::PATH).await {
            Ok(false) => {}
            Ok(true) => continue,
            Err(error) => {
                tracing::error!(%error, sn = %sn, "path poller: could not check the queue");
                continue;
            }
        }
        let start_pos = match queries::clean_path_summary(&state.db, &sn, None).await {
            Ok(Some((_, points_json, _, _, _))) => {
                crate::path::Assembly::from_json(&points_json).next_index()
            }
            Ok(None) => 0,
            Err(error) => {
                tracing::error!(%error, sn = %sn, "path poller: could not read the stored path");
                continue;
            }
        };
        let payload = json!({"startPos": start_pos, "mask": 0}).to_string();
        match queries::insert_command(&state.db, &sn, info_type::PATH, &payload, 0).await {
            Ok(command_id) => {
                tracing::debug!(sn = %sn, command_id, start_pos, "path poll queued")
            }
            Err(error) => {
                tracing::error!(%error, sn = %sn, "path poller: could not enqueue the fetch")
            }
        }
    }
}

async fn latest_mode(state: &AppState, sn: &str) -> Option<String> {
    let payload = queries::latest_event_payload(&state.db, sn, info_type::STATUS)
        .await
        .ok()
        .flatten()?;
    let value: Value = serde_json::from_str(&payload).ok()?;
    value
        .get("mode")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// One import per boot, idempotent: a robot that bound before the phase-6 handler
/// existed has its preBind stored as an event — the old catch-all kept the raw form
/// body, the handler stores JSON. Recover the `setID` id from the newest bind-or-unbind
/// event so the UI can resolve `?id=` without waiting for a re-bind; from then on the
/// state lives in SQLite and survives restarts like every other row. Public for tests.
pub async fn recover_bind_ids(pool: &SqlitePool) -> u64 {
    let devices = match queries::unbound_devices(pool).await {
        Ok(devices) => devices,
        Err(error) => {
            tracing::warn!(%error, "could not list devices for bind recovery");
            return 0;
        }
    };
    if devices.is_empty() {
        return 0;
    }
    let events = match queries::bind_events(pool).await {
        Ok(events) => events,
        Err(error) => {
            tracing::warn!(%error, "could not read the stored bind events");
            return 0;
        }
    };
    // The newest bind-or-unbind state per serial. The catch-all stored its rows with
    // a NULL `sn`, so there the body is the only place the serial survives.
    let mut latest: HashMap<String, (bool, Option<String>, i64)> = HashMap::new();
    for (sn, endpoint, payload, received_ms) in events {
        let (body_sn, user) = bind_fields_from_payload(&payload.unwrap_or_default());
        let Some(sn) = sn.or(body_sn) else {
            continue;
        };
        let bound = matches!(
            endpoint.as_str(),
            "/cleanPack/binding" | "//cleanPack/binding"
        );
        latest.insert(sn, (bound, user, received_ms));
    }

    let mut recovered = 0;
    for sn in devices {
        let Some((bound, user, received_ms)) = latest.get(&sn) else {
            continue;
        };
        // Only adopt from a *bind*: if the newest of the pair is an unbind, the robot
        // is knowingly unbound and we must not resurrect the old id.
        let Some(user) = user.as_ref().filter(|_| *bound) else {
            continue;
        };
        match queries::adopt_bind(pool, &sn, user, *received_ms).await {
            Ok(true) => {
                tracing::info!(
                    sn = %sn,
                    user = %user,
                    "recovered the setID id from a stored preBind"
                );
                recovered += 1;
            }
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(%error, sn = %sn, "could not adopt the recovered bind id")
            }
        }
    }
    recovered
}

/// `(sn, userId)` out of either storage shape: the handler's
/// `{"…","userId":"Foo"}` or the catch-all's raw `sn=…&ts=…&userId=Foo` form body.
fn bind_fields_from_payload(payload: &str) -> (Option<String>, Option<String>) {
    if let Ok(value) = serde_json::from_str::<Value>(payload) {
        return (field(value.get("sn")), field(value.get("userId")));
    }
    let form = crate::channel_a::form::Form::parse(payload.as_bytes());
    (
        form.get("sn").map(str::to_string),
        form.get("userId").map(str::to_string),
    )
}

fn field(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

async fn resolve(state: &AppState, id: &str) -> Option<String> {
    queries::device_sn_by_id(&state.db, id).await.ok().flatten()
}

fn unknown(id: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({"error": format!("unknown robot {id}")})),
    )
        .into_response()
}

fn internal(error: Error) -> Response {
    tracing::error!(%error, "web API request failed");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "internal"})),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freshness_lapses_with_the_watcher_ttl() {
        let watchers = Watchers::new();
        assert!(!watchers.fresh("SN"), "never touched: not watched");
        watchers.touch_at("SN", now_ms() - WATCHER_TTL_MS + 1_000);
        assert!(watchers.fresh("SN"), "inside the window");
        watchers.touch_at("SN", now_ms() - WATCHER_TTL_MS - 1);
        assert!(!watchers.fresh("SN"), "past the window");
    }
}
