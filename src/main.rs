//! `noobscenic` — command line entry point.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;
use tracing_subscriber::{fmt, EnvFilter};

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
    /// Serve channel A (and, from phase 3, the gateway). The default.
    Serve,
    /// Create or migrate the database, then exit.
    Migrate,
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

/// stderr always; a daily-rolled file too, unless it is switched off.
fn init_tracing(config: &Config) -> std::io::Result<Option<tracing_appender::non_blocking::WorkerGuard>> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(config.logging.level.clone()));
    let stderr = fmt::layer().with_writer(std::io::stderr).with_target(false);

    let Some(path) = config.log_file() else {
        tracing_subscriber::registry().with(filter).with(stderr).init();
        return Ok(None);
    };

    let dir = path.parent().unwrap_or(std::path::Path::new(".")).to_path_buf();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "noobscenic.log".to_string());
    std::fs::create_dir_all(&dir)?;

    let (writer, guard) = tracing_appender::non_blocking(tracing_appender::rolling::daily(dir, name));
    tracing_subscriber::registry()
        .with(filter)
        .with(stderr)
        .with(fmt::layer().with_writer(writer).with_ansi(false))
        .init();
    Ok(Some(guard))
}
