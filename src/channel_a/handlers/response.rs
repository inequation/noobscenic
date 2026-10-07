//! `POST /cleanPack/response` — command results (21011 paths today, more later).

use axum::extract::{Request, State};
use axum::response::Response;
use serde_json::{Value, json};

use crate::AppState;
use crate::channel_a::form::Form;
use crate::channel_a::handlers;
use crate::proto::{self, dispatch::Incoming};
use crate::wire::Recorded;

pub async fn response(State(state): State<AppState>, request: Request) -> Response {
    let recorded = request.extensions().get::<Recorded>().cloned();
    let trace = recorded
        .as_ref()
        .map(|r| r.reference.as_str())
        .unwrap_or("-");
    let body = axum::body::to_bytes(request.into_body(), crate::channel_a::MAX_BODY_BYTES)
        .await
        .unwrap_or_default();
    let form = Form::parse(&body);

    let Some(message) = form.data.as_ref() else {
        tracing::warn!(
            bytes = body.len(),
            trace,
            "response without a parseable data field"
        );
        return handlers::ok(json!({}));
    };

    let sn = form.get("sn").unwrap_or_default().to_string();
    let info_type = message
        .get("infoType")
        .and_then(Value::as_i64)
        .or_else(|| form.get_i64("infoType"));
    let payload = message.get("data").cloned().unwrap_or(Value::Null);
    if let Some(info_type) = info_type {
        // Best-effort ACK correlation (doc/PLAN.md §10.4): there is no correlation
        // id, so the newest `sent` command for this `(sn, infoType)` within the
        // command TTL window takes the credit; a miss invents nothing.
        let window_ms = state.config.gateway.command_ttl_secs as i64 * 1000;
        // Keep the whole response message, not just its `data` — the operator wants
        // the `message: "ok"` line too.
        let ack_payload = serde_json::to_string(message).unwrap_or_else(|_| "null".to_string());
        match crate::db::queries::ack_command(&state.db, &sn, info_type, &ack_payload, window_ms)
            .await
        {
            Ok(Some(id)) => {
                tracing::info!(sn = %sn, info_type, command_id = id, trace, "command acknowledged")
            }
            Ok(None) => {
                tracing::debug!(sn = %sn, info_type, trace, "response matched no sent command")
            }
            Err(error) => tracing::error!(%error, sn = %sn, "ACK correlation failed"),
        }
    }
    proto::dispatch(
        &state,
        Incoming {
            sn: &sn,
            channel: "A",
            endpoint: Some("/cleanPack/response"),
            trace_ref: recorded.as_ref().map(|r| r.reference.as_str()),
            info_type,
            data: &payload,
            event: None,
            task_id: form.get("taskid"),
            device_ts: form.get("ts"),
            user_id: form.get("userId"),
        },
    )
    .await;

    handlers::ok(json!({}))
}
