//! `noobscenic` — command line entry point.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;
use tracing_subscriber::{EnvFilter, fmt};

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

    /// Log filter, e.g. "debug" or "noobscenic=trace,sqlx=warn". RUST_LOG wins.
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
    Devices,
    /// One device's stored row, latest session and connection state.
    Device { sn: String },
    /// The newest semantic messages.
    Events {
        /// Device serial number (all devices unless given).
        sn: Option<String>,
        /// How many rows, newest first (default 20).
        n: Option<i64>,
    },
    /// The newest stored map: dims, origin, dock, areas.
    Map { sn: String },
    /// An assembled clean path (the newest unless a path id is given).
    Path { sn: String, path_id: Option<i64> },
    /// Enqueue a command for the gateway to push (encrypt:0 by default).
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
const ONE_SHOT_NOTE: &str =
    "one-shot run: `online` is this process's own gateway; the console shows the live state";

/// One-shot verbs run against the database directly (doc/PLAN.md §12): a `send` from
/// a second terminal lands in the `commands` table, which a running gateway drains.
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

/// stderr always; a daily-rolled file too, unless it is switched off.
fn init_tracing(
    config: &Config,
) -> std::io::Result<Option<tracing_appender::non_blocking::WorkerGuard>> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(config.logging.level.clone()));
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
