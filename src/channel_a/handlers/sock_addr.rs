//! `GET|POST /cleanPack/getSockAddr` — tell the robot where the channel-B gateway is.
//!
//! The robot normally dials the address cached in `ip_port.json` and this endpoint
//! is only its fallback (PROTOCOL.md, "How the robot chooses its Channel-B target").
//! That is exactly why it matters: it is the one path that lets a robot whose gateway
//! file went missing find us again without a re-home.

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, State};
use axum::response::Response;
use serde_json::{Value, json};

use crate::AppState;
use crate::channel_a::handlers;
use crate::config::Config;

pub async fn sock_addr(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Response {
    let addresses = advertised(&state.config, peer);
    if addresses.is_empty() {
        tracing::warn!(
            peer = %peer,
            "getSockAddr: no gateway address could be determined; answering with an empty list"
        );
    } else {
        tracing::info!(
            peer = %peer,
            addresses = %serde_json::Value::Array(addresses.clone()),
            "getSockAddr"
        );
    }
    handlers::ok(json!({"addr_list": addresses}))
}

/// Configured addresses if the operator set any; otherwise ask the OS which local
/// address it would use to reach this robot (a UDP `connect` sends nothing).
fn advertised(config: &Config, peer: SocketAddr) -> Vec<Value> {
    if let Some(list) = &config.gateway.advertise {
        return list
            .iter()
            .map(|addr| json!({"ip": addr.ip, "port": addr.port}))
            .collect();
    }

    let port = config.gateway.bind.port();
    let Ok(socket) = std::net::UdpSocket::bind(("0.0.0.0", 0)) else {
        return Vec::new();
    };
    if socket.connect((peer.ip(), 9)).is_err() {
        return Vec::new();
    }
    match socket.local_addr() {
        Ok(local) => vec![json!({"ip": local.ip().to_string(), "port": port})],
        Err(_) => Vec::new(),
    }
}
