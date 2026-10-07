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
                } => match state.registry.get(&sn) {
                    Some(handle) => {
                        let frame = json!({"infoType": info_type, "encrypt": 0, "data": data});
                        match handle.tx.send(frame).await {
                            Ok(()) => println!("queued infoType {info_type} for {sn}"),
                            Err(_) => println!("{sn} is gone (writer closed)"),
                        }
                    }
                    None => println!("{sn} is not online"),
                },
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
        "send" => {
            let (Some(sn), Some(info_type)) =
                (parts.next(), parts.next().and_then(|t| t.parse().ok()))
            else {
                return Command::Unknown(line.to_string());
            };
            let data = match parts.next() {
                None => json!({}),
                Some(text) => match serde_json::from_str(text) {
                    Ok(value) => value,
                    Err(_) => return Command::Unknown(line.to_string()),
                },
            };
            Command::Send {
                sn: sn.to_string(),
                info_type,
                data,
            }
        }
        _ => Command::Unknown(line.to_string()),
    }
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
