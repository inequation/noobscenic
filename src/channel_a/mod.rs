//! Channel A — the `cleanPack/*` HTTP surface (doc/PLAN.md §9).
//!
//! Phase 2 implements the endpoints the robot needs to hold a session: `register`
//! (mint a session + cookie), `getSockAddr` (the gateway's address — the robot's
//! fallback when its `ip_port.json` is missing), `sync` and the version check. Every
//! request also passes the session gate (§9.2): a known cookie proceeds, a missing
//! one is served with a warning, and a present-but-stale one gets `code:102`, which
//! is the documented way to make the robot re-register — including a robot that
//! paired against the phase-1 catch-all and still has an empty session.
//!
//! Everything else still falls through to the catch-all: tapped in full, persisted,
//! warned about, and answered with the benign `{"code":0}` envelope. That is
//! deliberately load-bearing — the documented endpoint list is not claimed to be
//! exhaustive, and a 404 where the device expected an answer can stall it, while an
//! unrecognised endpoint answered with `code:0` costs nothing. Every fallback hit is
//! how the endpoint list gets completed.

use std::net::SocketAddr;

use axum::Router;
use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::{Map, Value, json};
use tokio::net::TcpListener;

use crate::AppState;
use crate::db::now_ms;
use crate::error::Result;
use crate::session::{self, Credential};
use crate::wire::{Direction, Record, Recorded};

pub mod form;
pub(crate) mod handlers;

/// The device uploads LZ4 maps through `uploadEvents`, so bodies are not small; this
/// is a sanity bound, not a policy.
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

pub fn router(state: AppState) -> Router {
    Router::new()
        // `/` is the web UI, except when the query is the device's OTA version
        // check — `web::index` splits on that (doc/PLAN.md §19).
        .route("/", get(crate::web::index))
        // Browsers ask for this automatically; answering 204 keeps the catch-all's
        // warning log free of noise that is not device traffic.
        .route(
            "/favicon.ico",
            get(|| async { axum::http::StatusCode::NO_CONTENT }),
        )
        .route("/api/robots", get(crate::web::robots))
        .route("/api/commands", get(crate::web::commands))
        .route("/api/robot/{id}/summary", get(crate::web::summary))
        .route("/api/robot/{id}/map", get(crate::web::map))
        .route("/api/robot/{id}/path", get(crate::web::path))
        .route("/api/robot/{id}/command", post(crate::web::command))
        .route("/api/robot/{id}/control", post(crate::web::control))
        .route(
            "/api/robot/{id}/zones",
            get(crate::web::zones).put(crate::web::zones_write),
        )
        .route(
            "/api/robot/{id}/zones/refresh",
            post(crate::web::zones_refresh),
        )
        .route("/api/robot/{id}/zones/clean", post(crate::web::zones_clean))
        .route("/cleanPack/register", post(handlers::register::register))
        .route(
            "/cleanPack/getSockAddr",
            get(handlers::sock_addr::sock_addr).post(handlers::sock_addr::sock_addr),
        )
        .route("/cleanPack/binding", post(handlers::binding::binding))
        .route("/cleanPack/unbinding", post(handlers::binding::unbinding))
        // This unit posts its unbind to `//cleanPack/unbinding`, double slash and
        // all (wire traces 2026-10-06/07); route it to the same handler rather than
        // letting the catch-all answer without recording the transition.
        .route("//cleanPack/unbinding", post(handlers::binding::unbinding))
        .route("/cleanPack/sync", post(handlers::sync::sync))
        .route(
            "/cleanPack/uploadEvents",
            post(handlers::upload_events::upload_events),
        )
        .route("/cleanPack/response", post(handlers::response::response))
        .route("/cleanPack/uploadLogs", post(handlers::uploads::upload_raw))
        .route(
            "/cleanPack/uploadStats",
            post(handlers::uploads::upload_raw),
        )
        .route(
            "/cleanPack/uploadSingle",
            post(handlers::uploads::upload_raw),
        )
        .fallback(catch_all)
        // The gate sits inside the tap (the last layer added runs first), so the
        // `code:102` answers it produces are traced like every other response.
        .layer(middleware::from_fn_with_state(state.clone(), cookie_gate))
        .layer(middleware::from_fn_with_state(state.clone(), tap_traffic))
        .with_state(state)
}

pub async fn serve(
    listener: TcpListener,
    state: AppState,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
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

/// The session gate — doc/PLAN.md §9.2. `register` is exempt: it is what mints the
/// session in the first place.
async fn cookie_gate(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let path = request.uri().path();
    // The web UI is browser traffic, not a device: it has no cookie to offer, and
    // its own API is not part of the device contract.
    if path == "/cleanPack/register"
        || path == "/"
        || path == "/favicon.ico"
        || path.starts_with("/api/")
    {
        return next.run(request).await;
    }

    let check = session::check(
        &state.db,
        request.headers(),
        state.config.registration.session_ttl_secs,
    )
    .await;
    match check {
        Ok(Credential::Valid(_sn)) => next.run(request).await,
        Ok(Credential::Absent) => {
            tracing::warn!(%path, "channel-A request without a Cookie header; serving it anyway");
            next.run(request).await
        }
        Ok(Credential::Stale) => {
            tracing::info!(
                %path,
                "empty or unknown channel-A cookie; answering code:102 to make the device re-register"
            );
            handlers::sid_expired()
        }
        Err(error) => {
            // A database hiccup must not stall the robot: serve the request and let
            // the handler deal with its own persistence.
            tracing::error!(%error, %path, "session lookup failed; serving the request anyway");
            next.run(request).await
        }
    }
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
