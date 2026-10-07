//! The channel-A endpoints, phase by phase (doc/PLAN.md §9).

pub mod register;
pub mod response;
pub mod sock_addr;
pub mod sync;
pub mod upload_events;
pub mod uploads;

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

/// `{"code":0,"message":"ok","data":…}` — the channel-A success envelope.
pub fn ok(data: Value) -> Response {
    envelope(0, "ok", data)
}

/// `code:102` — "your sid expired": the documented signal that makes the device
/// discard its session and re-run `register` (doc/PLAN.md §9.2). It is also how a
/// robot that paired against the phase-1 catch-all (empty session) picks up a real
/// session without being re-homed.
pub fn sid_expired() -> Response {
    envelope(102, "sid expired", json!({}))
}

/// `code:212` — auth failure, used only when `registration.accept_all` is off.
pub fn auth_failed() -> Response {
    envelope(212, "auth fail", json!({}))
}

fn envelope(code: i64, message: &str, data: Value) -> Response {
    (
        StatusCode::OK,
        Json(json!({"code": code, "message": message, "data": data})),
    )
        .into_response()
}

/// The OTA answer: no update, with the flag at the top level *and* under `data`,
/// because the two RE sources disagree and the plan says emit both (doc/PLAN.md §9.3).
pub fn no_update() -> Response {
    (
        StatusCode::OK,
        Json(json!({
            "code": 0,
            "message": "ok",
            "hasUpdateFile": 0,
            "data": {"hasUpdateFile": 0},
        })),
    )
        .into_response()
}
