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
use axum::extract::{Form, Path, Query, Request, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tokio::sync::watch;

use crate::AppState;
use crate::backup;
use crate::channel_a::handlers;
use crate::channel_b::{codec, registry};
use crate::db::{now_ms, queries};
use crate::error::Error;
use crate::proto::info_type;
use crate::session;

/// The page, embedded at compile time.
const INDEX: &str = include_str!("../web/index.html");

/// The home-screen icon and the web-app manifest, embedded like the page. The icon is
/// rendered from `web/logo.svg` (see that file's history for the exact command) at
/// 192×192 — the size that serves both the browser tab and a home-screen shortcut —
/// with the artwork's transparency kept as authored (the SVG page is white at zero
/// alpha; do not flatten it) and Zopfli-compressed inside the PNG.
const FAVICON: &[u8] = include_bytes!("../web/favicon.png");
const MANIFEST: &str = include_str!("../web/manifest.webmanifest");

/// How long after its last API touch a device still counts as watched. The page's
/// 1.5 s poll keeps this fresh; closing (or hiding) the tab lets it lapse.
const WATCHER_TTL_MS: i64 = 5_000;

/// The presence-gated path poller's cadence while someone is watching.
const PATH_POLL_INTERVAL: Duration = Duration::from_secs(5);

/// How long one `21011` fetch may sit pending/sent before the path poller retires it
/// and tries again. Three poll intervals is generous for a request the robot normally
/// acks in ~200 ms; without this, one lost ACK (a server restart mid-flight, a dropped
/// reply) blocks every later path fetch — the 2026-10-12 stale `sent` row.
const PATH_REQUEST_STALE_MS: i64 = 15_000;

/// How long after the last control frame the watchdog steps in and leaves manual mode
/// (doc/PLAN.md §20.4). The robot would wait 30 s of its own accord; a closed tab or a
/// dead Wi-Fi should not leave it in `rfctrl` that long.
const CONTROL_WATCHDOG_MS: i64 = 2_000;
const CONTROL_WATCHDOG_TICK: Duration = Duration::from_millis(500);

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
    ("deepClean", 21005, "{\"mode\":\"depthTotalClean\"}"),
    (
        "yMop",
        21005,
        "{\"mode\":\"smartClean\",\"pathType\":\"y_word\"}",
    ),
];

// The one-shot actions that ride the realtime `21020` path, all refused otherwise:
// steering 3005-3008, 4001 zero speed and 4000 leave manual mode (FUNC_COMMANDS
// §2.1); the locate sound's 3010/3011 (§6.3); the mute toggle 3022 (§6.2 — a
// toggle, so the client only sends it to reach the state the echo says it is not
// in); spot clean 3001 (FUNC_MAP §6.3); and the brushes/fan switch's 3025 on /
// 3026 off (FUNC_COMMANDS §4.5).
const CONTROL_CODES: [i64; 12] = [
    3001, 3005, 3006, 3007, 3008, 3010, 3011, 3022, 3025, 3026, 4000, 4001,
];

/// The codes that mean "the client is driving": only these arm the
/// leave-manual-mode watchdog. Mute, locate and spot clean are one-shot frames —
/// arming the watchdog for them would make the server send a pointless `4000` two
/// seconds later.
const STEERING_CODES: [i64; 5] = [3005, 3006, 3007, 3008, 4001];

/// Which robots' APIs were touched recently enough to count as watched.
#[derive(Default)]
pub struct Watchers {
    last_touch: Mutex<HashMap<String, i64>>,
}

/// Devices we have recently sent steering frames to, and when — what the watchdog
/// needs to know. Cleared by an explicit `4000` (the client leaving manual mode).
#[derive(Default)]
pub struct Controls {
    last_frame: Mutex<HashMap<String, i64>>,
}

impl Controls {
    pub fn new() -> Controls {
        Controls::default()
    }

    pub fn touch(&self, sn: &str) {
        self.lock().insert(sn.to_string(), now_ms());
    }

    pub fn clear(&self, sn: &str) {
        self.lock().remove(sn);
    }

    fn stale(&self, stale_ms: i64) -> Vec<String> {
        let now = now_ms();
        self.lock()
            .iter()
            .filter(|(_, at)| now - **at >= stale_ms)
            .map(|(sn, _)| sn.clone())
            .collect()
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, i64>> {
        self.last_frame
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[cfg(test)]
    fn touch_at(&self, sn: &str, ms: i64) {
        self.lock().insert(sn.to_string(), ms);
    }
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

/// `GET /api/robots` — the dropdown's data. `id` is the serial number (stable in a
/// bookmark); `label` is what the user sees and can rename. The cloud account the
/// robot is bound to (`bind_user`) is deliberately not exposed.
pub async fn robots(State(state): State<AppState>) -> Response {
    let rows = match queries::list_robots(&state.db).await {
        Ok(rows) => rows,
        Err(error) => return internal(error),
    };
    let robots: Vec<Value> = rows
        .into_iter()
        .map(
            |(sn, label, _bind_user, bind_state, last_seen_ms, firmware, mcu)| {
                let online = state.registry.is_online(&sn);
                let label = label.unwrap_or_else(|| sn.clone());
                json!({
                    "id": sn,
                    "label": label,
                    "sn": sn,
                    "online": online,
                    "bind_state": bind_state,
                    "last_seen_ms": last_seen_ms,
                    "firmware": firmware,
                    "mcu": mcu,
                })
            },
        )
        .collect();
    Json(json!({"robots": robots})).into_response()
}

/// `GET /favicon.png` and `/favicon.ico` — the home-screen/tab icon, embedded bytes.
pub async fn favicon() -> Response {
    (
        [
            (header::CONTENT_TYPE, "image/png".to_string()),
            (header::CACHE_CONTROL, "public, max-age=86400".to_string()),
        ],
        axum::body::Bytes::from_static(FAVICON),
    )
        .into_response()
}

/// `GET /manifest.webmanifest` — what makes "add shortcut to website" open without
/// browser chrome on Android: `display: standalone` plus the icon.
pub async fn manifest() -> Response {
    (
        [
            (
                header::CONTENT_TYPE,
                "application/manifest+json".to_string(),
            ),
            (header::CACHE_CONTROL, "public, max-age=86400".to_string()),
        ],
        MANIFEST,
    )
        .into_response()
}

#[derive(Deserialize)]
pub struct SettingsForm {
    label: String,
}

/// `PUT /api/robot/{id}/settings` — the user-visible settings. For now that is only
/// the label: a name for the robot in this UI, unrelated to the cloud account it is
/// bound to (operator decision, 2026-10-09).
pub async fn settings(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(form): Json<SettingsForm>,
) -> Response {
    let Some(sn) = resolve(&state, &id).await else {
        return unknown(&id);
    };
    state.watchers.touch(&sn);

    let label = form.label.trim();
    if label.is_empty() {
        return bad_request("the label cannot be empty (it defaults to the serial number)");
    }
    if label.len() > 64 {
        return bad_request("the label is over 64 bytes");
    }
    match queries::set_device_label(&state.db, &sn, label).await {
        Ok(true) => {
            tracing::info!(sn = %sn, label, "device label set");
            Json(json!({"label": label})).into_response()
        }
        Ok(false) => unknown(&id),
        Err(error) => internal(error),
    }
}

/// `GET /api/commands` — the catalog's raw names, so page and server agree.
pub async fn commands() -> Response {
    let names: Vec<&'static str> = CATALOG.iter().map(|(name, _, _)| *name).collect();
    Json(json!({"commands": names})).into_response()
}

#[derive(Deserialize)]
pub struct ConfigForm {
    /// The wire `infoType` the setting rides: `21022` suction, `21024` config.
    info: i64,
    /// The wire `data.cmd` name, e.g. `setVolume` or `strong`.
    cmd: String,
    /// The wire `data.value`, where the command takes one.
    value: Option<i64>,
}

/// `POST /api/robot/{id}/config` — one robot-side setting, queued as `21022` or
/// `21024` (doc/PLAN.md §20.7).
///
/// Deliberately an allowlist rather than a general "send anything" tunnel: the
/// page only needs a handful of writes, and the console's `send` verb remains the
/// operator path for experiments. The robot's next status push is the read-back;
/// there is no reply worth waiting for.
pub async fn config(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(form): Json<ConfigForm>,
) -> Response {
    let Some(sn) = resolve(&state, &id).await else {
        return unknown(&id);
    };
    state.watchers.touch(&sn);

    let payload = match (form.info, form.cmd.as_str(), form.value) {
        (info_type::CONFIG, "setVolume", Some(value)) if (0..=10).contains(&value) => {
            json!({"cmd": "setVolume", "value": value})
        }
        (info_type::CONFIG, "setledswitch", Some(value)) if value == 0 || value == 1 => {
            json!({"cmd": "setledswitch", "value": value})
        }
        // Which end of 1..4 is "most water" is still unverified (FUNC_COMMANDS §4.3);
        // the page labels 1 low and 4 high as a working assumption.
        (info_type::CONFIG, "setWaterPump", Some(value)) if (1..=4).contains(&value) => {
            json!({"cmd": "setWaterPump", "value": value})
        }
        (info_type::SUCTION, "quiet" | "auto" | "strong" | "max", None) => {
            json!({"cmd": form.cmd.clone()})
        }
        _ => {
            return bad_request(
                "unsupported setting: expected 21024 setVolume 0-10 / setledswitch 0-1 / \
                 setWaterPump 1-4, or 21022 quiet|auto|strong|max",
            );
        }
    };
    match queries::insert_command(&state.db, &sn, form.info, &payload.to_string(), 0).await {
        Ok(command_id) => Json(json!({"queued": command_id})).into_response(),
        Err(error) => internal(error),
    }
}

/// `GET /api/robot/{id}/goods` — the newest `21015` reply, cached like zones.
pub async fn goods(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Some(sn) = resolve(&state, &id).await else {
        return unknown(&id);
    };
    state.watchers.touch(&sn);

    let stored = match queries::load_goods(&state.db, &sn).await {
        Ok(stored) => stored,
        Err(error) => return internal(error),
    };
    let (read_ms, payload) = stored.unwrap_or((None, None));
    let parts: Value = payload
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null);
    Json(json!({"read_ms": read_ms, "parts": parts})).into_response()
}

/// `POST /api/robot/{id}/goods/refresh` — ask the robot for its consumables
/// (`21015`). The reply lands in the cache through `cleanPack/response`.
pub async fn goods_refresh(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Some(sn) = resolve(&state, &id).await else {
        return unknown(&id);
    };
    state.watchers.touch(&sn);
    if !state.registry.is_online(&sn) {
        return conflict("device is offline");
    }
    match queries::insert_command(&state.db, &sn, info_type::GOODS_READ, "{}", 0).await {
        Ok(command_id) => Json(json!({"queued": command_id})).into_response(),
        Err(error) => internal(error),
    }
}

/// `POST /api/robot/{id}/goods/reset` — zero one consumable counter (`21016`).
///
/// `21016` always answers `"ok"`, so the page re-reads with `21015` afterwards
/// (FUNC_STATUS §7.2); `motor` and `dustBox` cannot be reset and are refused here,
/// while `mop` is accepted even though `21015` never echoes it back.
pub async fn goods_reset(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let Some(sn) = resolve(&state, &id).await else {
        return unknown(&id);
    };
    state.watchers.touch(&sn);
    if !state.registry.is_online(&sn) {
        return conflict("device is offline");
    }
    let Some(part) = body.get("part").and_then(Value::as_str) else {
        return bad_request("missing part: the consumable to reset");
    };
    if ![
        "battery",
        "filter",
        "mainBrush",
        "sideBrush",
        "sensors",
        "mop",
    ]
    .contains(&part)
    {
        return bad_request("unknown consumable");
    }
    let mut object = serde_json::Map::new();
    object.insert(part.to_string(), json!(0));
    let payload = Value::Object(object).to_string();
    match queries::insert_command(&state.db, &sn, info_type::GOODS_WRITE, &payload, 0).await {
        Ok(command_id) => Json(json!({"queued": command_id, "part": part})).into_response(),
        Err(error) => internal(error),
    }
}

/// `GET /api/robot/{id}/device-info` — the newest `21019` reply (Wi-Fi addresses,
/// signal, `compileVer`/`mcuVer`), cached like zones and consumables.
pub async fn device_info(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Some(sn) = resolve(&state, &id).await else {
        return unknown(&id);
    };
    state.watchers.touch(&sn);

    let stored = match queries::load_device_info(&state.db, &sn).await {
        Ok(stored) => stored,
        Err(error) => return internal(error),
    };
    let (read_ms, payload) = stored.unwrap_or((None, None));
    let parts: Value = payload
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null);
    Json(json!({"read_ms": read_ms, "parts": parts})).into_response()
}

/// `POST /api/robot/{id}/device-info/refresh` — ask for a fresh `21019`.
pub async fn device_info_refresh(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let Some(sn) = resolve(&state, &id).await else {
        return unknown(&id);
    };
    state.watchers.touch(&sn);
    if !state.registry.is_online(&sn) {
        return conflict("device is offline");
    }
    match queries::insert_command(&state.db, &sn, info_type::NETWORK_INFO, "{}", 0).await {
        Ok(command_id) => Json(json!({"queued": command_id})).into_response(),
        Err(error) => internal(error),
    }
}

/// `POST /api/robot/{id}/reboot` — the maintenance `21024 reboot` (COMMANDS.md).
///
/// Deliberately online-only: a reboot queued while the robot is away would fire
/// minutes later when it reconnects, which is exactly the kind of surprise the
/// generic queue exists to avoid for unattended actions.
pub async fn reboot(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Some(sn) = resolve(&state, &id).await else {
        return unknown(&id);
    };
    state.watchers.touch(&sn);
    if !state.registry.is_online(&sn) {
        return conflict("device is offline");
    }
    match queries::insert_command(&state.db, &sn, info_type::CONFIG, "{\"cmd\":\"reboot\"}", 0)
        .await
    {
        Ok(command_id) => Json(json!({"queued": command_id})).into_response(),
        Err(error) => internal(error),
    }
}

/// `POST /api/robot/{id}/map/delete` — `21024 delCurMap` (FUNC_MAP §8.1).
///
/// Destructive: blocked while a clean is running, like zone edits, so a map delete
/// cannot race the robot's own map writes mid-job.
pub async fn map_delete(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Some(sn) = resolve(&state, &id).await else {
        return unknown(&id);
    };
    state.watchers.touch(&sn);
    if !state.registry.is_online(&sn) {
        return conflict("device is offline");
    }
    if cleaning_in_progress(&state, &sn).await {
        return conflict("a clean is running; the map cannot be deleted");
    }
    match queries::insert_command(
        &state.db,
        &sn,
        info_type::CONFIG,
        "{\"cmd\":\"delCurMap\"}",
        0,
    )
    .await
    {
        Ok(command_id) => Json(json!({"queued": command_id})).into_response(),
        Err(error) => internal(error),
    }
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

/// `GET /api/robot/{id}/zones` — the newest known `AreaSetting`.
pub async fn zones(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Some(sn) = resolve(&state, &id).await else {
        return unknown(&id);
    };
    state.watchers.touch(&sn);

    let stored = match queries::load_area_settings(&state.db, &sn).await {
        Ok(stored) => stored,
        Err(error) => return internal(error),
    };
    let (version, read_ms, payload) = match stored {
        Some(stored) => (stored.version, stored.read_ms, stored.payload),
        None => (0, None, None),
    };
    let parsed: Option<Value> = payload.and_then(|text| serde_json::from_str(&text).ok());
    let zones = parsed.as_ref().map(zones_from_payload).unwrap_or_default();
    let map_id = parsed
        .as_ref()
        .and_then(|value| value.get("mapId"))
        .cloned()
        .unwrap_or(Value::Null);

    let backup = match queries::load_map_backup_meta(&state.db, &sn).await {
        Ok(Some((received_ms, record_name, md5, size))) => json!({
            "received_ms": received_ms,
            "record": record_name,
            "md5": md5,
            "size": size,
        }),
        Ok(None) => Value::Null,
        Err(error) => return internal(error),
    };

    Json(json!({
        "version": version,
        "read_ms": read_ms,
        "map_id": map_id,
        "zones": zones,
        "backup": backup,
        "cleaning": cleaning_in_progress(&state, &sn).await,
        "online": state.registry.is_online(&sn),
    }))
    .into_response()
}

/// `GET /api/robot/{id}/backup` — the staged map backup as an `NBMP` export file.
pub async fn backup_export(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Some(sn) = resolve(&state, &id).await else {
        return unknown(&id);
    };
    state.watchers.touch(&sn);

    let stored = match queries::load_map_backup(&state.db, &sn).await {
        Ok(Some(stored)) => stored,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "no map backup has been received for this robot yet"})),
            )
                .into_response();
        }
        Err(error) => return internal(error),
    };
    let (_, blob) = stored;
    let label = queries::device_label(&state.db, &sn)
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| sn.clone());
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let filename = format!("{}-{stamp}.nbmap", sanitise(&label));
    let bytes = backup::encode(&sn, &blob);

    (
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}\""),
            ),
        ],
        bytes,
    )
        .into_response()
}

/// `POST /api/robot/{id}/backup` — import an `NBMP` export and restore it: stage the
/// blob, hand the robot a URL to it and queue the `21025` (doc/PLAN.md §20.6).
pub async fn backup_import(
    State(state): State<AppState>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let Some(sn) = resolve(&state, &id).await else {
        return unknown(&id);
    };
    state.watchers.touch(&sn);

    // The robot fetches `downUrl` itself (no headers, no redirects — BACKUP_MAP §D1),
    // so the URL has to be absolute. The browser's Host header is our address as the
    // operator reaches us, which is what the robot on the same LAN needs.
    let host = match robot_facing_host(
        request
            .headers()
            .get(header::HOST)
            .and_then(|value| value.to_str().ok()),
    ) {
        Ok(host) => host,
        Err(message) => return bad_request(&message),
    };

    let body = axum::body::to_bytes(request.into_body(), 16 * 1024 * 1024)
        .await
        .unwrap_or_default();
    let export = match backup::decode(&body) {
        Ok(export) => export,
        Err(message) => return bad_request(&message),
    };
    if export.sn != sn {
        return bad_request(&format!(
            "this export was taken from {}; it cannot be restored to {}",
            export.sn, sn
        ));
    }

    let md5 = backup::md5_hex(&export.blob);
    let token = session::random_hex(16);
    if let Err(error) =
        queries::upsert_map_backup(&state.db, &sn, None, &md5, &token, &export.blob).await
    {
        return internal(error);
    }

    let down_url = format!("http://{host}/backup/{sn}/{token}");
    let payload = json!({"downUrl": down_url, "md5": md5}).to_string();
    let command_id =
        match queries::insert_command(&state.db, &sn, info_type::BACKUP_RESTORE, &payload, 0).await
        {
            Ok(command_id) => command_id,
            Err(error) => return internal(error),
        };
    tracing::info!(sn = %sn, command_id, bytes = export.blob.len(), %down_url, "map restore queued");
    Json(json!({
        "queued": command_id,
        "down_url": down_url,
        "md5": md5,
        "size": export.blob.len(),
    }))
    .into_response()
}

/// `GET /backup/{sn}/{token}` — the exact bytes the robot downloads for a `21025`
/// restore. No cookies (the robot sends none), no `Content-Encoding`, no
/// transformation: the md5 we handed the robot is of these bytes.
pub async fn backup_blob(
    State(state): State<AppState>,
    Path((sn, token)): Path<(String, String)>,
) -> Response {
    match queries::load_map_backup_by_token(&state.db, &sn, &token).await {
        Ok(Some(blob)) => (
            [
                (header::CONTENT_TYPE, "application/octet-stream".to_string()),
                (header::CACHE_CONTROL, "no-store".to_string()),
            ],
            blob,
        )
            .into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "no such backup"})),
        )
            .into_response(),
        Err(error) => internal(error),
    }
}

/// Keep an export filename to characters every filesystem accepts.
fn sanitise(label: &str) -> String {
    let cleaned: String = label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches('_').to_string();
    if cleaned.is_empty() {
        "robot".to_string()
    } else {
        cleaned
    }
}

/// The address the robot is told to fetch a backup from. The browser's `Host` header
/// is our address as the operator reaches us — and a UI opened through `localhost`
/// would hand the robot a URL it cannot reach (found live on 2026-10-10, where it
/// surfaced only as a `code:-3` "down fail"): refuse loopback hosts with guidance
/// instead of queueing a restore that cannot work.
fn robot_facing_host(host: Option<&str>) -> std::result::Result<String, String> {
    let Some(host) = host.map(str::trim).filter(|host| !host.is_empty()) else {
        return Err(
            "the request carried no Host header, so the robot cannot be told where to fetch the map"
                .to_string(),
        );
    };
    let name = if let Some(rest) = host.strip_prefix('[') {
        rest.split(']').next().unwrap_or(rest)
    } else {
        host.split(':').next().unwrap_or(host)
    }
    .to_ascii_lowercase();
    if name == "localhost"
        || name.ends_with(".localhost")
        || name == "::1"
        || name.starts_with("127.")
    {
        return Err(format!(
            "the robot cannot fetch a backup from {host}; open the web UI through the \
             server's LAN address and try the import again"
        ));
    }
    Ok(host.to_string())
}

/// `POST /api/robot/{id}/zones/refresh` — ask the robot for its list (`21004`). The
/// reply lands in the cache through `cleanPack/response`, like a path chunk.
pub async fn zones_refresh(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Some(sn) = resolve(&state, &id).await else {
        return unknown(&id);
    };
    state.watchers.touch(&sn);
    if !state.registry.is_online(&sn) {
        return conflict("device is offline");
    }
    match queries::insert_command(&state.db, &sn, info_type::ZONES_READ, "{}", 0).await {
        Ok(command_id) => Json(json!({"queued": command_id})).into_response(),
        Err(error) => internal(error),
    }
}

/// `PUT /api/robot/{id}/zones` — replace the whole list (`21003`).
///
/// Two deliberate refusals, both operator decisions from 2026-10-08:
/// * a clean in progress blocks edits — the robot's behaviour on mid-job edits is
///   untested, and an edit made against a list that is about to change could be
///   written back stale;
/// * the client must echo the `version` it read; a newer version means someone else
///   (or our own last write) touched the list, so we 409 instead of stomping it.
pub async fn zones_write(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let Some(sn) = resolve(&state, &id).await else {
        return unknown(&id);
    };
    state.watchers.touch(&sn);
    if !state.registry.is_online(&sn) {
        return conflict("device is offline");
    }
    if cleaning_in_progress(&state, &sn).await {
        return conflict("a clean is running; zone edits are blocked");
    }

    let stored = match queries::load_area_settings(&state.db, &sn).await {
        Ok(Some(stored)) => stored,
        Ok(None) => return conflict("no zone list has been read yet; refresh first"),
        Err(error) => return internal(error),
    };
    let client_version = body.get("version").and_then(Value::as_i64);
    if client_version != Some(stored.version) {
        return conflict(&format!(
            "zones changed since version {}; refresh and try again",
            client_version.unwrap_or(0)
        ));
    }
    let Some(value) = body.get("value") else {
        return bad_request("missing value: the list of regions");
    };
    let zones = match validate_zones(value) {
        Ok(zones) => zones,
        Err(message) => return bad_request(&message),
    };

    // The firmware stores `mapId` but never checks it (FUNC_MAP §5.1); keep whatever
    // the robot itself reported so the list round-trips unchanged apart from the edit.
    let map_id = stored
        .payload
        .as_deref()
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
        .and_then(|value| value.get("mapId").cloned())
        .unwrap_or(Value::Null);
    let payload = json!({"mapId": map_id, "value": zones}).to_string();

    let command_id =
        match queries::insert_command(&state.db, &sn, info_type::ZONES_WRITE, &payload, 0).await {
            Ok(command_id) => command_id,
            Err(error) => return internal(error),
        };
    let version = match queries::record_zone_write(&state.db, &sn, &payload).await {
        Ok(version) => version,
        Err(error) => return internal(error),
    };
    tracing::info!(sn = %sn, command_id, version, "zone list written");
    Json(json!({"queued": command_id, "version": version})).into_response()
}

/// `POST /api/robot/{id}/zones/clean` — clean the picked stored regions (`21023`).
///
/// The sequence is fixed by ZONE_CLEAN.md (2026-10-09): `21023` only writes the
/// CleanArea selection and never starts a job while idle, and `smartClean` would
/// synthesize a whole-map total region, wiping that selection — the earlier whole-home
/// runs. The start is `21005 {"mode":"appointClean"}`, which runs from the current
/// region set and works docked or idle. Stored forbid zones are folded in as `-3` so
/// no-go areas stay active, matching the robot's own schedule default of `[-1,-3]`.
pub async fn zones_clean(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let Some(sn) = resolve(&state, &id).await else {
        return unknown(&id);
    };
    state.watchers.touch(&sn);
    if !state.registry.is_online(&sn) {
        return conflict("device is offline");
    }
    if cleaning_in_progress(&state, &sn).await {
        return conflict("a clean is already running");
    }
    let ids: Vec<i64> = body
        .get("ids")
        .and_then(Value::as_array)
        .map(|ids| ids.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default();
    if ids.is_empty() {
        return bad_request("missing ids: the stored regions to clean");
    }
    let mut clean_id = vec![-3];
    clean_id.extend(ids);
    let selection = json!({"cleanId": clean_id}).to_string();
    let selection_command =
        match queries::insert_command(&state.db, &sn, info_type::ZONE_CLEAN, &selection, 0).await {
            Ok(command_id) => command_id,
            Err(error) => return internal(error),
        };
    // Enqueued second, so it is pushed second (the poller keeps insertion order).
    let start_command = match queries::insert_command(
        &state.db,
        &sn,
        info_type::CLEAN,
        "{\"mode\":\"appointClean\"}",
        0,
    )
    .await
    {
        Ok(command_id) => command_id,
        Err(error) => return internal(error),
    };
    tracing::info!(
        sn = %sn,
        selection_command,
        start_command,
        zones = clean_id.len() - 1,
        "zone clean queued (21023 + appointClean)"
    );
    Json(json!({"queued": selection_command, "start": start_command})).into_response()
}

/// The square the firmware itself builds for a spot clean (FUNC_MAP.md §4/§6.3:
/// 1.5 m, i.e. ±750 mm around the point).
const SPOT_SIDE_MM: i64 = 1_500;

/// `POST /api/robot/{id}/spot` — placed spot cleaning: clean around an arbitrary
/// world point (`21023` point region + `appointClean`).
///
/// The sequence is the same one `zones_clean` uses, with the target carried inline
/// instead of as stored ids: `21023` writes the CleanArea selection and
/// `21005 {"mode":"appointClean"}` starts from it (ZONE_CLEAN.md). A `mode:"point"`
/// region makes the robot plan the route itself and spot-clean at the point — live
/// 2026-10-12: `subMode:"point"`, it drove to the requested coordinates and cleaned
/// (FIELD_NOTES.md). Stored forbid zones ride along as `-3` so no-go areas stay
/// active.
pub async fn spot_clean(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let Some(sn) = resolve(&state, &id).await else {
        return unknown(&id);
    };
    state.watchers.touch(&sn);
    if !state.registry.is_online(&sn) {
        return conflict("device is offline");
    }
    if cleaning_in_progress(&state, &sn).await {
        return conflict("a clean is already running");
    }
    let (Some(x), Some(y)) = (
        body.get("x").and_then(Value::as_i64),
        body.get("y").and_then(Value::as_i64),
    ) else {
        return bad_request("missing x/y: the spot centre in millimetres");
    };

    // `active` is the firmware's cover mode: `normal` is the single-direction pass,
    // `depth` is the deep clean (FUNC_MAP.md §4). The UI's Deep checkbox picks it;
    // the default stays the plain spot clean.
    let cover = if body.get("deep").and_then(Value::as_bool).unwrap_or(false) {
        "depth"
    } else {
        "normal"
    };
    let half = SPOT_SIDE_MM / 2;
    let region = json!({
        "vertexs": [
            [x - half, y - half],
            [x + half, y - half],
            [x + half, y + half],
            [x - half, y + half],
        ],
        "active": cover,
        "mode": "point",
    });
    let selection = json!({"cleanId": [-3], "extraAreas": [region]}).to_string();
    let selection_command =
        match queries::insert_command(&state.db, &sn, info_type::ZONE_CLEAN, &selection, 0).await {
            Ok(command_id) => command_id,
            Err(error) => return internal(error),
        };
    // Enqueued second, so it is pushed second (the poller keeps insertion order).
    let start_command = match queries::insert_command(
        &state.db,
        &sn,
        info_type::CLEAN,
        "{\"mode\":\"appointClean\"}",
        0,
    )
    .await
    {
        Ok(command_id) => command_id,
        Err(error) => return internal(error),
    };
    tracing::info!(
        sn = %sn,
        selection_command,
        start_command,
        x,
        y,
        "spot clean queued (21023 point + appointClean)"
    );
    Json(json!({"queued": selection_command, "start": start_command})).into_response()
}

/// A clean in progress blocks zone edits and zone-clean starts (operator decision,
/// 2026-10-08). `FindChargerAndWash` counts as part of the job it interrupted.
async fn cleaning_in_progress(state: &AppState, sn: &str) -> bool {
    matches!(
        latest_mode(state, sn).await.as_deref(),
        Some("sweep") | Some("FindChargerAndWash") | Some("DustCenterWorking")
    )
}

/// The list out of either shape the robot might answer with: `{"mapId":…,"value":[…]}`,
/// or a bare array.
fn zones_from_payload(payload: &Value) -> Vec<Value> {
    if let Some(value) = payload.get("value").and_then(Value::as_array) {
        return value.clone();
    }
    payload.as_array().cloned().unwrap_or_default()
}

/// Validate an edited zone list before it reaches the firmware (doc/PLAN.md §20.5):
/// whole `[x, y]` integer vertices, at least three distinct ones, no collapsed
/// polygon. The string fields are **capped** to the robot's fixed buffers
/// (`name`/`tag` 31 UTF-8 bytes, `mode` 30): the firmware `strcpy`s them into 32-byte
/// stack buffers, so anything longer is a real overflow — the UI caps too, and this
/// is the backstop that must hold even for a hand-crafted API call. Returns the list
/// with consecutive duplicate vertices dropped.
fn validate_zones(value: &Value) -> std::result::Result<Vec<Value>, String> {
    let Some(regions) = value.as_array() else {
        return Err("value must be an array of regions".into());
    };
    let mut out = Vec::with_capacity(regions.len());
    for (index, region) in regions.iter().enumerate() {
        let Some(object) = region.as_object() else {
            return Err(format!("region {index} is not an object"));
        };
        let mut normalized = object.clone();
        for (key, limit) in [("name", 31), ("tag", 31), ("mode", 30)] {
            if let Some(text) = object.get(key).and_then(Value::as_str) {
                let capped = cap_bytes(text, limit);
                if capped.len() != text.len() {
                    tracing::warn!(
                        region = index,
                        field = key,
                        bytes = text.len(),
                        limit,
                        "zone field capped to the robot's buffer"
                    );
                    normalized.insert(key.to_string(), Value::String(capped.to_string()));
                }
            }
        }
        let Some(vertices) = object.get("vertexs").and_then(Value::as_array) else {
            return Err(format!("region {index}: missing vertexs"));
        };
        let mut points: Vec<[i64; 2]> = Vec::with_capacity(vertices.len());
        for vertex in vertices {
            let pair = vertex
                .as_array()
                .filter(|pair| pair.len() == 2)
                .ok_or_else(|| format!("region {index}: every vertex must be [x, y]"))?;
            let (Some(x), Some(y)) = (pair[0].as_i64(), pair[1].as_i64()) else {
                return Err(format!(
                    "region {index}: vertex coordinates must be integers (mm)"
                ));
            };
            if points.last() != Some(&[x, y]) {
                points.push([x, y]);
            }
        }
        if points.len() < 3 {
            return Err(format!(
                "region {index}: fewer than three distinct vertices"
            ));
        }
        if polygon_area(&points) == 0 {
            return Err(format!("region {index}: the polygon collapses to a line"));
        }
        // The robot snaps vertices onto its own 50 mm lattice, whose phase relative to
        // the map raster could not be observed from the wire (21004 echoes verbatim and
        // 20002 area[] stayed empty even with a saved zone). Two cells of thickness in
        // each axis means no half-cell offset in either direction can collapse a zone.
        let xs: Vec<i64> = points.iter().map(|[x, _]| *x).collect();
        let ys: Vec<i64> = points.iter().map(|[_, y]| *y).collect();
        let span_x = xs.iter().max().unwrap_or(&0) - xs.iter().min().unwrap_or(&0);
        let span_y = ys.iter().max().unwrap_or(&0) - ys.iter().min().unwrap_or(&0);
        if span_x < 100 || span_y < 100 {
            return Err(format!(
                "region {index}: {span_x}x{span_y} mm is thinner than two 50 mm cells"
            ));
        }
        normalized.insert(
            "vertexs".into(),
            Value::Array(points.iter().map(|[x, y]| json!([x, y])).collect()),
        );
        out.push(Value::Object(normalized));
    }
    Ok(out)
}

/// Twice the shoelace area of a polygon; zero means every vertex is collinear.
fn polygon_area(points: &[[i64; 2]]) -> i128 {
    let mut sum = 0i128;
    for index in 0..points.len() {
        let [x1, y1] = points[index];
        let [x2, y2] = points[(index + 1) % points.len()];
        sum += i128::from(x1) * i128::from(y2) - i128::from(x2) * i128::from(y1);
    }
    sum
}

/// The longest prefix of `text` that fits in `limit` UTF-8 bytes and still ends on a
/// character boundary — never split a multi-byte character to reach the limit.
fn cap_bytes(text: &str, limit: usize) -> &str {
    if text.len() <= limit {
        return text;
    }
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
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
    match handle.tx.send(registry::Outbound::frame(frame)).await {
        Ok(()) => {
            // Arm the watchdog for steering — unless this frame *is* the client
            // leaving manual mode; one-shot frames (mute, locate, spot clean) do
            // not arm it at all.
            if form.code == 4000 {
                state.controls.clear(&sn);
            } else if STEERING_CODES.contains(&form.code) {
                state.controls.touch(&sn);
            }
            Json(json!({"code": form.code})).into_response()
        }
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

/// The steering watchdog (doc/PLAN.md §20.4): a client that stops sending control
/// frames — tab closed, Wi-Fi died mid-hold — gets `4000` from the server instead of
/// leaving the robot in manual mode until its own 30 s timeout.
pub fn spawn_control_watchdog(state: AppState, mut shutdown: watch::Receiver<()>) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(CONTROL_WATCHDOG_TICK);
        loop {
            tokio::select! {
                _ = shutdown.changed() => break,
                _ = ticker.tick() => poll_control_watchdog_once(&state, CONTROL_WATCHDOG_MS).await,
            }
        }
    });
}

/// One watchdog pass. Public so tests can drive it without the timer.
pub async fn poll_control_watchdog_once(state: &AppState, stale_ms: i64) {
    for sn in state.controls.stale(stale_ms) {
        state.controls.clear(&sn);
        let Some(handle) = state.registry.get(&sn) else {
            continue; // offline: there is nothing left to stop
        };
        let frame = codec::envelope(0, codec::message(21020, json!({"ctrlCode": 4000}), None));
        match handle.tx.send(registry::Outbound::frame(frame)).await {
            Ok(()) => tracing::info!(
                sn = %sn,
                "steering watchdog: no control frames; sent 4000 to leave manual mode"
            ),
            Err(_) => tracing::debug!(sn = %sn, "steering watchdog: writer is gone"),
        }
    }
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
        // One fetch at a time. A request that has gone unacked for too long is
        // retired: a lost reply must not wedge every later poll (2026-10-12).
        match queries::active_command(&state.db, &sn, info_type::PATH).await {
            Ok(None) => {}
            Ok(Some((id, sent_ms, created_ms))) => {
                let since = now_ms() - sent_ms.unwrap_or(created_ms);
                if since < PATH_REQUEST_STALE_MS {
                    continue;
                }
                match queries::fail_command(&state.db, id, "path poll timed out; no ACK").await {
                    Ok(()) => tracing::warn!(
                        sn = %sn,
                        command_id = id,
                        stale_ms = since,
                        "path poller: retired an unacked 21011 request"
                    ),
                    Err(error) => {
                        tracing::error!(%error, sn = %sn, command_id = id,
                            "path poller: could not retire a stale request");
                        continue;
                    }
                }
            }
            Err(error) => {
                tracing::error!(%error, sn = %sn, "path poller: could not check the queue");
                continue;
            }
        }
        // A new job has its own path id: the old assembly's next index is meaningless
        // (and its at-end request is the one the robot failed to answer), so ask the
        // new path from its start.
        let current_path = match queries::latest_map_path_id(&state.db, &sn).await {
            Ok(path_id) => path_id,
            Err(error) => {
                tracing::error!(%error, sn = %sn, "path poller: could not read the map path id");
                continue;
            }
        };
        let start_pos = match queries::clean_path_summary(&state.db, &sn, None).await {
            Ok(Some((stored_id, points_json, _, _, _))) => {
                if current_path.is_some_and(|current| current != stored_id) {
                    0
                } else {
                    crate::path::Assembly::from_json(&points_json).next_index()
                }
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

fn conflict(message: &str) -> Response {
    (StatusCode::CONFLICT, Json(json!({"error": message}))).into_response()
}

fn bad_request(message: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({"error": message}))).into_response()
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

    #[test]
    fn control_frames_arm_the_watchdog_until_they_go_stale() {
        let controls = Controls::new();
        assert!(controls.stale(0).is_empty(), "nothing to watch yet");
        controls.touch("SN");
        assert!(
            controls.stale(60_000).is_empty(),
            "a fresh frame is not stale"
        );
        assert_eq!(controls.stale(0), vec!["SN".to_string()]);
        controls.touch_at("SN", now_ms() - 5_000);
        assert_eq!(
            controls.stale(CONTROL_WATCHDOG_MS),
            vec!["SN".to_string()],
            "five seconds of silence is past the watchdog"
        );
        controls.clear("SN");
        assert!(controls.stale(0).is_empty(), "cleared by an explicit 4000");
    }
}
