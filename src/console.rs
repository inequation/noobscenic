//! The operator console — doc/PLAN.md §12. Minimal for now: see who is online and
//! push one command frame, which is what the later phases build on.
//!
//! Reads lines from stdin when it is a terminal, and does nothing at all otherwise,
//! so the server stays headless whenever nobody can type into it.

use std::io::IsTerminal;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::watch;

use crate::AppState;

const HELP: &str = "\
commands:
  devices                          who is connected on channel B
  send <sn> <infoType> <json>      queue one cloud->device frame (encrypt:0)
  send-enc <sn> <infoType> <json>  same, but encrypt:1 with the device's session key
  send-full <sn> <json>            send the given JSON object as the whole frame
  style compact|styled             how channel-B frames are written
  pongs on|off                     answer 21006 pings (liveness experiment)
  quit                             shut the server down";

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
                Command::Devices => {
                    let online = state.registry.online();
                    if online.is_empty() {
                        println!("no devices online");
                    }
                    for device in online {
                        println!(
                            "{}  {}  since {}",
                            device.conn_id, device.peer, device.connected_ms
                        );
                    }
                }
                Command::Send {
                    sn,
                    info_type,
                    data,
                } => {
                    let frame = json!({"infoType": info_type, "encrypt": 0, "data": data});
                    send(&state, &sn, frame, &format!("infoType {info_type}")).await;
                }
                Command::SendEncrypted {
                    sn,
                    info_type,
                    data,
                } => match crate::db::queries::latest_session_key(&state.db, &sn).await {
                    Ok(Some(key)) => {
                        match crate::channel_b::crypto::encrypt_command(info_type, &data, &key) {
                            Some(frame) => {
                                send(
                                    &state,
                                    &sn,
                                    frame,
                                    &format!("infoType {info_type} (encrypt:1)"),
                                )
                                .await;
                            }
                            None => println!("{sn}: the session key is too short for AES-128"),
                        }
                    }
                    Ok(None) => println!("{sn}: no live session to encrypt with"),
                    Err(error) => println!("{sn}: session lookup failed: {error}"),
                },
                Command::Style { styled } => {
                    state
                        .frame_style
                        .store(styled, std::sync::atomic::Ordering::Relaxed);
                    println!(
                        "channel-B frame style: {}",
                        if styled { "styled" } else { "compact" }
                    );
                }
                Command::SendFull { sn, frame } => {
                    send(&state, &sn, frame, "frame").await;
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

async fn send(state: &AppState, sn: &str, frame: Value, label: &str) {
    match state.registry.get(sn) {
        Some(handle) => match handle.tx.send(frame).await {
            Ok(()) => println!("queued {label} for {sn}"),
            Err(_) => println!("{sn} is gone (writer closed)"),
        },
        None => println!("{sn} is not online"),
    }
}

#[derive(Debug, PartialEq)]
enum Command {
    Empty,
    Help,
    Devices,
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
        assert_eq!(
            parse(
                "send-full SN {\"infoType\": 20001, \"connectionType\": 1, \"encrypt\": 0, \"data\": {}}"
            ),
            Command::SendFull {
                sn: "SN".into(),
                frame: json!({"infoType": 20001, "connectionType": 1, "encrypt": 0, "data": {}}),
            }
        );
    }

    #[test]
    fn the_other_verbs_parse() {
        assert_eq!(parse("  "), Command::Empty);
        assert_eq!(parse("help"), Command::Help);
        assert_eq!(parse("devices"), Command::Devices);
        assert_eq!(parse("quit"), Command::Quit);
        assert_eq!(parse("frobnicate"), Command::Unknown("frobnicate".into()));
    }
}
