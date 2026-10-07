//! Phase-6 binding state machine over channel A: the preBind is recorded and
//! answered idempotently, unbinding flips the state back, and both land in
//! `events` with a usable `trace_ref`.

use std::path::PathBuf;

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode, header};
use serde_json::Value;
use tower::ServiceExt;

use noobscenic::AppState;
use noobscenic::config::{Config, Logging, Wire};

const SN: &str = "LSLDSM7PROTEST03";

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("noobscenic-bind-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// The tap is on: half of this feature is the event row that ties a bind to the
/// bytes that asked for it.
async fn setup(tag: &str) -> (AppState, Router) {
    let config = Config {
        data_dir: scratch(tag),
        logging: Logging {
            file_enabled: false,
            wire: Wire {
                enabled: true,
                dir: Some(scratch(tag).join("traces")),
                ..Wire::default()
            },
            ..Logging::default()
        },
        ..Config::default()
    };
    let state = noobscenic::start(config).await.expect("state");
    let app = noobscenic::channel_a::router(state.clone());
    (state, app)
}

async fn cleanup(state: AppState) {
    let dir = state.config.data_dir.clone();
    state.db.close().await;
    let _ = std::fs::remove_dir_all(dir);
}

fn post(path: &str, body: &str) -> Request<Body> {
    let mut request = Request::builder()
        .method("POST")
        .uri(path)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body.to_string()))
        .expect("request");
    request
        .extensions_mut()
        .insert(ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            45678,
        ))));
    request
}

async fn call(app: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (status, serde_json::from_slice(&bytes).expect("JSON body"))
}

/// `(bind_state, bind_user, bound_ms, unbound_ms)`.
async fn device_row(state: &AppState) -> (String, Option<String>, Option<i64>, Option<i64>) {
    sqlx::query_as("SELECT bind_state, bind_user, bound_ms, unbound_ms FROM devices WHERE sn = ?")
        .bind(SN)
        .fetch_one(&state.db)
        .await
        .expect("device row")
}

#[tokio::test]
async fn a_prebind_is_recorded_answered_and_idempotent() {
    let (state, app) = setup("bind").await;

    // The body this unit actually sends: `ts` repeats the SN (vendor quirk).
    let body = format!("sn={SN}&ts={SN}&userId=Foo");
    let (status, response) = call(&app, post("/cleanPack/binding", &body)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["code"], 0);

    let (bind_state, bind_user, bound_ms, unbound_ms) = device_row(&state).await;
    assert_eq!(bind_state, "bound");
    assert_eq!(bind_user.as_deref(), Some("Foo"));
    assert!(bound_ms.is_some(), "the bind time must be recorded");
    assert!(unbound_ms.is_none());

    // BindUser retries until it sees code:0; a re-send must succeed just the same.
    let (status, response) = call(&app, post("/cleanPack/binding", &body)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["code"], 0);
    assert_eq!(device_row(&state).await.0, "bound");

    let events: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT endpoint, trace_ref FROM events WHERE sn = ? ORDER BY id")
            .bind(SN)
            .fetch_all(&state.db)
            .await
            .expect("events");
    assert_eq!(events.len(), 2, "both preBinds must be recorded");
    for (endpoint, trace_ref) in events {
        assert_eq!(endpoint, "/cleanPack/binding");
        let reference = trace_ref.expect("trace_ref");
        let (file, seq) = reference.split_once('#').expect("file#seq");
        assert!(
            !seq.is_empty() && state.config.wire_dir().join(file).exists(),
            "trace_ref {reference} must point at a real file"
        );
    }

    cleanup(state).await;
}

#[tokio::test]
async fn unbinding_flips_the_state_back() {
    let (state, app) = setup("unbind").await;
    let body = format!("sn={SN}&ts={SN}&userId=Foo");
    let _ = call(&app, post("/cleanPack/binding", &body)).await;
    assert_eq!(device_row(&state).await.0, "bound");

    let (status, response) = call(&app, post("/cleanPack/unbinding", &body)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["code"], 0);

    let (bind_state, bind_user, bound_ms, unbound_ms) = device_row(&state).await;
    assert_eq!(bind_state, "unbound");
    assert!(bind_user.is_none(), "unbinding clears the user");
    assert!(bound_ms.is_some(), "the bind stays on record as history");
    assert!(unbound_ms.is_some());

    cleanup(state).await;
}

#[tokio::test]
async fn a_bind_without_an_sn_is_answered_but_not_recorded() {
    let (state, app) = setup("nosn").await;
    let (status, response) = call(&app, post("/cleanPack/binding", "ts=1&userId=Foo")).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "never stall the robot over a bad body"
    );
    assert_eq!(response["code"], 0);

    let devices: i64 = sqlx::query_scalar("SELECT count(1) FROM devices")
        .fetch_one(&state.db)
        .await
        .expect("count");
    assert_eq!(devices, 0);
    cleanup(state).await;
}

#[tokio::test]
async fn the_double_slash_unbind_path_the_robot_actually_posts_is_recorded() {
    let (state, app) = setup("dslash").await;
    let body = format!("sn={SN}&ts={SN}&userId=Foo");
    let _ = call(&app, post("/cleanPack/binding", &body)).await;
    assert_eq!(device_row(&state).await.0, "bound");

    // The bench unit's own URL: `//cleanPack/unbinding` (wire traces).
    let (status, response) = call(&app, post("//cleanPack/unbinding", &body)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["code"], 0);
    assert_eq!(
        device_row(&state).await.0,
        "unbound",
        "the double-slash path must flip the state like the canonical one"
    );

    cleanup(state).await;
}
