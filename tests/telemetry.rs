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
use noobscenic::config::{Config, Logging, Wire};

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
    let exists = state.config.wire_dir().join(file).exists();
    exists && !seq.is_empty()
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
    assert!(
        trace_ref_is_usable(&state, row.2.as_deref()),
        "trace_ref {:?}",
        row.2
    );

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
    assert!(trace_ref_is_usable(&state, row.6.as_deref()));

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
    assert!(trace_ref_is_usable(&state, trace.as_deref()));

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
    assert_eq!(rows.len(), 3);
    for (endpoint, fields, trace) in &rows {
        assert!(fields.contains("\"taskid\":\"7\""), "{fields}");
        assert!(fields.contains("\"anything\":true"), "{fields}");
        assert!(trace_ref_is_usable(&state, trace.as_deref()), "{endpoint}");
    }

    cleanup(state).await;
}
