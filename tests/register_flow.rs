//! Channel-A flows: `register` mints a session, the cookie gate enforces it (and
//! nudges the device into re-registering when it is empty or stale), `getSockAddr`
//! hands out the gateway, and `sync`/the version check answer no-update.

use std::net::SocketAddr;
use std::path::PathBuf;

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode, header};
use serde_json::Value;
use tower::ServiceExt;

use noobscenic::AppState;
use noobscenic::config::{AdvertisedAddr, Config, Gateway, Logging, Registration, Wire};

const PEER: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 45678);

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("noobscenic-a-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

async fn setup(tag: &str) -> (AppState, Router) {
    setup_with(
        tag,
        Some(vec![AdvertisedAddr {
            ip: "192.168.1.208".into(),
            port: 8081,
        }]),
        None,
    )
    .await
}

async fn setup_with(
    tag: &str,
    advertise: Option<Vec<AdvertisedAddr>>,
    session_ttl_secs: Option<u64>,
) -> (AppState, Router) {
    let config = Config {
        data_dir: scratch(tag),
        gateway: Gateway {
            advertise,
            ..Gateway::default()
        },
        registration: Registration {
            session_ttl_secs,
            ..Registration::default()
        },
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

fn request(method: &str, uri: &str, body: &str, cookie: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    let mut request = builder.body(Body::from(body.to_string())).expect("request");
    request.extensions_mut().insert(ConnectInfo(PEER));
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

#[tokio::test]
async fn register_mints_a_session_and_a_cookie() {
    let (state, app) = setup("register").await;

    let (status, body) = call(
        &app,
        request(
            "POST",
            "/cleanPack/register",
            "sn=SN1&ld_sn=LDSN&sig=s%2Bq%3D&ts=0",
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["code"], 0);

    let session = body["data"]["session"].as_str().expect("session");
    let cookie = body["data"]["cookies"].as_str().expect("cookies");
    assert_eq!(session.len(), 32, "the device stores at most 63 bytes");
    assert_eq!(cookie.len(), 32, "the device stores at most 55 bytes");

    let device: (String, Option<String>) =
        sqlx::query_as("SELECT sn, ld_sn FROM devices WHERE sn = 'SN1'")
            .fetch_one(&state.db)
            .await
            .expect("device row");
    assert_eq!(device, ("SN1".to_string(), Some("LDSN".to_string())));

    let sig: Option<String> = sqlx::query_scalar("SELECT sig_raw FROM sessions WHERE cookie = ?")
        .bind(cookie)
        .fetch_one(&state.db)
        .await
        .expect("session row");
    assert_eq!(
        sig.as_deref(),
        Some("s+q="),
        "sig is stored URL-decoded and unverified"
    );

    cleanup(state).await;
}

#[tokio::test]
async fn an_empty_or_unknown_cookie_gets_code_102() {
    let (state, app) = setup("stale").await;
    let (_, registered) = call(&app, request("POST", "/cleanPack/register", "sn=SN2", None)).await;
    let cookie = registered["data"]["cookies"].as_str().expect("cookies");

    // Exactly what the robot sent after pairing against the phase-1 catch-all: a
    // Cookie header with no value.
    let (_, body) = call(&app, request("POST", "/cleanPack/sync", "sn=SN2", Some(""))).await;
    assert_eq!(
        body["code"], 102,
        "an empty cookie must force a re-register"
    );

    let (_, body) = call(
        &app,
        request(
            "POST",
            "/cleanPack/sync",
            "sn=SN2",
            Some("cookies=deadbeef"),
        ),
    )
    .await;
    assert_eq!(
        body["code"], 102,
        "an unknown cookie must force a re-register"
    );

    // The gate covers the catch-all too, so binding/uploads converge the same way.
    let (_, body) = call(
        &app,
        request(
            "POST",
            "/cleanPack/uploadEvents",
            "sn=SN2",
            Some("cookies="),
        ),
    )
    .await;
    assert_eq!(body["code"], 102);

    let (_, body) = call(
        &app,
        request(
            "POST",
            "/cleanPack/sync",
            "sn=SN2",
            Some(&format!("cookies={cookie}")),
        ),
    )
    .await;
    assert_eq!(
        body["code"], 0,
        "the freshly minted cookie must be accepted"
    );

    cleanup(state).await;
}

#[tokio::test]
async fn a_missing_cookie_is_still_served() {
    let (state, app) = setup("absent").await;

    let (_, body) = call(
        &app,
        request("POST", "/cleanPack/someNewEndpoint", "sn=SN3", None),
    )
    .await;
    assert_eq!(
        body["code"], 0,
        "no Cookie header at all must not stall the device"
    );

    let rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE endpoint = '/cleanPack/someNewEndpoint'",
    )
    .fetch_one(&state.db)
    .await
    .expect("events count");
    assert_eq!(rows, 1, "the fallback still persists what it saw");

    cleanup(state).await;
}

#[tokio::test]
async fn get_sock_addr_returns_the_configured_gateway() {
    let (state, app) = setup("sockaddr").await;
    let (_, registered) = call(&app, request("POST", "/cleanPack/register", "sn=SN4", None)).await;
    let cookie = format!(
        "cookies={}",
        registered["data"]["cookies"].as_str().expect("cookies")
    );

    let (_, body) = call(
        &app,
        request(
            "GET",
            "/cleanPack/getSockAddr?version=1&sn=SN4&companyId=48",
            "",
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(body["code"], 0);
    assert_eq!(body["data"]["addr_list"][0]["ip"], "192.168.1.208");
    assert_eq!(body["data"]["addr_list"][0]["port"], 8081);

    cleanup(state).await;
}

#[tokio::test]
async fn get_sock_addr_auto_detects_the_local_address_when_unconfigured() {
    let (state, app) = setup_with("sockaddr-auto", None, None).await;

    // No cookie either: the gate serves a missing header, so this is exactly the
    // path a robot takes when its ip_port.json went missing and it comes asking.
    let (_, body) = call(
        &app,
        request("GET", "/cleanPack/getSockAddr?version=1&sn=SN6", "", None),
    )
    .await;
    assert_eq!(body["code"], 0);
    assert_eq!(
        body["data"]["addr_list"][0]["ip"], "127.0.0.1",
        "the local address facing the requesting peer"
    );
    assert_eq!(
        body["data"]["addr_list"][0]["port"], 8081,
        "the gateway's configured port"
    );

    cleanup(state).await;
}

#[tokio::test]
async fn an_expired_session_gets_code_102_and_registration_recovers() {
    let (state, app) = setup_with("ttl", None, Some(60)).await;
    let (_, registered) = call(&app, request("POST", "/cleanPack/register", "sn=SN7", None)).await;
    let cookie = registered["data"]["cookies"]
        .as_str()
        .expect("cookies")
        .to_string();

    // Age the session past its TTL: the next request must be told to re-register.
    sqlx::query("UPDATE sessions SET created_ms = created_ms - 600000 WHERE cookie = ?")
        .bind(&cookie)
        .execute(&state.db)
        .await
        .expect("age the session");
    let (_, body) = call(
        &app,
        request(
            "POST",
            "/cleanPack/sync",
            "sn=SN7",
            Some(&format!("cookies={cookie}")),
        ),
    )
    .await;
    assert_eq!(body["code"], 102, "an expired sid must force a re-register");

    // ...and the device recovering the documented way — register again, retry with
    // the new cookie — must be served.
    let (_, again) = call(&app, request("POST", "/cleanPack/register", "sn=SN7", None)).await;
    let fresh = again["data"]["cookies"]
        .as_str()
        .expect("fresh cookie")
        .to_string();
    assert_ne!(fresh, cookie);
    let (_, body) = call(
        &app,
        request(
            "POST",
            "/cleanPack/sync",
            "sn=SN7",
            Some(&format!("cookies={fresh}")),
        ),
    )
    .await;
    assert_eq!(body["code"], 0);

    cleanup(state).await;
}

#[tokio::test]
async fn sync_records_attributes_and_answers_no_update() {
    let (state, app) = setup("sync").await;
    let (_, registered) = call(&app, request("POST", "/cleanPack/register", "sn=SN5", None)).await;
    let cookie = format!(
        "cookies={}",
        registered["data"]["cookies"].as_str().expect("cookies")
    );

    let (_, body) = call(
        &app,
        request(
            "POST",
            "/cleanPack/sync",
            "sn=SN5&companyId=48&mcuVer=S6&version=0.7.1&versionCode=1241&gitSha=NULL&cloud=psnk",
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(body["code"], 0);
    assert_eq!(body["hasUpdateFile"], 0, "top-level shape");
    assert_eq!(body["data"]["hasUpdateFile"], 0, "nested shape");

    let row: (Option<i64>, Option<String>, Option<String>, Option<i64>) = sqlx::query_as(
        "SELECT company_id, mcu_ver, app_version, version_code FROM devices WHERE sn = 'SN5'",
    )
    .fetch_one(&state.db)
    .await
    .expect("device row");
    assert_eq!(
        row,
        (
            Some(48),
            Some("S6".to_string()),
            Some("0.7.1".to_string()),
            Some(1241)
        )
    );

    let (_, body) = call(
        &app,
        request("GET", "/?version=1&sn=SN5&companyId=48", "", None),
    )
    .await;
    assert_eq!(body["code"], 0);
    assert_eq!(body["hasUpdateFile"], 0);
    assert_eq!(body["data"]["hasUpdateFile"], 0);

    cleanup(state).await;
}
