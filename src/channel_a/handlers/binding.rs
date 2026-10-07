//! `POST /cleanPack/binding|unbinding` — the preBind state machine
//! (doc/PLAN.md §9, PROTOCOL §A "Gap 2").
//!
//! The robot's BindUser flow fires the preBind the moment its channel-B session is
//! alive and reads plain `code:0` as "directly bind success" — that is what ended
//! the pairing indication on the bench unit. We record the transition and answer
//! idempotently: a retry caused by a lost reply must not fail the bind, and an
//! unbind is just the other direction. Nothing is gated on the state — maps and
//! commands keep flowing either way; this is bookkeeping and observability.

use axum::extract::{Request, State};
use axum::response::Response;
use serde_json::json;

use crate::AppState;
use crate::channel_a::form::Form;
use crate::channel_a::handlers;
use crate::db::queries;
use crate::wire::Recorded;

pub async fn binding(State(state): State<AppState>, request: Request) -> Response {
    transition(state, request, "/cleanPack/binding", true).await
}

pub async fn unbinding(State(state): State<AppState>, request: Request) -> Response {
    transition(state, request, "/cleanPack/unbinding", false).await
}

async fn transition(
    state: AppState,
    request: Request,
    endpoint: &'static str,
    bind: bool,
) -> Response {
    let recorded = request.extensions().get::<Recorded>().cloned();
    let trace = recorded
        .as_ref()
        .map(|r| r.reference.as_str())
        .unwrap_or("-");
    let body = axum::body::to_bytes(request.into_body(), crate::channel_a::MAX_BODY_BYTES)
        .await
        .unwrap_or_default();
    let form = Form::parse(&body);

    let sn = form.get("sn").unwrap_or_default().to_string();
    let user_id = form.get("userId").map(str::to_string);
    // `ts` repeats the SN on this unit — a vendor quirk stored verbatim, not validated.
    let device_ts = form.get("ts");

    if sn.is_empty() {
        tracing::warn!(
            endpoint,
            trace,
            "bind request without an sn; answering code:0"
        );
        return handlers::ok(json!({}));
    }

    // The preBind can arrive before the device ever registered: the pairing chain is
    // binding → register → sync (PAIRING_LOG_ANALYSIS §8).
    if let Err(error) = queries::upsert_device_seen(&state.db, &sn, None).await {
        tracing::error!(%error, %sn, "could not adopt the device on its bind");
    }

    let previous = queries::bind_state(&state.db, &sn).await.ok().flatten();
    if let Err(error) = queries::record_bind(&state.db, &sn, bind, user_id.as_deref()).await {
        // Answer code:0 regardless: a database hiccup must not leave the robot's
        // BindUser retrying until it times out (doc/PLAN.md §17).
        tracing::error!(%error, %sn, endpoint, "could not record the bind transition");
    }
    tracing::info!(
        sn = %sn,
        endpoint,
        user = user_id.as_deref().unwrap_or("-"),
        previous = previous.as_deref().unwrap_or("-"),
        trace,
        "bind transition"
    );

    let fields = json!({"sn": &sn, "ts": device_ts, "userId": &user_id});
    let result = queries::insert_event(
        &state.db,
        &sn,
        "A",
        Some(endpoint),
        None,
        None,
        None,
        user_id.as_deref(),
        device_ts,
        &fields.to_string(),
        recorded.as_ref().map(|r| r.reference.as_str()),
    )
    .await;
    if let Err(error) = result {
        tracing::error!(%error, %sn, "could not persist the bind request");
    }

    handlers::ok(json!({}))
}
