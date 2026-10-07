//! `POST /cleanPack/sync` and the version check — device attributes, OTA off.

use std::collections::HashMap;

use axum::extract::{Query, Request, State};
use axum::response::Response;

use crate::AppState;
use crate::channel_a::form::Form;
use crate::channel_a::handlers;
use crate::db::queries;
use crate::wire::Recorded;

pub async fn sync(State(state): State<AppState>, request: Request) -> Response {
    let recorded = request.extensions().get::<Recorded>().cloned();
    let trace = recorded
        .as_ref()
        .map(|r| r.reference.as_str())
        .unwrap_or("-");
    let body = axum::body::to_bytes(request.into_body(), crate::channel_a::MAX_BODY_BYTES)
        .await
        .unwrap_or_default();
    let form = Form::parse(&body);

    let Some(sn) = form
        .get("sn")
        .filter(|sn| !sn.is_empty())
        .map(str::to_string)
    else {
        tracing::warn!(trace, "sync without an sn; answering no-update anyway");
        return handlers::no_update();
    };

    let result = queries::upsert_device_sync(
        &state.db,
        &sn,
        form.get_i64("companyId"),
        form.get("mcuVer"),
        form.get("version"),
        form.get_i64("versionCode"),
        form.get("gitSha"),
        form.get("cloud"),
    )
    .await;
    if let Err(error) = result {
        tracing::error!(%error, sn = %sn, trace, "could not record the sync attributes");
    } else {
        tracing::info!(sn = %sn, trace, "sync recorded");
    }

    handlers::no_update()
}

/// `GET /?version=1&sn=…&companyId=…` — the device's version/OTA check.
pub async fn version_check(
    State(state): State<AppState>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    if let Some(sn) = query.get("sn").filter(|sn| !sn.is_empty())
        && let Err(error) = queries::upsert_device_seen(&state.db, sn, None).await
    {
        tracing::error!(%error, sn = %sn, "could not touch the device on the version check");
    }
    tracing::debug!(query = ?query, "version check; answering no-update");
    handlers::no_update()
}
