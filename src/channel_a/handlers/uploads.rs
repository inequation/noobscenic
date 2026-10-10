//! `POST /cleanPack/uploadLogs|uploadStats|uploadSingle` — stored verbatim, except
//! `uploadSingle`'s clean-record multipart, whose `backupMap` part is staged as the
//! one-slot map backup (doc/PLAN.md §20.6) and whose parts are kept parsed and readable.

use axum::extract::{Request, State};
use axum::http::header;
use axum::response::Response;
use serde_json::{Map, Value, json};

use crate::AppState;
use crate::channel_a::form::Form;
use crate::channel_a::handlers;
use crate::channel_a::multipart::{self, Part};
use crate::db::queries;
use crate::wire::Recorded;

pub async fn upload_raw(State(state): State<AppState>, request: Request) -> Response {
    let endpoint = request.uri().path().to_string();
    let recorded = request.extensions().get::<Recorded>().cloned();
    let trace = recorded
        .as_ref()
        .map(|r| r.reference.as_str())
        .unwrap_or("-");
    let content_type = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let body = axum::body::to_bytes(request.into_body(), crate::channel_a::MAX_BODY_BYTES)
        .await
        .unwrap_or_default();

    // The clean-record upload is real multipart with a binary `.bkmap` part; the
    // lenient form parser turns that into nonsense (BACKUP_MAP.md §A5).
    if let Some(parts) = multipart::parse(&content_type, &body) {
        return store_multipart(&state, &endpoint, &parts, recorded.as_ref(), body.len()).await;
    }

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

/// Parse the robot's multipart clean-record upload: keep a readable field map, and
/// stage the `backupMap` blob when it carries one (the 4-part form without it is
/// legal — BACKUP_MAP.md §A5).
async fn store_multipart(
    state: &AppState,
    endpoint: &str,
    parts: &[Part],
    recorded: Option<&Recorded>,
    body_len: usize,
) -> Response {
    let trace = recorded.map(|r| r.reference.as_str()).unwrap_or("-");
    let find = |name: &str| parts.iter().find(|part| part.name == name);

    let mut fields = Map::new();
    for part in parts {
        let value = match &part.filename {
            Some(filename) => json!({
                "filename": filename,
                "bytes": part.content.len(),
            }),
            None => Value::String(
                part.text()
                    .unwrap_or("(not UTF-8)")
                    .chars()
                    .take(4096)
                    .collect(),
            ),
        };
        fields.insert(part.name.clone(), value);
    }
    let fields_json = Value::Object(fields).to_string();
    let sn = find("sn").and_then(Part::text);
    let trace_ref = recorded.map(|r| r.reference.as_str());

    if let Err(error) =
        queries::insert_upload_raw(&state.db, sn, endpoint, &fields_json, trace_ref).await
    {
        tracing::error!(%error, %endpoint, "could not store the raw upload");
    }

    if let Some(backup) = find("backupMap") {
        let computed = crate::backup::md5_hex(&backup.content);
        let declared = find("backupMapMd5").and_then(Part::text);
        if declared != Some(computed.as_str()) {
            // The robot deletes its own copy once we answer success, so a body that
            // does not match the md5 the robot computed must be refused, not stored:
            // it retries on the next ~10 s poll with the intact files (BACKUP_MAP §A5).
            tracing::warn!(
                declared = declared.unwrap_or("-"),
                computed = %computed,
                bytes = backup.content.len(),
                trace,
                "backupMap md5 does not match backupMapMd5; asking the robot to retry"
            );
            return handlers::upload_failed();
        }
        let record_name = find("cleanFile")
            .and_then(|part| part.filename.clone())
            .or_else(|| backup.filename.clone());
        let token = crate::session::random_hex(16);
        match queries::upsert_map_backup(
            &state.db,
            sn.unwrap_or("-"),
            record_name.as_deref(),
            &computed,
            &token,
            &backup.content,
        )
        .await
        {
            Ok(()) => tracing::info!(
                sn = sn.unwrap_or("-"),
                record = record_name.as_deref().unwrap_or("-"),
                bytes = backup.content.len(),
                md5 = %computed,
                trace,
                "map backup staged"
            ),
            Err(error) => tracing::error!(%error, "could not stage the map backup"),
        }
    } else {
        tracing::info!(%endpoint, bytes = body_len, trace, "multipart upload stored (no backup)");
    }

    handlers::ok(json!({}))
}
