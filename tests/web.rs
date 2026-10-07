//! Phase-7 web UI API: the page at `/`, the robot list and `?id=` resolution, the
//! map/path payloads the canvas draws, the command catalog, and the
//! presence-gated path poller.

use std::path::PathBuf;

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode, header};
use base64::Engine as _;
use serde_json::{Value, json};
use tower::ServiceExt;

use noobscenic::AppState;
use noobscenic::config::{Config, Logging, Wire};

const SN: &str = "LSLDSM7PROTEST04";
const ID: &str = "Foo";

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("noobscenic-web-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

async fn setup(tag: &str) -> (AppState, Router) {
    let config = Config {
        data_dir: scratch(tag),
        logging: Logging {
            file_enabled: false,
            wire: Wire {
                enabled: false,
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

fn get(path: &str) -> Request<Body> {
    let mut request = Request::builder()
        .method("GET")
        .uri(path)
        .header(header::ACCEPT, "text/html")
        .body(Body::empty())
        .expect("request");
    request
        .extensions_mut()
        .insert(ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            45678,
        ))));
    request
}

fn post_form(path: &str, body: &str) -> Request<Body> {
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

async fn call(app: &Router, request: Request<Body>) -> (StatusCode, String, Option<String>) {
    let response = app.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .map(|value| value.to_str().unwrap_or_default().to_string());
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        String::from_utf8_lossy(&bytes).into_owned(),
        content_type,
    )
}

async fn call_json(app: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let (status, body, _) = call(app, request).await;
    let value = serde_json::from_str(&body).unwrap_or(Value::Null);
    (status, value)
}

async fn adopt(state: &AppState) {
    noobscenic::db::queries::upsert_device_seen(&state.db, SN, None)
        .await
        .expect("device");
    noobscenic::db::queries::record_bind(&state.db, SN, true, Some(ID))
        .await
        .expect("bind");
}

#[tokio::test]
async fn the_root_serves_the_page_and_still_answers_the_version_check() {
    let (state, app) = setup("root").await;

    let (status, body, content_type) = call(&app, get("/")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        content_type.unwrap_or_default().starts_with("text/html"),
        "the browser gets HTML"
    );
    assert!(
        body.contains("<canvas"),
        "the page must carry the map canvas"
    );
    assert!(body.contains("/api/robots"), "the page must call the API");

    let (status, value) = call_json(&app, get("/?version=1&sn=LSLDSM7PROTEST04")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["hasUpdateFile"], 0, "the robot still gets its JSON");

    cleanup(state).await;
}

#[tokio::test]
async fn robots_are_listed_by_their_setid_and_resolved_with_an_sn_fallback() {
    let (state, app) = setup("robots").await;
    adopt(&state).await;

    let (status, value) = call_json(&app, get("/api/robots")).await;
    assert_eq!(status, StatusCode::OK);
    let robot = &value["robots"][0];
    assert_eq!(robot["id"], ID);
    assert_eq!(robot["sn"], SN);
    assert_eq!(robot["bind_state"], "bound");

    let (status, value) = call_json(&app, get("/api/robot/Foo/summary")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["sn"], SN);
    assert!(value["status"].is_null(), "no telemetry yet");

    let (status, value) = call_json(&app, get(&format!("/api/robot/{SN}/summary"))).await;
    assert_eq!(status, StatusCode::OK, "the sn must work as the id too");
    assert_eq!(value["id"], SN);

    let (status, _) = call_json(&app, get("/api/robot/nobody/summary")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    cleanup(state).await;
}

#[tokio::test]
async fn the_map_endpoint_returns_the_stored_grid_and_frame() {
    let (state, app) = setup("map").await;
    adopt(&state).await;

    // 4x4 grid of 0x7F: one literal, a 15-byte match at offset 1.
    let block = vec![0x1B, 0x7F, 0x01, 0x00, 0x00];
    let upload = noobscenic::map::MapUpload {
        sn: Some(SN.into()),
        map_id: Some(7),
        auto_area_id: None,
        path_id: None,
        width: Some(4),
        height: Some(4),
        resolution: Some(0.05),
        x_min: Some(-1.0),
        y_min: Some(-2.0),
        lz4_len: Some(block.len() as i64),
        area: Vec::new(),
        map: Some(base64::engine::general_purpose::STANDARD.encode(&block)),
        charge_handle_pos: Some(vec![-100, 200]),
        charge_handle_phi: Some(0),
        charge_handle_state: Some("find".into()),
    };
    noobscenic::db::queries::insert_map_upload(&state.db, SN, &upload, &block, None)
        .await
        .expect("map row");

    let (status, value) = call_json(&app, get("/api/robot/Foo/map")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["width"], 4);
    assert_eq!(value["height"], 4);
    assert_eq!(value["dock_x"], -100);
    assert_eq!(value["dock_y"], 200);
    let grid = base64::engine::general_purpose::STANDARD
        .decode(value["grid_b64"].as_str().expect("grid"))
        .expect("base64");
    assert_eq!(grid, vec![0x7F; 16], "the grid arrives decompressed");

    let (_, summary) = call_json(&app, get("/api/robot/Foo/summary")).await;
    assert_eq!(summary["map"]["map_id"], 7);
    assert_eq!(summary["map"]["width"], 4);

    cleanup(state).await;
}

#[tokio::test]
async fn the_path_endpoint_strips_the_type_tags() {
    let (state, app) = setup("path").await;
    adopt(&state).await;

    // 3933 & !3 = 3932; -296 is already a multiple of four; the hole is dropped.
    noobscenic::db::queries::upsert_clean_path(
        &state.db,
        SN,
        42,
        Some(ID),
        3,
        "[[3933.0,-296.0],null,[5160.0,-2064.0]]",
        false,
        None,
    )
    .await
    .expect("path row");

    let (status, value) = call_json(&app, get("/api/robot/Foo/path")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["path_id"], 42);
    assert_eq!(value["points"], json!([[3932, -296], [5160, -2064]]));

    let (_, summary) = call_json(&app, get("/api/robot/Foo/summary")).await;
    assert_eq!(summary["path"]["span"], 3, "the hole still occupies a slot");
    assert_eq!(summary["path"]["points"], 2);

    cleanup(state).await;
}

#[tokio::test]
async fn catalog_commands_are_enqueued_and_unknown_names_rejected() {
    let (state, app) = setup("command").await;
    adopt(&state).await;

    let (status, value) = call_json(&app, get("/api/commands")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        value["commands"],
        json!(["smartClean", "pause", "continue", "stop", "findCharge"])
    );

    let (status, value) =
        call_json(&app, post_form("/api/robot/Foo/command", "name=smartClean")).await;
    assert_eq!(status, StatusCode::OK);
    let queued = value["queued"].as_i64().expect("row id");

    let (info_type, payload, command_state): (i64, String, String) =
        sqlx::query_as("SELECT info_type, payload, state FROM commands WHERE id = ?")
            .bind(queued)
            .fetch_one(&state.db)
            .await
            .expect("command row");
    assert_eq!(info_type, 21005);
    assert_eq!(payload, "{\"mode\":\"smartClean\"}");
    assert_eq!(command_state, "pending");

    let (status, _) = call_json(&app, post_form("/api/robot/Foo/command", "name=dropTables")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let count: i64 = sqlx::query_scalar("SELECT count(1) FROM commands")
        .fetch_one(&state.db)
        .await
        .expect("count");
    assert_eq!(count, 1, "an unknown name must not reach the queue");

    cleanup(state).await;
}

#[tokio::test]
async fn the_path_poller_fetches_only_while_watched_and_never_while_charging() {
    let (state, _app) = setup("poller").await;
    adopt(&state).await;

    // A live connection, without a socket: the poller only checks the registry.
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    state.registry.register(
        SN,
        noobscenic::channel_b::registry::handle("b-test", "127.0.0.1:1", tx),
    );
    // A growing path: one point stored, so the next fetch starts at index 1.
    noobscenic::db::queries::upsert_clean_path(
        &state.db,
        SN,
        42,
        Some(ID),
        1,
        "[[100.0,200.0]]",
        false,
        None,
    )
    .await
    .expect("path row");
    let status = "{\"mode\":\"smartclean\",\"elec\":88}";
    noobscenic::db::queries::insert_event(
        &state.db,
        SN,
        "A",
        Some("/cleanPack/uploadEvents"),
        Some(20001),
        None,
        None,
        Some(ID),
        None,
        status,
        None,
    )
    .await
    .expect("status event");

    // Nobody is watching: no request goes out.
    noobscenic::web::poll_paths_once(&state).await;
    let count: i64 = sqlx::query_scalar("SELECT count(1) FROM commands WHERE info_type = 21011")
        .fetch_one(&state.db)
        .await
        .expect("count");
    assert_eq!(count, 0, "an unwatched robot must not be polled");

    // The page polls: one fetch, from the first point we do not have.
    state.watchers.touch(SN);
    noobscenic::web::poll_paths_once(&state).await;
    let (payload, command_state): (String, String) =
        sqlx::query_as("SELECT payload, state FROM commands WHERE info_type = 21011")
            .fetch_one(&state.db)
            .await
            .expect("path command");
    let payload: Value = serde_json::from_str(&payload).expect("payload JSON");
    assert_eq!(payload, json!({"startPos": 1, "mask": 0}));
    assert_eq!(command_state, "pending");

    // While it is still pending or sent, the poller waits.
    noobscenic::web::poll_paths_once(&state).await;
    let count: i64 = sqlx::query_scalar("SELECT count(1) FROM commands WHERE info_type = 21011")
        .fetch_one(&state.db)
        .await
        .expect("count");
    assert_eq!(count, 1, "one 21011 request in flight at a time");

    // Charging: no path to grow, no request.
    sqlx::query("UPDATE commands SET state = 'acked' WHERE info_type = 21011")
        .execute(&state.db)
        .await
        .expect("settle the row");
    noobscenic::db::queries::insert_event(
        &state.db,
        SN,
        "A",
        Some("/cleanPack/uploadEvents"),
        Some(20001),
        None,
        None,
        Some(ID),
        None,
        "{\"mode\":\"charge\",\"elec\":95}",
        None,
    )
    .await
    .expect("status event");
    noobscenic::web::poll_paths_once(&state).await;
    let count: i64 = sqlx::query_scalar("SELECT count(1) FROM commands WHERE info_type = 21011")
        .fetch_one(&state.db)
        .await
        .expect("count");
    assert_eq!(count, 1, "a charging robot must not be polled");

    cleanup(state).await;
}
