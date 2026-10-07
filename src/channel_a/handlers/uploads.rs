//! `POST /cleanPack/uploadLogs|uploadStats|uploadSingle` — stored verbatim; the
//! payload shapes are not pinned by the RE docs, so nothing is interpreted.

use axum::extract::{Request, State};
use axum::response::Response;
use serde_json::{Map, Value, json};

use crate::AppState;
use crate::channel_a::form::Form;
use crate::channel_a::handlers;
use crate::db::queries;
use crate::wire::Recorded;

pub async fn upload_raw(State(state): State<AppState>, request: Request) -> Response {
    let endpoint = request.uri().path().to_string();
    let recorded = request.extensions().get::<Recorded>().cloned();
    let trace = recorded
        .as_ref()
        .map(|r| r.reference.as_str())
        .unwrap_or("-");
    let body = axum::body::to_bytes(request.into_body(), crate::channel_a::MAX_BODY_BYTES)
        .await
        .unwrap_or_default();
    let form = Form::parse(&body);

    let mut fields = Map::new();
    for (key, value) in form.entries() {
        fields.insert(key.clone(), Value::String(value.clone()));
    }
    if let Some(data) = form
        .data
        .clone()
        .or_else(|| form.data_raw.clone().map(Value::String))
    {
        fields.insert("data".to_string(), data);
    }
    let fields_json = Value::Object(fields).to_string();
    let sn = form.get("sn");

    let result = queries::insert_upload_raw(
        &state.db,
        sn,
        &endpoint,
        &fields_json,
        recorded.as_ref().map(|r| r.reference.as_str()),
    )
    .await;
    match result {
        Ok(()) => tracing::info!(
            %endpoint,
            sn = sn.unwrap_or("-"),
            bytes = body.len(),
            trace,
            "raw upload stored"
        ),
        Err(error) => tracing::error!(%error, %endpoint, "could not store the raw upload"),
    }

    handlers::ok(json!({}))
}
