//! Phase-4 telemetry over channel A: `uploadEvents` status/maps, `response` paths,
//! and the raw upload endpoints — each landing in its table with a usable trace_ref.

use std::path::PathBuf;

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode, header};
use base64::Engine as _;
use serde_json::{Value, json};
use tower::ServiceExt;

use noobscenic::AppState;
use noobscenic::config::{Config, LogFile, Logging, Wire};

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("noobscenic-tel-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Unlike the other integration tests, the tap is **on**: half of this phase is the
/// `trace_ref` that ties every stored row back to its bytes.
async fn setup(tag: &str) -> (AppState, Router) {
    let config = Config {
        data_dir: scratch(tag),
        logging: Logging {
            file: Some(LogFile::Enabled(false)),
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

/// 4x4 grid of `0x7F` cells: one literal, then a 15-byte match at offset 1.
fn tiny_lz4_grid() -> Vec<u8> {
    vec![0x1B, 0x7F, 0x01, 0x00, 0x00]
}

fn trace_ref_is_usable(state: &AppState, reference: Option<&str>) -> bool {
    let Some(reference) = reference else {
        return false;
    };
    let Some((file, seq)) = reference.split_once('#') else {
        return false;
    };
    if seq.is_empty() {
        return false;
    }
    if file.is_empty() {
        // A raw-only build names the byte file by sequence alone; the date directory
        // it landed in is not part of the reference (doc/PLAN.md §21).
        return cfg!(feature = "debug-raw");
    }
    state.config.wire_dir().join(file).exists()
}

/// Every stored row carries a usable `trace_ref` when a sink is compiled in, and
/// NULL when none is (doc/PLAN.md §21).
fn assert_trace_ref(state: &AppState, reference: Option<&str>, what: &str) {
    if cfg!(any(feature = "debug-trace", feature = "debug-raw")) {
        assert!(
            trace_ref_is_usable(state, reference),
            "{what}: trace_ref {reference:?}"
        );
    } else {
        assert!(
            reference.is_none(),
            "{what}: no trace sink is compiled in, got {reference:?}"
        );
    }
}

type MapRow = (
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Vec<u8>,
    Option<i64>,
    Option<i64>,
    Option<String>,
);

#[tokio::test]
async fn a_status_push_becomes_an_event_row() {
    let (state, app) = setup("status").await;
    let body =
        "sn=TELE&ts=1234&data={\"infoType\":20001,\"data\":{\"mode\":\"dormant\",\"elec\":99}}";
    let (status, response) = call(&app, post("/cleanPack/uploadEvents", body)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["code"], 0);

    let row: (i64, String, Option<String>) =
        sqlx::query_as("SELECT info_type, payload, trace_ref FROM events WHERE sn = 'TELE'")
            .fetch_one(&state.db)
            .await
            .expect("event row");
    assert_eq!(row.0, 20001);
    assert!(row.1.contains("\"mode\":\"dormant\""), "{}", row.1);
    assert_trace_ref(&state, row.2.as_deref(), "status push");

    cleanup(state).await;
}

#[tokio::test]
async fn dock_statuses_advance_one_path_run_each() {
    let (state, app) = setup("path-docks").await;
    for mode in ["dormant", "charge", "fullcharge", "sweep", "charge"] {
        let body = format!(
            "sn=TELE&ts=1&data={{\"infoType\":20001,\"data\":{{\"mode\":\"{mode}\",\"elec\":99}}}}"
        );
        let (status, response) = call(&app, post("/cleanPack/uploadEvents", &body)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["code"], 0);
    }

    let state_row: (i64, i64, Option<i64>) = sqlx::query_as(
        "SELECT docked, current_run_id, previous_run_id FROM path_state WHERE sn = 'TELE'",
    )
    .fetch_one(&state.db)
    .await
    .expect("path state");
    assert_eq!(
        state_row,
        (1, 2, Some(1)),
        "charge -> fullcharge is one dock event; the next charge starts run 2"
    );

    cleanup(state).await;
}

/// With `history` off the table is a two-row cache per device: the newest
/// status and the newest bind/unbind row, and nothing else.
#[cfg(not(feature = "history"))]
#[tokio::test]
async fn events_keep_only_the_newest_status_and_bind_rows() {
    let (state, app) = setup("status-lean").await;
    for mode in ["dormant", "sweep"] {
        let body = format!(
            "sn=TELE&ts=1&data={{\"infoType\":20001,\"data\":{{\"mode\":\"{mode}\",\"elec\":99}}}}"
        );
        let (status, response) = call(&app, post("/cleanPack/uploadEvents", &body)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["code"], 0);
    }
    // A non-status, non-bind message is dropped instead of stored.
    let body = "sn=TELE&ts=1&data={\"infoType\":20003,\"data\":{\"code\":6023}}";
    let (status, _) = call(&app, post("/cleanPack/uploadEvents", body)).await;
    assert_eq!(status, StatusCode::OK);

    // Bind and unbind rows prune to the newest one.
    for endpoint in ["/cleanPack/binding", "//cleanPack/unbinding"] {
        noobscenic::db::queries::insert_event(
            &state.db,
            "TELE",
            "A",
            Some(endpoint),
            None,
            None,
            None,
            None,
            None,
            "{\"userId\":\"u\"}",
            None,
        )
        .await
        .expect("bind event");
    }

    let status: (i64, String) = sqlx::query_as(
        "SELECT info_type, payload FROM events WHERE sn = 'TELE' AND info_type = 20001",
    )
    .fetch_one(&state.db)
    .await
    .expect("status row");
    assert_eq!(status.0, 20001);
    assert!(
        status.1.contains("\"mode\":\"sweep\""),
        "the newest status wins: {}",
        status.1
    );
    let binds: Vec<String> =
        sqlx::query_scalar("SELECT endpoint FROM events WHERE sn = 'TELE' AND info_type IS NULL")
            .fetch_all(&state.db)
            .await
            .expect("bind rows");
    assert_eq!(binds, vec!["//cleanPack/unbinding"], "the newest bind wins");
    let count: i64 = sqlx::query_scalar("SELECT count(1) FROM events WHERE sn = 'TELE'")
        .fetch_one(&state.db)
        .await
        .expect("count");
    assert_eq!(count, 2, "one status and one bind row per device");

    // A row written by a feature-on build is removed by the startup prune.
    sqlx::query(
        "INSERT INTO events (sn, channel, direction, info_type, payload, received_ms)
         VALUES ('TELE', 'A', 'in', 20003, '{}', ?)",
    )
    .bind(noobscenic::db::now_ms())
    .execute(&state.db)
    .await
    .expect("history row");
    let pruned = noobscenic::db::queries::prune_events_latest_only(&state.db)
        .await
        .expect("prune");
    assert_eq!(pruned, 1, "the history row was pruned");

    cleanup(state).await;
}

#[tokio::test]
async fn a_map_upload_is_decoded_and_stored_with_its_blob() {
    let (state, app) = setup("map").await;
    let map = base64::engine::general_purpose::STANDARD.encode(tiny_lz4_grid());
    let message = json!({
        "infoType": 20002,
        "data": {
            "SN": "TELE", "mapId": 77, "autoAreaId": 3, "pathId": 9,
            "width": 4, "height": 4, "resolution": 0.05, "x_min": -1.0, "y_min": -2.0,
            "lz4_len": tiny_lz4_grid().len(), "area": [{"id": 1, "name": "kitchen"}],
            "map": map, "base64_len": map.len(),
            "chargeHandlePos": [1500, -2500], "chargeHandlePhi": 3141,
            "chargeHandleState": "find",
        }
    });
    let body = format!("sn=TELE&ts=1&data={message}");
    let (status, response) = call(&app, post("/cleanPack/uploadEvents", &body)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["code"], 0);

    let row: MapRow = sqlx::query_as(
        "SELECT map_id, width, height, cells_lz4, dock_x, dock_y, trace_ref
             FROM map_uploads WHERE sn = 'TELE'",
    )
    .fetch_one(&state.db)
    .await
    .expect("map row");
    assert_eq!(row.0, Some(77));
    assert_eq!((row.1, row.2), (Some(4), Some(4)));
    assert_eq!(
        row.3,
        tiny_lz4_grid(),
        "the compressed blob is stored as received"
    );
    assert_eq!(
        (row.4, row.5),
        (Some(1500), Some(-2500)),
        "dock position in mm"
    );
    assert_trace_ref(&state, row.6.as_deref(), "map upload");

    cleanup(state).await;
}

#[tokio::test]
async fn map_uploads_keep_only_the_newest_grid_per_device() {
    let (state, app) = setup("map-latest").await;
    let map = base64::engine::general_purpose::STANDARD.encode(tiny_lz4_grid());
    let body_for = |sn: &str, map_id: i64| {
        let message = json!({
            "infoType": 20002,
            "data": {
                "SN": sn, "mapId": map_id, "autoAreaId": 3, "pathId": 9,
                "width": 4, "height": 4, "resolution": 0.05, "x_min": -1.0, "y_min": -2.0,
                "lz4_len": tiny_lz4_grid().len(), "area": [],
                "map": map.clone(), "base64_len": map.len(),
                "chargeHandlePos": [1500, -2500], "chargeHandlePhi": 3141,
                "chargeHandleState": "find",
            }
        });
        format!("sn={sn}&ts=1&data={message}")
    };
    for (sn, map_id) in [("TELE", 77), ("TELE", 78), ("OTHER", 5)] {
        let body = body_for(sn, map_id);
        let (status, response) = call(&app, post("/cleanPack/uploadEvents", &body)).await;
        assert_eq!(status, StatusCode::OK, "{sn}/{map_id}");
        assert_eq!(response["code"], 0, "{sn}/{map_id}");
    }

    let rows: Vec<(String, i64)> =
        sqlx::query_as("SELECT sn, map_id FROM map_uploads ORDER BY sn, id")
            .fetch_all(&state.db)
            .await
            .expect("map rows");
    assert_eq!(
        rows,
        vec![("OTHER".to_string(), 5), ("TELE".to_string(), 78)],
        "one live grid per device, and it is the newest upload"
    );

    cleanup(state).await;
}

#[tokio::test]
async fn clean_path_chunks_assemble_across_responses() {
    let (state, app) = setup("path").await;

    // Out of order: the tail chunk first, then the head.
    let tail = "data={\"message\":\"ok\",\"infoType\":21011,\"data\":{\"userId\":\"u\",\"pathID\":9,\"startPos\":2,\"totalPoints\":4,\"posArray\":[[3004,3000],[4008,4004]],\"pointCounts\":2}}&infoType=21011&sn=TELE&userId=u";
    let head = "data={\"message\":\"ok\",\"infoType\":21011,\"data\":{\"userId\":\"u\",\"pathID\":9,\"startPos\":0,\"totalPoints\":4,\"posArray\":[[1004,1000],[2008,2004]],\"pointCounts\":2}}&infoType=21011&sn=TELE&userId=u";
    for body in [tail, head] {
        let (status, response) = call(&app, post("/cleanPack/response", body)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["code"], 0);
    }

    let (total, complete, points, trace): (i64, i64, String, Option<String>) = sqlx::query_as(
        "SELECT total_points, complete, points_json, trace_ref FROM clean_paths WHERE sn = 'TELE'",
    )
    .fetch_one(&state.db)
    .await
    .expect("path row");
    assert_eq!(total, 4);
    assert_eq!(complete, 1, "all four points arrived");
    assert_eq!(
        points,
        "[[1004.0,1000.0],[2008.0,2004.0],[3004.0,3000.0],[4008.0,4004.0]]"
    );
    assert_trace_ref(&state, trace.as_deref(), "clean path");

    cleanup(state).await;
}

/// With `history` off paths are bounded by dock returns, not by individual path ids.
/// At the second dock the run before the first boundary is dropped; at the third the
/// next-oldest run goes, leaving one completed route plus the current one.
#[cfg(not(feature = "history"))]
#[tokio::test]
async fn clean_paths_keep_the_current_and_previous_dock_runs() {
    let (state, _app) = setup("paths-lean").await;
    for path_id in [7, 8] {
        noobscenic::db::queries::upsert_clean_path(
            &state.db,
            "TELE",
            path_id,
            None,
            1,
            "[[100.0,100.0]]",
            true,
            None,
        )
        .await
        .expect("path row");
    }

    // D1 closes run 0 and starts run 1.
    noobscenic::db::queries::observe_path_mode(&state.db, "TELE", "charge")
        .await
        .expect("first dock");
    noobscenic::db::queries::observe_path_mode(&state.db, "TELE", "sweep")
        .await
        .expect("leave dock");
    for path_id in [9, 10] {
        noobscenic::db::queries::upsert_clean_path(
            &state.db,
            "TELE",
            path_id,
            None,
            2,
            "[[200.0,200.0],[300.0,300.0]]",
            true,
            None,
        )
        .await
        .expect("path row");
    }

    // D2 drops everything before D1, but keeps every segment of run 1.
    let change = noobscenic::db::queries::observe_path_mode(&state.db, "TELE", "fullcharge")
        .await
        .expect("second dock");
    assert_eq!(change.run_id, 2);
    assert_eq!(change.pruned, 2);
    let rows: Vec<(i64, i64)> =
        sqlx::query_as("SELECT path_id, run_id FROM clean_paths ORDER BY path_id")
            .fetch_all(&state.db)
            .await
            .expect("paths");
    assert_eq!(
        rows,
        vec![(9, 1), (10, 1)],
        "all segments of the completed run survive their first dock return"
    );

    // Run 2 is now in progress. D3 removes run 1 and leaves only run 2.
    noobscenic::db::queries::observe_path_mode(&state.db, "TELE", "sweep")
        .await
        .expect("leave dock again");
    noobscenic::db::queries::upsert_clean_path(
        &state.db,
        "TELE",
        11,
        None,
        2,
        "[[400.0,400.0],[500.0,500.0]]",
        true,
        None,
    )
    .await
    .expect("current path row");
    let change = noobscenic::db::queries::observe_path_mode(&state.db, "TELE", "charge")
        .await
        .expect("third dock");
    assert_eq!(change.run_id, 3);
    assert_eq!(change.pruned, 2);
    let rows: Vec<(i64, i64)> =
        sqlx::query_as("SELECT path_id, run_id FROM clean_paths ORDER BY path_id")
            .fetch_all(&state.db)
            .await
            .expect("paths");
    assert_eq!(
        rows,
        vec![(11, 2)],
        "only the last completed run and the current run remain"
    );

    cleanup(state).await;
}

#[cfg(not(feature = "history"))]
#[tokio::test]
async fn startup_prune_keeps_the_last_two_dock_runs() {
    let (state, _app) = setup("paths-startup").await;
    noobscenic::db::queries::observe_path_mode(&state.db, "TELE", "charge")
        .await
        .expect("D1");
    noobscenic::db::queries::observe_path_mode(&state.db, "TELE", "sweep")
        .await
        .expect("leave D1");
    noobscenic::db::queries::upsert_clean_path(
        &state.db,
        "TELE",
        9,
        None,
        1,
        "[[100.0,100.0]]",
        true,
        None,
    )
    .await
    .expect("run 1 segment");
    noobscenic::db::queries::observe_path_mode(&state.db, "TELE", "charge")
        .await
        .expect("D2");
    noobscenic::db::queries::observe_path_mode(&state.db, "TELE", "sweep")
        .await
        .expect("leave D2");
    noobscenic::db::queries::upsert_clean_path(
        &state.db,
        "TELE",
        10,
        None,
        1,
        "[[200.0,200.0]]",
        true,
        None,
    )
    .await
    .expect("run 2 segment");
    sqlx::query(
        "INSERT INTO clean_paths
             (sn, path_id, total_points, points_json, complete, updated_ms, run_id)
         VALUES ('TELE', 8, 1, '[[0.0,0.0]]', 1, ?, 0)",
    )
    .bind(noobscenic::db::now_ms() - 60_000)
    .execute(&state.db)
    .await
    .expect("legacy run 0");

    let pruned = noobscenic::db::queries::prune_clean_paths_before_previous_run(&state.db)
        .await
        .expect("startup prune");
    assert_eq!(pruned, 1);
    let path_ids: Vec<i64> = sqlx::query_scalar("SELECT path_id FROM clean_paths ORDER BY path_id")
        .fetch_all(&state.db)
        .await
        .expect("paths");
    assert_eq!(path_ids, vec![9, 10]);

    cleanup(state).await;
}

#[cfg(feature = "history")]
#[tokio::test]
async fn history_keeps_paths_across_dock_runs() {
    let (state, _app) = setup("paths-history").await;
    for path_id in [7, 8, 9] {
        noobscenic::db::queries::observe_path_mode(
            &state.db,
            "TELE",
            if path_id == 7 { "sweep" } else { "charge" },
        )
        .await
        .expect("mode");
        noobscenic::db::queries::observe_path_mode(&state.db, "TELE", "sweep")
            .await
            .expect("leave dock");
        noobscenic::db::queries::upsert_clean_path(
            &state.db,
            "TELE",
            path_id,
            None,
            1,
            "[[100.0,100.0]]",
            true,
            None,
        )
        .await
        .expect("path row");
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM clean_paths")
        .fetch_one(&state.db)
        .await
        .expect("count");
    assert_eq!(count, 3, "the history build never applies path retention");

    cleanup(state).await;
}

#[tokio::test]
async fn raw_uploads_are_stored_verbatim() {
    let (state, app) = setup("raw").await;
    let body = "sn=TELE&taskid=7&data={\"anything\":true}";
    for endpoint in [
        "/cleanPack/uploadLogs",
        "/cleanPack/uploadStats",
        "/cleanPack/uploadSingle",
    ] {
        let (status, response) = call(&app, post(endpoint, body)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["code"], 0);
    }

    let rows: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT endpoint, fields_json, trace_ref FROM uploads_raw WHERE sn = 'TELE'",
    )
    .fetch_all(&state.db)
    .await
    .expect("upload rows");
    if cfg!(feature = "debug-raw") {
        assert_eq!(rows.len(), 3);
        for (endpoint, fields, trace) in &rows {
            assert!(fields.contains("\"taskid\":\"7\""), "{fields}");
            assert!(fields.contains("\"anything\":true"), "{fields}");
            assert_trace_ref(&state, trace.as_deref(), endpoint);
        }
    } else {
        assert!(
            rows.is_empty(),
            "the upload archive needs debug-raw: {rows:?}"
        );
    }

    cleanup(state).await;
}
