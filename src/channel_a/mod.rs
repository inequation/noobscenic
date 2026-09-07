//! Channel A — the `cleanPack/*` HTTP surface (doc/PLAN.md §9).
//!
//! Phase 1 implements only the catch-all: every request is tapped in full, persisted,
//! warned about, and answered with the benign `{"code":0}` envelope. That is
//! deliberately load-bearing — the documented endpoint list is not claimed to be
//! exhaustive, and a 404 where the device expected an answer can stall it, while an
//! unrecognised endpoint answered with `code:0` costs nothing. Every fallback hit is
//! how the endpoint list gets completed.

use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::Router;
use serde_json::{json, Map, Value};

use crate::db::now_ms;
use crate::error::Result;
use crate::wire::{Direction, Record, Recorded};
use crate::AppState;

/// The device uploads LZ4 maps through `uploadEvents`, so bodies are not small; this
/// is a sanity bound, not a policy.
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

pub fn router(state: AppState) -> Router {
    Router::new()
        .fallback(catch_all)
        .layer(middleware::from_fn_with_state(state.clone(), tap_traffic))
        .with_state(state)
}

pub async fn serve(
    state: AppState,
    bind: SocketAddr,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(addr = %listener.local_addr()?, "channel A (HTTP) listening");

    axum::serve(
        listener,
        router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown)
    .await?;
    Ok(())
}

/// Buffer both bodies so the tap sees exactly what crossed the socket, and hand the
/// request's trace reference to the handler so its database rows can point at it.
async fn tap_traffic(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.to_string());

    let (parts, body) = request.into_parts();
    let method = parts.method.to_string();
    let path = parts.uri.path().to_string();
    let query = parts.uri.query().map(str::to_string);

    let bytes = match axum::body::to_bytes(body, MAX_BODY_BYTES).await {
        Ok(bytes) => bytes,
        Err(error) => {
            // Answer benignly anyway: an error here would only push the device into a
            // retry loop, and the tap already has the headers.
            tracing::warn!(%error, %method, %path, "could not read the request body");
            return ok_envelope().into_response();
        }
    };

    let mut record = Record::new("A", Direction::In, "request", &bytes)
        .meta("method", method.clone())
        .meta("path", path.clone())
        .meta("headers", header_map(&parts.headers));
    if let Some(peer) = &peer {
        record = record.peer(peer.clone());
    }
    if let Some(query) = &query {
        record = record.meta("query", query.clone());
    }
    let recorded = state.tap.record(record);

    let mut request = Request::from_parts(parts, Body::from(bytes));
    if let Some(recorded) = recorded.clone() {
        request.extensions_mut().insert(recorded);
    }

    let response = next.run(request).await;

    let (parts, body) = response.into_parts();
    let bytes = match axum::body::to_bytes(body, MAX_BODY_BYTES).await {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::warn!(%error, "could not read our own response body");
            return Response::from_parts(parts, Body::empty());
        }
    };

    let mut record = Record::new("A", Direction::Out, "response", &bytes)
        .meta("method", method)
        .meta("path", path)
        .meta("status", parts.status.as_u16());
    if let Some(peer) = peer {
        record = record.peer(peer);
    }
    if let Some(recorded) = &recorded {
        record = record.meta("req_seq", recorded.seq);
    }
    state.tap.record(record);

    Response::from_parts(parts, Body::from(bytes))
}

/// Accept anything, remember it, answer harmlessly.
async fn catch_all(State(state): State<AppState>, request: Request) -> Response {
    let recorded = request.extensions().get::<Recorded>().cloned();
    let method = request.method().to_string();
    let path = request.uri().path().to_string();

    let body = axum::body::to_bytes(request.into_body(), MAX_BODY_BYTES)
        .await
        .unwrap_or_default();
    let payload = String::from_utf8_lossy(&body).into_owned();

    tracing::warn!(
        %method,
        %path,
        bytes = body.len(),
        trace = recorded.as_ref().map(|r| r.reference.as_str()).unwrap_or("-"),
        "unhandled channel-A endpoint; answered code:0"
    );

    let result = sqlx::query(
        "INSERT INTO events (sn, channel, direction, endpoint, payload, trace_ref, received_ms)
         VALUES (NULL, 'A', 'in', ?, ?, ?, ?)",
    )
    .bind(&path)
    .bind(&payload)
    .bind(recorded.map(|r| r.reference))
    .bind(now_ms())
    .execute(&state.db)
    .await;

    if let Err(error) = result {
        // The device does not care that our database is unhappy; still answer.
        tracing::error!(%error, %path, "could not persist the unhandled request");
    }

    ok_envelope().into_response()
}

/// `{"code":0,"message":"ok","data":{}}` — the channel-A success envelope.
fn ok_envelope() -> impl IntoResponse {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        json!({"code": 0, "message": "ok", "data": {}}).to_string(),
    )
}

fn header_map(headers: &HeaderMap) -> Value {
    let mut object = Map::new();
    for (name, value) in headers {
        object.insert(
            name.as_str().to_string(),
            Value::String(String::from_utf8_lossy(value.as_bytes()).into_owned()),
        );
    }
    Value::Object(object)
}
