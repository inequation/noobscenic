//! The operator console and the one-shot verbs — doc/PLAN.md §12.
//!
//! Reads lines from stdin when it is a terminal, and does nothing at all otherwise,
//! so the server stays headless whenever nobody can type into it. The verbs that
//! print state are public because `main.rs` exposes each of them as a one-shot CLI
//! subcommand too; `send` in both places only enqueues into the `commands` table
//! (§10.3), which the gateway's poller drains.

use std::io::IsTerminal;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::watch;

use crate::AppState;
use crate::channel_b::codec;
use crate::db::queries;

const HELP: &str = "\
commands:
  devices                             known devices: online state, last seen, version
  device <sn>                         stored row + latest session + connection
  events [<sn>] [n]                   newest n semantic messages (default 20)
  map <sn>                            newest map: dims, origin, dock, areas
  path <sn> [<path_id>]               assembled clean path summary
  commands [<sn>] [n]                 queue state: pending/sent/acked/expired/failed
  send <sn> <infoType> <json>         enqueue a command (encrypt:0)
  send-enc <sn> <infoType> <json>     enqueue with encrypt:1 (needs the config flag)
  send-full <sn> <json>               send one raw inner message now (experiment)
  trace on|off                        toggle the wire tap
  style compact|styled                how channel-B frames are written
  pongs on|off                        answer 21006 pings (liveness experiment)
  quit                                shut the server down";

const EVENTS_DEFAULT: i64 = 20;
const COMMANDS_DEFAULT: i64 = 20;
const PREVIEW: usize = 120;

pub fn spawn(state: AppState, shutdown: watch::Sender<()>) {
    if !state.config.console.enabled {
        tracing::debug!("console disabled by config; running headless");
        return;
    }
    if !std::io::stdin().is_terminal() {
        tracing::debug!("stdin is not a terminal; running headless");
        return;
    }

    tokio::spawn(async move {
        println!("noobscenic console ready (`help` for commands)");
        let mut lines = BufReader::new(tokio::io::stdin()).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            match parse(&line) {
                Command::Empty => {}
                Command::Help => println!("{HELP}"),
                Command::Devices => show_devices(&state).await,
                Command::Device { sn } => show_device(&state, &sn).await,
                Command::Events { sn, limit } => show_events(&state, sn.as_deref(), limit).await,
                Command::Map { sn } => show_map(&state, &sn).await,
                Command::Path { sn, path_id } => show_path(&state, &sn, path_id).await,
                Command::Commands { sn, limit } => {
                    show_commands(&state, sn.as_deref(), limit).await
                }
                Command::Send {
                    sn,
                    info_type,
                    data,
                } => enqueue(&state, &sn, info_type, data, false).await,
                Command::SendEncrypted {
                    sn,
                    info_type,
                    data,
                } => enqueue(&state, &sn, info_type, data, true).await,
                Command::SendFull { sn, frame } => {
                    send_now(&state, &sn, "message", codec::envelope(0, frame)).await
                }
                Command::Trace { on } => {
                    state.tap.set_enabled(on);
                    println!(
                        "wire tracing: {}",
                        if state.tap.is_enabled() { "on" } else { "off" }
                    );
                }
                Command::Style { styled } => {
                    state
                        .frame_style
                        .store(styled, std::sync::atomic::Ordering::Relaxed);
                    println!(
                        "channel-B frame style: {}",
                        if styled { "styled" } else { "compact" }
                    );
                }
                Command::Pongs { on } => {
                    state.pongs.store(on, std::sync::atomic::Ordering::Relaxed);
                    println!(
                        "pongs: {}",
                        if on {
                            "on"
                        } else {
                            "off (the robot should drop us if it cares)"
                        }
                    );
                }
                Command::Quit => {
                    println!("shutting down");
                    let _ = shutdown.send(());
                    break;
                }
                Command::Unknown(text) => println!("unknown command: {text} (`help` lists them)"),
            }
        }
    });
}

/// `devices` — every device ever seen, with its live connection state if any.
pub async fn show_devices(state: &AppState) {
    match queries::list_devices(&state.db).await {
        Err(error) => println!("could not list devices: {error}"),
        Ok(devices) if devices.is_empty() => println!("no devices seen yet"),
        Ok(devices) => {
            for (sn, bind_state, mcu_ver, app_version, last_seen_ms) in devices {
                let online = if state.registry.is_online(&sn) {
                    "online"
                } else {
                    "offline"
                };
                let version = app_version.as_deref().or(mcu_ver.as_deref()).unwrap_or("-");
                println!(
                    "{sn:<20} {online:<8} {bind_state:<8} last seen {}  {version}",
                    ts(last_seen_ms)
                );
            }
        }
    }
}

/// `device <sn>` — the stored row, the newest session and the live connection.
pub async fn show_device(state: &AppState, sn: &str) {
    let summary = match queries::device_summary(&state.db, sn).await {
        Ok(summary) => summary,
        Err(error) => return println!("device lookup failed: {error}"),
    };
    let Some((sn, ld_sn, bind_state, mcu_ver, app_version, first_seen_ms, last_seen_ms)) = summary
    else {
        return println!("{sn}: no such device");
    };
    println!("sn            {sn}");
    println!("ld_sn         {}", ld_sn.as_deref().unwrap_or("-"));
    println!("bind state    {bind_state}");
    println!(
        "app version   {}  (mcu {})",
        app_version.as_deref().unwrap_or("-"),
        mcu_ver.as_deref().unwrap_or("-")
    );
    println!("first seen    {}", ts(first_seen_ms));
    println!("last seen     {}", ts(last_seen_ms));
    match state.registry.get(&sn) {
        Some(handle) => println!(
            "connection    online, {} from {}",
            handle.conn_id, handle.peer
        ),
        None => println!("connection    offline"),
    }
    match queries::latest_session_summary(&state.db, &sn).await {
        Ok(Some((created_ms, last_used_ms, revoked_ms, key_len))) => println!(
            "session       {}, {key_len}-char key, created {}, last used {}",
            if revoked_ms.is_some() {
                "revoked"
            } else {
                "live"
            },
            ts(created_ms),
            last_used_ms.map(ts).unwrap_or_else(|| "-".to_string())
        ),
        Ok(None) => println!("session       none"),
        Err(error) => println!("session       lookup failed: {error}"),
    }
}

/// `events [<sn>] [n]` — the newest semantic messages, oldest of the batch first.
pub async fn show_events(state: &AppState, sn: Option<&str>, limit: i64) {
    match queries::recent_events(&state.db, sn, limit).await {
        Err(error) => println!("could not read events: {error}"),
        Ok(events) if events.is_empty() => println!("no events"),
        Ok(events) => {
            for event in events.iter().rev() {
                let info = match event.info_type {
                    Some(value) => crate::proto::info_type::name(value)
                        .map(str::to_string)
                        .unwrap_or_else(|| value.to_string()),
                    None => "-".to_string(),
                };
                println!(
                    "#{:<6} {}  {}  {:<10} {:<12} {}  {}",
                    event.id,
                    ts(event.received_ms),
                    event.channel,
                    event.endpoint.as_deref().unwrap_or("-"),
                    info,
                    event.trace_ref.as_deref().unwrap_or("-"),
                    preview(event.payload.as_deref().unwrap_or(""))
                );
            }
        }
    }
}

/// `map <sn>` — the newest stored map's identity and frame.
pub async fn show_map(state: &AppState, sn: &str) {
    let summary = match queries::latest_map_summary(&state.db, sn).await {
        Ok(summary) => summary,
        Err(error) => return println!("map lookup failed: {error}"),
    };
    let Some((
        map_id,
        path_id,
        width,
        height,
        resolution,
        x_min,
        y_min,
        dock_x,
        dock_y,
        dock_state,
        areas_json,
        received_ms,
    )) = summary
    else {
        return println!("{sn}: no maps stored");
    };
    println!("sn            {sn}");
    println!("map / path    {} / {}", show(map_id), show(path_id));
    println!("dims          {}x{} cells", show(width), show(height));
    println!(
        "resolution    {}",
        resolution
            .map(|value| format!("{value:.3} m/cell"))
            .unwrap_or_else(|| "-".to_string())
    );
    println!(
        "origin        ({}, {}) m",
        x_min.map(|v| format!("{v:.2}")).unwrap_or("?".into()),
        y_min.map(|v| format!("{v:.2}")).unwrap_or("?".into())
    );
    println!(
        "dock          ({}, {}) mm  state {}",
        show(dock_x),
        show(dock_y),
        dock_state.as_deref().unwrap_or("-")
    );
    println!("areas         {}", areas_json.as_deref().unwrap_or("[]"));
    println!("received      {}", ts(received_ms));
}

/// `path <sn> [<path_id>]` — one assembled clean path.
pub async fn show_path(state: &AppState, sn: &str, path_id: Option<i64>) {
    let summary = match queries::clean_path_summary(&state.db, sn, path_id).await {
        Ok(summary) => summary,
        Err(error) => return println!("path lookup failed: {error}"),
    };
    let Some((path_id, points_json, total_points, complete, updated_ms)) = summary else {
        return println!("{sn}: no stored clean path");
    };
    println!("sn            {sn}");
    println!("path          {path_id}");
    println!(
        "points        {} stored / {total_points} reported",
        points_stored(&points_json)
    );
    println!("complete      {}", if complete != 0 { "yes" } else { "no" });
    println!("updated       {}", ts(updated_ms));
}

/// `commands [<sn>] [n]` — the queue, oldest of the batch first.
pub async fn show_commands(state: &AppState, sn: Option<&str>, limit: i64) {
    match queries::recent_commands(&state.db, sn, limit).await {
        Err(error) => println!("could not read the command queue: {error}"),
        Ok(commands) if commands.is_empty() => println!("no commands"),
        Ok(commands) => {
            for command in commands.iter().rev() {
                let mut line = format!(
                    "#{:<5} {:<8} {:<6} enc:{}  {:<20} created {}",
                    command.id,
                    command.state,
                    command.info_type,
                    command.encrypt,
                    command.sn,
                    ago(command.created_ms)
                );
                if let Some(sent_ms) = command.sent_ms {
                    line += &format!("  sent {}", ago(sent_ms));
                }
                if let Some(ack_ms) = command.ack_ms {
                    line += &format!("  acked {}", ago(ack_ms));
                }
                if let Some(error) = &command.error {
                    line += &format!("  error: {error}");
                }
                if let Some(trace_ref) = &command.trace_ref {
                    line += &format!("  {trace_ref}");
                }
                println!("{line}");
            }
        }
    }
}

/// `send` / `send-enc` / the CLI's `send` — enqueue; the poller does the rest.
pub async fn enqueue(state: &AppState, sn: &str, info_type: i64, data: Value, encrypt: bool) {
    let payload = match serde_json::to_string(&data) {
        Ok(payload) => payload,
        Err(error) => return println!("payload does not serialize: {error}"),
    };
    match queries::insert_command(&state.db, sn, info_type, &payload, i64::from(encrypt)).await {
        Err(error) => println!("could not enqueue: {error}"),
        Ok(id) => {
            let online = if state.registry.is_online(sn) {
                "online"
            } else {
                "offline; it will go when the device connects"
            };
            println!(
                "queued #{id}  {info_type} -> {sn}  encrypt:{}  ({online}, ttl {}s)",
                i64::from(encrypt),
                state.config.gateway.command_ttl_secs
            );
            if encrypt && !state.config.gateway.encrypt_commands {
                println!("note: gateway.encrypt_commands is off, so this one will fail");
            }
        }
    }
}

/// `send-full` — one frame right now, bypassing the queue. An experiment tool; the
/// queue is what real sends use.
async fn send_now(state: &AppState, sn: &str, label: &str, frame: Value) {
    match state.registry.get(sn) {
        Some(handle) => {
            match handle
                .tx
                .send(crate::channel_b::registry::Outbound::frame(frame))
                .await
            {
                Ok(()) => println!("sent {label} to {sn} now (bypasses the queue)"),
                Err(_) => println!("{sn} is gone (writer closed)"),
            }
        }
        None => println!("{sn} is not online"),
    }
}

fn ts(ms: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms)
        .map(|time| time.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| ms.to_string())
}

fn ago(ms: i64) -> String {
    let delta = crate::db::now_ms() - ms;
    if delta < 0 {
        return "just now".to_string();
    }
    let secs = delta / 1000;
    match secs {
        0..60 => format!("{secs}s ago"),
        60..3600 => format!("{}m ago", secs / 60),
        3600..86400 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86400),
    }
}

fn show<T: std::fmt::Display>(value: Option<T>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn preview(text: &str) -> String {
    if text.chars().count() <= PREVIEW {
        text.to_string()
    } else {
        text.chars().take(PREVIEW).collect::<String>() + "…"
    }
}

/// How many points the stored JSON actually carries (sparse assemblies have holes).
fn points_stored(points_json: &str) -> usize {
    serde_json::from_str::<Vec<Value>>(points_json)
        .map(|points| points.iter().filter(|point| !point.is_null()).count())
        .unwrap_or(0)
}

#[derive(Debug, PartialEq)]
enum Command {
    Empty,
    Help,
    Devices,
    Device {
        sn: String,
    },
    Events {
        sn: Option<String>,
        limit: i64,
    },
    Map {
        sn: String,
    },
    Path {
        sn: String,
        path_id: Option<i64>,
    },
    Commands {
        sn: Option<String>,
        limit: i64,
    },
    Send {
        sn: String,
        info_type: i64,
        data: Value,
    },
    SendEncrypted {
        sn: String,
        info_type: i64,
        data: Value,
    },
    SendFull {
        sn: String,
        frame: Value,
    },
    Trace {
        on: bool,
    },
    Style {
        styled: bool,
    },
    Pongs {
        on: bool,
    },
    Quit,
    Unknown(String),
}

fn parse(line: &str) -> Command {
    let line = line.trim();
    let mut parts = line.splitn(4, char::is_whitespace);
    match parts.next().unwrap_or_default() {
        "" => Command::Empty,
        "help" => Command::Help,
        "devices" => Command::Devices,
        "quit" => Command::Quit,
        "device" => match parts.next() {
            Some(sn) if !sn.is_empty() => Command::Device { sn: sn.to_string() },
            _ => Command::Unknown(line.to_string()),
        },
        "events" => {
            let rest: Vec<&str> = parts.collect();
            match filter_limit(&rest, EVENTS_DEFAULT) {
                Some((sn, limit)) => Command::Events { sn, limit },
                None => Command::Unknown(line.to_string()),
            }
        }
        "commands" => {
            let rest: Vec<&str> = parts.collect();
            match filter_limit(&rest, COMMANDS_DEFAULT) {
                Some((sn, limit)) => Command::Commands { sn, limit },
                None => Command::Unknown(line.to_string()),
            }
        }
        "map" => match parts.next() {
            Some(sn) if !sn.is_empty() => Command::Map { sn: sn.to_string() },
            _ => Command::Unknown(line.to_string()),
        },
        "path" => match (parts.next(), parts.next()) {
            (Some(sn), None) if !sn.is_empty() => Command::Path {
                sn: sn.to_string(),
                path_id: None,
            },
            (Some(sn), Some(path_id)) if !sn.is_empty() => match path_id.parse() {
                Ok(path_id) => Command::Path {
                    sn: sn.to_string(),
                    path_id: Some(path_id),
                },
                Err(_) => Command::Unknown(line.to_string()),
            },
            _ => Command::Unknown(line.to_string()),
        },
        "send" => match send_fields(parts) {
            Some((sn, info_type, data)) => Command::Send {
                sn,
                info_type,
                data,
            },
            None => Command::Unknown(line.to_string()),
        },
        "send-enc" => match send_fields(parts) {
            Some((sn, info_type, data)) => Command::SendEncrypted {
                sn,
                info_type,
                data,
            },
            None => Command::Unknown(line.to_string()),
        },
        "send-full" => {
            // Re-split with a limit of three: the frame is the whole rest of the line.
            let mut whole = line.splitn(3, char::is_whitespace);
            whole.next();
            match (whole.next(), whole.next()) {
                (Some(sn), Some(text)) => match serde_json::from_str(text) {
                    Ok(frame) => Command::SendFull {
                        sn: sn.to_string(),
                        frame,
                    },
                    Err(_) => Command::Unknown(line.to_string()),
                },
                _ => Command::Unknown(line.to_string()),
            }
        }
        "trace" => match parts.next() {
            Some("on") => Command::Trace { on: true },
            Some("off") => Command::Trace { on: false },
            _ => Command::Unknown(line.to_string()),
        },
        "style" => match parts.next() {
            Some("styled") => Command::Style { styled: true },
            Some("compact") => Command::Style { styled: false },
            _ => Command::Unknown(line.to_string()),
        },
        "pongs" => match parts.next() {
            Some("on") => Command::Pongs { on: true },
            Some("off") => Command::Pongs { on: false },
            _ => Command::Unknown(line.to_string()),
        },
        _ => Command::Unknown(line.to_string()),
    }
}

/// `[<sn>] [n]`: a lone integer token is the limit, a lone non-integer is the
/// device, and `"<sn> <n>"` is both.
fn filter_limit(rest: &[&str], default: i64) -> Option<(Option<String>, i64)> {
    match rest {
        [] => Some((None, default)),
        [one] => match one.parse::<i64>() {
            Ok(limit) => Some((None, limit)),
            Err(_) => Some((Some(one.to_string()), default)),
        },
        [sn, limit] => match limit.parse::<i64>() {
            Ok(limit) => Some((Some(sn.to_string()), limit)),
            Err(_) => None,
        },
        _ => None,
    }
}

fn send_fields<'a>(mut parts: impl Iterator<Item = &'a str>) -> Option<(String, i64, Value)> {
    let sn = parts.next()?;
    let info_type = parts.next()?.parse().ok()?;
    let data = match parts.next() {
        None => json!({}),
        Some(text) => serde_json::from_str(text).ok()?,
    };
    Some((sn.to_string(), info_type, data))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_send_with_json_that_contains_spaces() {
        assert_eq!(
            parse("send LSLDSM7PRO20403551 21003 {\"area\": [{\"id\": 1}]}"),
            Command::Send {
                sn: "LSLDSM7PRO20403551".into(),
                info_type: 21003,
                data: json!({"area": [{"id": 1}]}),
            }
        );
    }

    #[test]
    fn a_missing_payload_defaults_to_an_empty_object() {
        assert_eq!(
            parse("send SN 20001"),
            Command::Send {
                sn: "SN".into(),
                info_type: 20001,
                data: json!({})
            }
        );
    }

    #[test]
    fn malformed_sends_are_reported_not_guessed() {
        assert_eq!(
            parse("send SN not-a-number {}"),
            Command::Unknown("send SN not-a-number {}".into())
        );
        assert_eq!(
            parse("send SN 20001 {oops"),
            Command::Unknown("send SN 20001 {oops".into())
        );
        assert_eq!(parse("send SN"), Command::Unknown("send SN".into()));
        assert_eq!(
            parse("send-enc SN nope {}"),
            Command::Unknown("send-enc SN nope {}".into())
        );
    }

    #[test]
    fn parses_an_encrypted_send_and_the_style_toggle() {
        assert_eq!(
            parse("send-enc SN 21024 {\"cmd\": \"setledswitch\", \"value\": 0}"),
            Command::SendEncrypted {
                sn: "SN".into(),
                info_type: 21024,
                data: json!({"cmd": "setledswitch", "value": 0}),
            }
        );
        assert_eq!(parse("style styled"), Command::Style { styled: true });
        assert_eq!(parse("style compact"), Command::Style { styled: false });
        assert_eq!(
            parse("style sideways"),
            Command::Unknown("style sideways".into())
        );
        assert_eq!(parse("pongs on"), Command::Pongs { on: true });
        assert_eq!(parse("pongs off"), Command::Pongs { on: false });
        assert_eq!(parse("pongs maybe"), Command::Unknown("pongs maybe".into()));
    }

    #[test]
    fn parses_the_state_verbs() {
        assert_eq!(parse("devices"), Command::Devices);
        assert_eq!(parse("device SN"), Command::Device { sn: "SN".into() });
        assert_eq!(parse("device"), Command::Unknown("device".into()));
        assert_eq!(
            parse("events"),
            Command::Events {
                sn: None,
                limit: 20
            }
        );
        assert_eq!(
            parse("events SN"),
            Command::Events {
                sn: Some("SN".into()),
                limit: 20
            }
        );
        assert_eq!(parse("events 5"), Command::Events { sn: None, limit: 5 });
        assert_eq!(
            parse("events SN 5"),
            Command::Events {
                sn: Some("SN".into()),
                limit: 5
            }
        );
        assert_eq!(
            parse("events SN not-a-number"),
            Command::Unknown("events SN not-a-number".into())
        );
        assert_eq!(parse("map SN"), Command::Map { sn: "SN".into() });
        assert_eq!(parse("map"), Command::Unknown("map".into()));
        assert_eq!(
            parse("path SN"),
            Command::Path {
                sn: "SN".into(),
                path_id: None
            }
        );
        assert_eq!(
            parse("path SN 123"),
            Command::Path {
                sn: "SN".into(),
                path_id: Some(123)
            }
        );
        assert_eq!(parse("path SN x"), Command::Unknown("path SN x".into()));
        assert_eq!(
            parse("commands"),
            Command::Commands {
                sn: None,
                limit: 20
            }
        );
        assert_eq!(
            parse("commands SN 3"),
            Command::Commands {
                sn: Some("SN".into()),
                limit: 3
            }
        );
        assert_eq!(parse("trace on"), Command::Trace { on: true });
        assert_eq!(parse("trace off"), Command::Trace { on: false });
        assert_eq!(
            parse("trace sideways"),
            Command::Unknown("trace sideways".into())
        );
    }
}
