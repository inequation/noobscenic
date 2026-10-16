//! `noobscenic` — command line entry point.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use tracing_subscriber::filter::{LevelFilter, Targets};
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

use noobscenic::config::{Config, Overrides};

#[derive(Parser, Debug)]
#[command(
    name = "noobscenic",
    version,
    about = "A replacement cloud for the Proscenic M7 Pro robot vacuum."
)]
struct Cli {
    /// Configuration file (default: ./config.json if it exists).
    #[arg(short, long, value_name = "FILE", global = true)]
    config: Option<PathBuf>,

    /// Where the database, logs and traces live.
    #[arg(long, value_name = "DIR", global = true)]
    data_dir: Option<PathBuf>,

    /// Address for the channel-A HTTP listener.
    #[arg(long, value_name = "ADDR", global = true)]
    http_bind: Option<SocketAddr>,

    /// Log filter: a level, `target=level` pairs, or a bare target (trace).
    /// `RUST_LOG` overrides it; targets match by prefix, not regex.
    #[arg(long, value_name = "LEVEL", global = true)]
    log_level: Option<String>,

    /// Turn the wire tap off for this run.
    #[arg(long, global = true)]
    no_wire_trace: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Serve the channel-A HTTP API and the channel-B gateway. The default.
    Serve,
    /// Create or migrate the database, then exit.
    Migrate,
    /// Known devices: online state, last seen, version.
    #[cfg(feature = "console")]
    Devices,
    /// One device's stored row, latest session and connection state.
    #[cfg(feature = "console")]
    Device { sn: String },
    /// The newest semantic messages.
    #[cfg(all(feature = "console", feature = "history"))]
    Events {
        /// Device serial number (all devices unless given).
        sn: Option<String>,
        /// How many rows, newest first (default 20).
        n: Option<i64>,
    },
    /// The newest stored map: dims, origin, dock, areas.
    #[cfg(feature = "console")]
    Map { sn: String },
    /// An assembled clean path (the newest unless a path id is given).
    #[cfg(feature = "console")]
    Path { sn: String, path_id: Option<i64> },
    /// Enqueue a command for the gateway to push (encrypt:0 by default).
    #[cfg(feature = "console")]
    Send {
        sn: String,
        info_type: i64,
        /// Payload object, e.g. '{"cmd":"start"}'. Defaults to {}.
        json: Option<String>,
        /// Enqueue with encrypt:1 (needs gateway.encrypt_commands).
        #[arg(long)]
        encrypt: bool,
    },
    /// Queue state: pending/sent/acked/expired/failed.
    #[cfg(feature = "console")]
    Commands {
        /// Device serial number (all devices unless given).
        sn: Option<String>,
        /// How many rows, newest first (default 20).
        n: Option<i64>,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let overrides = Overrides {
        data_dir: cli.data_dir.clone(),
        http_bind: cli.http_bind,
        log_level: cli.log_level.clone(),
        no_wire_trace: cli.no_wire_trace,
    };

    let config = match Config::load(cli.config.as_deref(), &overrides) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("noobscenic: {error}");
            return ExitCode::FAILURE;
        }
    };

    // Held for as long as the process runs: dropping it stops the file writer.
    let _log_guard = match init_tracing(&config) {
        Ok(guard) => guard,
        Err(error) => {
            eprintln!("noobscenic: could not set up logging: {error}");
            return ExitCode::FAILURE;
        }
    };

    let result = match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => noobscenic::run(config).await,
        Command::Migrate => match noobscenic::start(config).await {
            Ok(state) => {
                println!("database is up to date");
                state.db.close().await;
                Ok(())
            }
            Err(error) => Err(error),
        },
        #[cfg(feature = "console")]
        verb => run_verb(config, verb).await,
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "fatal");
            eprintln!("noobscenic: {error}");
            ExitCode::FAILURE
        }
    }
}

/// A one-shot CLI run has no gateway of its own, so "online" can only mean "the
/// server process you are talking to"; the console sees the live state.
#[cfg(feature = "console")]
const ONE_SHOT_NOTE: &str =
    "one-shot run: `online` is this process's own gateway; the console shows the live state";

/// One-shot verbs run against the database directly (doc/PLAN.md §12): a `send` from
/// a second terminal lands in the `commands` table, which a running gateway drains.
#[cfg(feature = "console")]
async fn run_verb(config: Config, verb: Command) -> noobscenic::error::Result<()> {
    use noobscenic::console;

    let state = noobscenic::start(config).await?;
    match verb {
        Command::Devices => {
            console::show_devices(&state).await;
            println!("({ONE_SHOT_NOTE})");
        }
        Command::Device { sn } => {
            console::show_device(&state, &sn).await;
            println!("({ONE_SHOT_NOTE})");
        }
        #[cfg(feature = "history")]
        Command::Events { sn, n } => {
            console::show_events(&state, sn.as_deref(), n.unwrap_or(20)).await
        }
        Command::Map { sn } => console::show_map(&state, &sn).await,
        Command::Path { sn, path_id } => console::show_path(&state, &sn, path_id).await,
        Command::Send {
            sn,
            info_type,
            json,
            encrypt,
        } => {
            let data = match &json {
                Some(text) => serde_json::from_str(text)?,
                None => serde_json::json!({}),
            };
            console::enqueue(&state, &sn, info_type, data, encrypt).await;
        }
        Command::Commands { sn, n } => {
            console::show_commands(&state, sn.as_deref(), n.unwrap_or(20)).await
        }
        Command::Serve | Command::Migrate => unreachable!("handled before run_verb"),
    }
    state.db.close().await;
    Ok(())
}

/// The log filter: `RUST_LOG` if it has a usable directive, else
/// `logging.level`, else `info`.
fn log_filter(config: &Config) -> Targets {
    std::env::var("RUST_LOG")
        .ok()
        .and_then(|spec| parse_filter(&spec))
        .or_else(|| parse_filter(&config.logging.level))
        .unwrap_or_else(|| Targets::new().with_default(LevelFilter::INFO))
}

/// Parse an `EnvFilter`-style directive list without the regex machinery:
/// `debug` (default level), `sqlx=warn` (target), or a bare `sqlx` (trace).
/// Unlike `EnvFilter`, a target-only list keeps `info` as the default level.
fn parse_filter(spec: &str) -> Option<Targets> {
    let mut filter = Targets::new();
    let mut default = None;
    let mut parsed = false;
    for directive in spec.split(',') {
        let directive = directive.trim();
        if directive.is_empty() {
            continue;
        }
        match directive.split_once('=') {
            Some((target, level)) => match level.trim().parse::<LevelFilter>() {
                Ok(level) if !target.trim().is_empty() => {
                    filter = filter.with_target(target.trim(), level);
                    parsed = true;
                }
                _ => eprintln!("noobscenic: ignoring log directive {directive:?}"),
            },
            None => match directive.parse::<LevelFilter>() {
                Ok(level) => {
                    default = Some(level);
                    parsed = true;
                }
                Err(_) => {
                    filter = filter.with_target(directive, LevelFilter::TRACE);
                    parsed = true;
                }
            },
        }
    }
    parsed.then(|| filter.with_default(default.unwrap_or(LevelFilter::INFO)))
}

/// stderr always; a daily-rolled file too, unless it is switched off.
fn init_tracing(
    config: &Config,
) -> std::io::Result<Option<tracing_appender::non_blocking::WorkerGuard>> {
    let filter = log_filter(config);
    let stderr = fmt::layer().with_writer(std::io::stderr).with_target(false);

    let Some(path) = config.log_file() else {
        tracing_subscriber::registry()
            .with(filter)
            .with(stderr)
            .init();
        return Ok(None);
    };

    let dir = path
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .to_path_buf();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "noobscenic.log".to_string());
    std::fs::create_dir_all(&dir)?;

    let (writer, guard) =
        tracing_appender::non_blocking(tracing_appender::rolling::daily(dir, name));
    tracing_subscriber::registry()
        .with(filter)
        .with(stderr)
        .with(fmt::layer().with_writer(writer).with_ansi(false))
        .init();
    Ok(Some(guard))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing::Level;

    #[test]
    fn level_directive_sets_the_default() {
        let filter = parse_filter("debug").expect("a level parses");
        assert!(filter.would_enable("noobscenic::web", &Level::DEBUG));
        assert!(!filter.would_enable("noobscenic::web", &Level::TRACE));
    }

    #[test]
    fn target_directives_match_by_prefix() {
        let filter = parse_filter("noobscenic=trace,sqlx=warn").expect("targets parse");
        assert!(filter.would_enable("noobscenic::web", &Level::TRACE));
        assert!(filter.would_enable("sqlx::query", &Level::WARN));
        assert!(!filter.would_enable("sqlx::query", &Level::INFO));
        // No bare level: everything else keeps the info default.
        assert!(filter.would_enable("tokio", &Level::INFO));
        assert!(!filter.would_enable("tokio", &Level::DEBUG));
    }

    #[test]
    fn bare_target_means_trace() {
        let filter = parse_filter("sqlx").expect("a bare target parses");
        assert!(filter.would_enable("sqlx::query", &Level::TRACE));
        assert!(filter.would_enable("tokio", &Level::INFO));
    }

    #[test]
    fn junk_is_ignored_or_rejected() {
        assert!(parse_filter("").is_none());
        assert!(parse_filter("  ,  ").is_none());
        assert!(parse_filter("nonsense=level").is_none());
        let filter = parse_filter("nonsense=level,info").expect("the valid directive survives");
        assert!(filter.would_enable("noobscenic", &Level::INFO));
    }
}
