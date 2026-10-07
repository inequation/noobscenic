//! The one place that understands payloads — doc/PLAN.md §11.
//!
//! Channel A `uploadEvents` carries `{"infoType":N,"data":{…}}` inside its form body;
//! channel A `response` carries command results the same way; channel B frames are the
//! same object. All three land here, and everything is persisted with the `trace_ref`
//! of the bytes that produced it.

use serde_json::Value;

use base64::Engine as _;

use crate::db::queries;
use crate::wire::Recorded;
use crate::{AppState, map, path};

use super::info_type;

/// One incoming payload, from either channel.
pub struct Incoming<'a> {
    pub sn: &'a str,
    pub channel: &'static str,
    /// Channel-A endpoint path, for the `events` row.
    pub endpoint: Option<&'a str>,
    pub trace_ref: Option<&'a str>,
    pub info_type: Option<i64>,
    /// The message's `data` member — what the handlers actually process.
    pub data: &'a Value,
    /// `uploadEvents`' extra fields, when the template carries them.
    pub event: Option<i64>,
    pub task_id: Option<&'a str>,
    pub device_ts: Option<&'a str>,
    pub user_id: Option<&'a str>,
}

impl<'a> Incoming<'a> {
    pub fn new(sn: &'a str, channel: &'static str, data: &'a Value) -> Incoming<'a> {
        Incoming {
            sn,
            channel,
            endpoint: None,
            trace_ref: None,
            info_type: None,
            data,
            event: None,
            task_id: None,
            device_ts: None,
            user_id: None,
        }
    }

    pub fn trace(mut self, recorded: Option<&'a Recorded>) -> Self {
        self.trace_ref = recorded.map(|recorded| recorded.reference.as_str());
        self
    }

    pub fn endpoint(mut self, endpoint: &'a str) -> Self {
        self.endpoint = Some(endpoint);
        self
    }
}

pub async fn dispatch(state: &AppState, incoming: Incoming<'_>) {
    let trace = incoming.trace_ref.unwrap_or("-");
    match incoming.info_type {
        Some(info_type::MAP) => store_map(state, &incoming).await,
        Some(info_type::PATH) => store_path(state, &incoming).await,
        Some(info_type::STATUS) => {
            tracing::debug!(sn = %incoming.sn, trace, "status push");
            store_event(state, &incoming).await;
        }
        Some(info_type::EVENT) | Some(info_type::RECORD) => {
            tracing::info!(
                sn = %incoming.sn,
                info_type = incoming.info_type,
                trace,
                "telemetry event"
            );
            store_event(state, &incoming).await;
        }
        Some(other) => {
            tracing::warn!(
                sn = %incoming.sn,
                info_type = other,
                trace,
                "unhandled infoType; persisted"
            );
            store_event(state, &incoming).await;
        }
        None => {
            tracing::warn!(sn = %incoming.sn, trace, "payload without an infoType; persisted");
            store_event(state, &incoming).await;
        }
    }
}

/// The status/event/record catch-all row: every semantic message is kept verbatim.
async fn store_event(state: &AppState, incoming: &Incoming<'_>) {
    let payload = serde_json::to_string(incoming.data).unwrap_or_else(|_| "null".to_string());
    let result = queries::insert_event(
        &state.db,
        incoming.sn,
        incoming.channel,
        incoming.endpoint,
        incoming.info_type,
        incoming.event,
        incoming.task_id,
        incoming.user_id,
        incoming.device_ts,
        &payload,
        incoming.trace_ref,
    )
    .await;
    if let Err(error) = result {
        tracing::error!(%error, sn = %incoming.sn, "could not persist the payload");
    }
}

async fn store_map(state: &AppState, incoming: &Incoming<'_>) {
    let upload = match map::parse(incoming.data) {
        Ok(upload) => upload,
        Err(error) => {
            tracing::warn!(%error, sn = %incoming.sn, "map upload does not parse; stored as an event");
            return store_event(state, incoming).await;
        }
    };
    let decoded = match map::decode(&upload) {
        Ok(decoded) => Some(decoded),
        Err(error) => {
            // Still store the row: the compressed grid is kept as received, so a
            // decoder fix can recover it later (doc/PLAN.md §7).
            tracing::warn!(%error, sn = %incoming.sn, "map grid did not decode; storing the raw blob");
            None
        }
    };
    let compressed = upload
        .map
        .as_deref()
        .and_then(|encoded| {
            base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .ok()
        })
        .unwrap_or_default();

    let result = queries::insert_map_upload(
        &state.db,
        incoming.sn,
        &upload,
        &compressed,
        incoming.trace_ref,
    )
    .await;
    match result {
        Ok(()) => tracing::info!(
            sn = %incoming.sn,
            map_id = ?upload.map_id,
            path_id = ?upload.path_id,
            dimensions = ?upload.width.zip(upload.height),
            grid = decoded.as_ref().map(|decoded| decoded.summary()).unwrap_or_else(|| "not decoded".into()),
            trace = incoming.trace_ref.unwrap_or("-"),
            "map upload stored"
        ),
        Err(error) => tracing::error!(%error, sn = %incoming.sn, "could not store the map upload"),
    }
}

async fn store_path(state: &AppState, incoming: &Incoming<'_>) {
    let chunk = match path::parse(incoming.data) {
        Ok(chunk) => chunk,
        Err(error) => {
            tracing::warn!(%error, sn = %incoming.sn, "clean-path reply does not parse");
            return store_event(state, incoming).await;
        }
    };

    let existing = queries::load_clean_path(&state.db, incoming.sn, chunk.path_id).await;
    let mut assembly = match existing {
        Ok(Some(row)) => path::Assembly::from_json(&row.0),
        Ok(None) => path::Assembly::new(),
        Err(error) => {
            tracing::error!(%error, sn = %incoming.sn, "could not load the clean path");
            return;
        }
    };
    let filled = assembly.merge(&chunk);
    let complete = assembly.is_complete();

    let user_id = chunk
        .user_id
        .clone()
        .or_else(|| incoming.user_id.map(str::to_string));
    let result = queries::upsert_clean_path(
        &state.db,
        incoming.sn,
        chunk.path_id,
        user_id.as_deref(),
        assembly.len() as i64,
        &assembly.to_json(),
        complete,
        incoming.trace_ref,
    )
    .await;
    match result {
        Ok(()) => tracing::info!(
            sn = %incoming.sn,
            path_id = chunk.path_id,
            filled,
            total = assembly.len(),
            complete,
            trace = incoming.trace_ref.unwrap_or("-"),
            "clean path chunk merged"
        ),
        Err(error) => tracing::error!(%error, sn = %incoming.sn, "could not store the clean path"),
    }
}
