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

fn post_json(path: &str, body: &Value) -> Request<Body> {
    let mut request = Request::builder()
        .method("POST")
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json")
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

fn put_json(path: &str, body: &Value) -> Request<Body> {
    let mut request = Request::builder()
        .method("PUT")
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json")
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
        json!([
            "smartClean",
            "pause",
            "continue",
            "stop",
            "findCharge",
            "pauseReturn"
        ])
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
async fn the_control_endpoint_writes_realtime_steering_frames() {
    let (state, app) = setup("control").await;
    adopt(&state).await;

    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    state.registry.register(
        SN,
        noobscenic::channel_b::registry::handle("b-test", "127.0.0.1:1", tx),
    );

    let (status, value) = call_json(&app, post_form("/api/robot/Foo/control", "code=3005")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["code"], 3005);
    let frame = rx.try_recv().expect("a steering frame was written");
    assert_eq!(frame.frame["encrypt"], 0);
    assert_eq!(frame.frame["data"]["infoType"], 21020);
    assert_eq!(frame.frame["data"]["data"]["ctrlCode"], 3005);
    assert!(
        frame.command_id.is_none(),
        "steering bypasses the queue: no row to point at"
    );

    // Only the documented steering and stop codes are accepted.
    let (status, _) = call_json(&app, post_form("/api/robot/Foo/control", "code=3013")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(rx.try_recv().is_err(), "a refused code must not be written");

    // An offline device is a conflict, not a silent drop.
    state.registry.remove(SN, "b-test");
    let (status, _) = call_json(&app, post_form("/api/robot/Foo/control", "code=4000")).await;
    assert_eq!(status, StatusCode::CONFLICT);

    cleanup(state).await;
}

#[tokio::test]
async fn the_control_watchdog_leaves_manual_mode_when_the_client_goes_quiet() {
    let (state, app) = setup("watchdog").await;
    adopt(&state).await;

    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    state.registry.register(
        SN,
        noobscenic::channel_b::registry::handle("b-test", "127.0.0.1:1", tx),
    );

    // A steering frame arms the watchdog; a fresh one must not fire it.
    let _ = call_json(&app, post_form("/api/robot/Foo/control", "code=3005")).await;
    let steering = rx.try_recv().expect("the steering frame");
    assert_eq!(steering.frame["data"]["data"]["ctrlCode"], 3005);
    noobscenic::web::poll_control_watchdog_once(&state, i64::MAX).await;
    assert!(
        rx.try_recv().is_err(),
        "a recent frame must not trigger the watchdog"
    );

    // Silence: the watchdog leaves manual mode once, then stays quiet.
    noobscenic::web::poll_control_watchdog_once(&state, 0).await;
    let stop = rx.try_recv().expect("the watchdog frame");
    assert_eq!(stop.frame["encrypt"], 0);
    assert_eq!(stop.frame["data"]["infoType"], 21020);
    assert_eq!(stop.frame["data"]["data"]["ctrlCode"], 4000);
    noobscenic::web::poll_control_watchdog_once(&state, 0).await;
    assert!(
        rx.try_recv().is_err(),
        "the watchdog fires once per session"
    );

    // An explicit 4000 from the client clears the tracking, so no second stop follows.
    let _ = call_json(&app, post_form("/api/robot/Foo/control", "code=4000")).await;
    let explicit = rx.try_recv().expect("the client's own 4000");
    assert_eq!(explicit.frame["data"]["data"]["ctrlCode"], 4000);
    noobscenic::web::poll_control_watchdog_once(&state, 0).await;
    assert!(
        rx.try_recv().is_err(),
        "an explicit leave must not be followed by a watchdog frame"
    );

    cleanup(state).await;
}

async fn store_status(state: &AppState, mode: &str) {
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
        &format!("{{\"mode\":\"{mode}\",\"elec\":90}}"),
        None,
    )
    .await
    .expect("status event");
}

async fn last_command(state: &AppState) -> (i64, String) {
    sqlx::query_as("SELECT info_type, payload FROM commands ORDER BY id DESC LIMIT 1")
        .fetch_one(&state.db)
        .await
        .expect("a command row")
}

#[tokio::test]
async fn zones_read_back_can_be_edited_and_written_with_an_etag() {
    let (state, app) = setup("zones").await;
    adopt(&state).await;
    let (tx, _rx) = tokio::sync::mpsc::channel(4);
    state.registry.register(
        SN,
        noobscenic::channel_b::registry::handle("b-test", "127.0.0.1:1", tx),
    );

    // Refresh enqueues a 21004 read.
    let (status, _) = call_json(&app, post_form("/api/robot/Foo/zones/refresh", "")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(last_command(&state).await.0, 21004);

    // The robot's reply lands through cleanPack/response and becomes the cached list.
    let reply = format!(
        "sn={SN}&infoType=21004&ts=1&userId=u&data={}",
        "{\"infoType\":21004,\"data\":{\"mapId\":7,\"value\":[]}}"
    );
    let (status, _) = call_json(&app, post_form("/cleanPack/response", &reply)).await;
    assert_eq!(status, StatusCode::OK);

    let (status, zones) = call_json(&app, get("/api/robot/Foo/zones")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(zones["version"], 1);
    assert_eq!(zones["map_id"], 7);
    assert_eq!(zones["zones"], json!([]));

    // Save a rectangle against that version.
    let rectangle = json!([{
        "vertexs": [[100, 100], [300, 100], [300, 300], [100, 300]],
        "active": "forbid", "forbidType": "all", "id": 1, "name": "Desk",
    }]);
    let (status, written) = call_json(
        &app,
        put_json(
            "/api/robot/Foo/zones",
            &json!({"version": 1, "value": rectangle}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(written["version"], 2);
    let (info_type, payload) = last_command(&state).await;
    assert_eq!(info_type, 21003);
    let payload: Value = serde_json::from_str(&payload).expect("payload JSON");
    assert_eq!(payload["mapId"], 7, "the robot's own mapId is kept");
    assert_eq!(payload["value"][0]["name"], "Desk");

    // The etag: the same version cannot be written twice, and a bad list is refused.
    let (status, _) = call_json(
        &app,
        put_json(
            "/api/robot/Foo/zones",
            &json!({"version": 1, "value": rectangle}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "stale version");
    let two_vertices = json!([{"vertexs": [[0, 0], [100, 100]], "active": "forbid"}]);
    let (status, _) = call_json(
        &app,
        put_json(
            "/api/robot/Foo/zones",
            &json!({"version": 2, "value": two_vertices}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "two vertices");
    let long_name = json!([{
        "vertexs": [[0, 0], [100, 0], [100, 100]],
        "active": "forbid", "name": "x".repeat(32),
    }]);
    let (status, _) = call_json(
        &app,
        put_json(
            "/api/robot/Foo/zones",
            &json!({"version": 2, "value": long_name}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "name over the buffer");

    cleanup(state).await;
}

#[tokio::test]
async fn zone_edits_and_zone_cleans_are_blocked_while_a_clean_runs() {
    let (state, app) = setup("zones-clean").await;
    adopt(&state).await;
    let (tx, _rx) = tokio::sync::mpsc::channel(4);
    state.registry.register(
        SN,
        noobscenic::channel_b::registry::handle("b-test", "127.0.0.1:1", tx),
    );
    noobscenic::db::queries::store_area_settings(&state.db, SN, "{\"mapId\":7,\"value\":[]}", None)
        .await
        .expect("cached list");
    store_status(&state, "sweep").await;

    let rectangle = json!([{"vertexs": [[0, 0], [100, 0], [100, 100]], "active": "forbid"}]);
    let (status, error) = call_json(
        &app,
        put_json(
            "/api/robot/Foo/zones",
            &json!({"version": 1, "value": rectangle}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        error["error"]
            .as_str()
            .unwrap_or_default()
            .contains("clean"),
        "the refusal names the running clean: {error}"
    );

    let (status, _) = call_json(
        &app,
        post_json("/api/robot/Foo/zones/clean", &json!({"ids": [1]})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "no second clean");

    // Once docked, the same request becomes a queued 21023.
    store_status(&state, "fullcharge").await;
    let (status, queued) = call_json(
        &app,
        post_json("/api/robot/Foo/zones/clean", &json!({"ids": [1, 2]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(queued["queued"].is_i64());
    let (info_type, payload) = last_command(&state).await;
    assert_eq!(info_type, 21023);
    let payload: Value = serde_json::from_str(&payload).expect("payload JSON");
    assert_eq!(payload["cleanId"], json!([1, 2]));

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

/// The catch-all stored the preBind as the raw form body before the phase-6 handler
/// existed; the recovery turns it into a real `bind_user`.
async fn store_bind_event(state: &AppState, endpoint: &str, payload: &str) {
    noobscenic::db::queries::insert_event(
        &state.db,
        SN,
        "A",
        Some(endpoint),
        None,
        None,
        None,
        None,
        None,
        payload,
        None,
    )
    .await
    .expect("event");
}

/// The pre-phase-6 catch-all stored its rows without an `sn` — the body is the only
/// place the serial survives (this is the bench unit's actual history).
async fn store_legacy_bind_event(state: &AppState, endpoint: &str, payload: &str) {
    sqlx::query(
        "INSERT INTO events (sn, channel, direction, endpoint, payload, received_ms)
         VALUES (NULL, 'A', 'in', ?, ?, ?)",
    )
    .bind(endpoint)
    .bind(payload)
    .bind(noobscenic::db::now_ms())
    .execute(&state.db)
    .await
    .expect("legacy event");
}

#[tokio::test]
async fn a_stored_prebind_recovers_the_setid_id_for_the_ui() {
    let (state, app) = setup("recover").await;
    noobscenic::db::queries::upsert_device_seen(&state.db, SN, None)
        .await
        .expect("device");
    store_legacy_bind_event(
        &state,
        "/cleanPack/binding",
        &format!("sn={SN}&ts={SN}&userId=Foo"),
    )
    .await;

    assert_eq!(noobscenic::web::recover_bind_ids(&state.db).await, 1);
    let (_, value) = call_json(&app, get("/api/robots")).await;
    assert_eq!(
        value["robots"][0]["id"], ID,
        "the UI can now resolve ?id=Foo"
    );
    assert_eq!(value["robots"][0]["bind_state"], "bound");
    let bound_ms: Option<i64> = sqlx::query_scalar("SELECT bound_ms FROM devices WHERE sn = ?")
        .bind(SN)
        .fetch_one(&state.db)
        .await
        .expect("bound_ms");
    assert!(bound_ms.is_some(), "the original bind time is kept");

    assert_eq!(
        noobscenic::web::recover_bind_ids(&state.db).await,
        0,
        "a recovered device is not touched again"
    );
    cleanup(state).await;
}

#[tokio::test]
async fn a_later_unbind_blocks_recovery_and_a_json_prebind_still_works() {
    let (state, _app) = setup("recover2").await;
    noobscenic::db::queries::upsert_device_seen(&state.db, SN, None)
        .await
        .expect("device");
    store_bind_event(
        &state,
        "/cleanPack/binding",
        &format!("sn={SN}&ts={SN}&userId=Foo"),
    )
    .await;
    // The newest of the pair is an unbind: the robot is knowingly unbound.
    store_bind_event(&state, "//cleanPack/unbinding", &format!("sn={SN}")).await;
    assert_eq!(
        noobscenic::web::recover_bind_ids(&state.db).await,
        0,
        "an unbind must not resurrect the old id"
    );

    // A fresh bind arrives in the phase-6 handler's JSON shape.
    store_bind_event(
        &state,
        "/cleanPack/binding",
        &format!("{{\"sn\":\"{SN}\",\"ts\":\"{SN}\",\"userId\":\"Bar\"}}"),
    )
    .await;
    assert_eq!(noobscenic::web::recover_bind_ids(&state.db).await, 1);
    let user: Option<String> = sqlx::query_scalar("SELECT bind_user FROM devices WHERE sn = ?")
        .bind(SN)
        .fetch_one(&state.db)
        .await
        .expect("bind_user");
    assert_eq!(user.as_deref(), Some("Bar"));
    cleanup(state).await;
}
