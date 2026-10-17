//! JSON configuration — see doc/PLAN.md §6.
//!
//! Defaults are the documented ones, so a missing config file is a working config.
//! Paths left unset are derived from `data_dir`, which is why they are `Option`s here
//! rather than eagerly-defaulted strings: `--data-dir` should move the database, the
//! log and the traces together without the operator restating all three.
//!
//! Unknown keys are rejected. Permissiveness is a rule about what the *device* sends
//! us (doc/PLAN.md §2); a typo in the operator's own config file is worth reporting.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub data_dir: PathBuf,
    pub database: Database,
    pub http: Http,
    pub gateway: Gateway,
    pub console: Console,
    /// The account to drop root to on POSIX; `None` stays as started. Only acted on
    /// with the `posix-runas` feature — see doc/PLAN.md §6.
    pub runas: Option<RunAs>,
    pub registration: Registration,
    pub logging: Logging,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            data_dir: PathBuf::from("./var"),
            database: Database::default(),
            http: Http::default(),
            gateway: Gateway::default(),
            console: Console::default(),
            runas: None,
            registration: Registration::default(),
            logging: Logging::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Database {
    /// Each connection allocates its own page cache; two is plenty here and
    /// keeps the pool's RAM bounded (doc/ROUTER_BUILD.md).
    pub max_connections: u32,
}

impl Default for Database {
    fn default() -> Self {
        Database { max_connections: 2 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Http {
    pub bind: SocketAddr,
}

impl Default for Http {
    fn default() -> Self {
        Http {
            bind: "0.0.0.0:8080".parse().expect("valid default"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Gateway {
    pub bind: SocketAddr,
    /// What `getSockAddr` hands the robot. Must be reachable *from the robot*, which
    /// is rarely the bind address. `None` means auto-detect from the requesting peer.
    pub advertise: Option<Vec<AdvertisedAddr>>,
    pub ping_timeout_secs: u64,
    pub max_frame_bytes: usize,
    pub ack_handshake: bool,
    /// Answer `21006` pings with `"isExistConnect":true`, telling the robot an
    /// app/cloud is online — that is what enables its status and map pushes
    /// (FUNC_STATUS.md §2.3). `false` pongs keep the link alive but silent.
    pub announce_app_online: bool,
    pub encrypt_commands: bool,
    pub command_ttl_secs: u64,
}

impl Default for Gateway {
    fn default() -> Self {
        Gateway {
            bind: "0.0.0.0:8081".parse().expect("valid default"),
            advertise: None,
            ping_timeout_secs: 120,
            max_frame_bytes: 8 * 1024 * 1024,
            ack_handshake: true,
            announce_app_online: true,
            encrypt_commands: false,
            command_ttl_secs: 300,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdvertisedAddr {
    pub ip: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Console {
    pub enabled: bool,
}

impl Default for Console {
    fn default() -> Self {
        Console { enabled: true }
    }
}

/// `getpwnam`-style account to run as (feature `posix-runas`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunAs {
    /// The account to become. Must exist in the system's passwd database.
    pub user: String,
    /// The group to become; `null` means the user's primary group, like the
    /// reference snippet. Must exist in the system's group database.
    pub group: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Registration {
    /// Adopt any vacuum that registers. The `sig` cannot be verified anyway.
    pub accept_all: bool,
    /// `None` = sessions never expire. Setting it makes the server answer `code:102`
    /// after the TTL, which is the documented way to force a re-register.
    pub session_ttl_secs: Option<u64>,
}

impl Default for Registration {
    fn default() -> Self {
        Registration {
            accept_all: true,
            session_ttl_secs: None,
        }
    }
}

/// Where application logs go. A path overrides the derived location and `false`
/// disables file logging; omitting the field (or `null`) uses the derived path.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LogFile {
    Path(PathBuf),
    Enabled(bool),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Logging {
    pub level: String,
    pub file: Option<LogFile>,
    pub wire: Wire,
}

impl Default for Logging {
    fn default() -> Self {
        Logging {
            level: "info".to_string(),
            file: None,
            wire: Wire::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WireFormat {
    Jsonl,
    Raw,
    Both,
}

impl WireFormat {
    pub fn jsonl(self) -> bool {
        matches!(self, WireFormat::Jsonl | WireFormat::Both)
    }

    pub fn raw(self) -> bool {
        matches!(self, WireFormat::Raw | WireFormat::Both)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Wire {
    pub enabled: bool,
    /// Defaults to `<data_dir>/traces`.
    pub dir: Option<PathBuf>,
    pub format: WireFormat,
    /// Applies to the JSONL sink only. The raw sink is never truncated — the whole
    /// point of it is that it is verbatim.
    pub max_body_bytes: usize,
    /// `0` = never rotate.
    pub rotate_mb: u64,
    /// `0` = keep forever.
    pub retain_days: u64,
    /// Masks `session`, `cookies` and `sig` in the JSONL sink. Never in the raw sink.
    pub redact_secrets: bool,
}

impl Default for Wire {
    fn default() -> Self {
        Wire {
            enabled: true,
            dir: None,
            format: WireFormat::Both,
            max_body_bytes: 256 * 1024,
            rotate_mb: 64,
            retain_days: 14,
            redact_secrets: false,
        }
    }
}

/// Command-line values that win over the file.
#[derive(Debug, Default, Clone)]
pub struct Overrides {
    pub data_dir: Option<PathBuf>,
    pub http_bind: Option<SocketAddr>,
    pub log_level: Option<String>,
    pub no_wire_trace: bool,
}

impl Config {
    /// Load from `path`, or from `./config.json` if it exists, or use the defaults.
    ///
    /// An explicitly requested file that does not exist is an error; the implicit one
    /// silently falls back to defaults.
    pub fn load(path: Option<&Path>, overrides: &Overrides) -> Result<Config> {
        let explicit = path.is_some();
        let path = path.unwrap_or_else(|| Path::new("config.json"));

        let mut config = if path.exists() {
            let text = std::fs::read_to_string(path).map_err(|source| Error::ConfigRead {
                path: path.to_path_buf(),
                source,
            })?;
            serde_json::from_str(&text).map_err(|source| Error::ConfigParse {
                path: path.to_path_buf(),
                source,
            })?
        } else if explicit {
            return Err(Error::config(format!("{} does not exist", path.display())));
        } else {
            Config::default()
        };

        config.apply(overrides);
        config.validate()?;
        Ok(config)
    }

    fn apply(&mut self, overrides: &Overrides) {
        if let Some(dir) = &overrides.data_dir {
            self.data_dir = dir.clone();
        }
        if let Some(bind) = overrides.http_bind {
            self.http.bind = bind;
        }
        if let Some(level) = &overrides.log_level {
            self.logging.level = level.clone();
        }
        if overrides.no_wire_trace {
            self.logging.wire.enabled = false;
        }
    }

    fn validate(&self) -> Result<()> {
        if self.database.max_connections == 0 {
            return Err(Error::config("database.max_connections must be at least 1"));
        }
        if self.gateway.max_frame_bytes < 4096 {
            return Err(Error::config(
                "gateway.max_frame_bytes is implausibly small",
            ));
        }
        if let Some(list) = &self.gateway.advertise
            && list.is_empty()
        {
            return Err(Error::config(
                "gateway.advertise is an empty list; use null for auto-detect",
            ));
        }
        Ok(())
    }

    /// The SQLite file always follows `data_dir`, so state moves as one unit.
    pub fn database_url(&self) -> String {
        format!("sqlite://{}", self.data_dir.join("noobscenic.db").display())
    }

    pub fn log_file(&self) -> Option<PathBuf> {
        match &self.logging.file {
            None | Some(LogFile::Enabled(true)) => {
                Some(self.data_dir.join("logs").join("noobscenic.log"))
            }
            Some(LogFile::Enabled(false)) => None,
            Some(LogFile::Path(path)) => Some(path.clone()),
        }
    }

    pub fn wire_dir(&self) -> PathBuf {
        self.logging
            .wire
            .dir
            .clone()
            .unwrap_or_else(|| self.data_dir.join("traces"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_documented_ones() {
        let config = Config::default();
        assert_eq!(config.http.bind.port(), 8080);
        assert_eq!(config.gateway.bind.port(), 8081);
        assert_eq!(
            sqlite_path(&config.database_url()),
            Path::new("./var").join("noobscenic.db")
        );
        assert_eq!(config.wire_dir(), Path::new("./var").join("traces"));
        assert!(config.registration.accept_all);
        assert!(config.gateway.ack_handshake);
        assert!(config.gateway.announce_app_online);
    }

    #[test]
    fn data_dir_moves_the_derived_paths_together() {
        let mut config = Config::default();
        config.apply(&Overrides {
            data_dir: Some("/srv/nb".into()),
            ..Default::default()
        });
        assert_eq!(
            sqlite_path(&config.database_url()),
            Path::new("/srv/nb").join("noobscenic.db")
        );
        assert_eq!(config.wire_dir(), Path::new("/srv/nb").join("traces"));
        assert_eq!(
            config.log_file(),
            Some(Path::new("/srv/nb").join("logs/noobscenic.log"))
        );
    }

    #[test]
    fn explicit_paths_survive_a_data_dir_override() {
        let mut config = Config::default();
        config.logging.wire.dir = Some("/var/log/nb-traces".into());
        config.apply(&Overrides {
            data_dir: Some("/srv/nb".into()),
            ..Default::default()
        });
        assert_eq!(config.wire_dir(), PathBuf::from("/var/log/nb-traces"));
    }

    #[test]
    fn log_file_can_be_derived_overridden_or_disabled() {
        let default: Config = serde_json::from_str(r#"{"logging": {"file": null}}"#).unwrap();
        assert_eq!(
            default.log_file(),
            Some(Path::new("./var").join("logs/noobscenic.log"))
        );

        let custom: Config =
            serde_json::from_str(r#"{"logging": {"file": "/var/log/nb.log"}}"#).unwrap();
        assert_eq!(custom.log_file(), Some(PathBuf::from("/var/log/nb.log")));

        let disabled: Config = serde_json::from_str(r#"{"logging": {"file": false}}"#).unwrap();
        assert_eq!(disabled.log_file(), None);
    }

    #[test]
    fn a_typo_in_the_config_is_reported() {
        let err = serde_json::from_str::<Config>(r#"{"data_dirr": "./x"}"#).unwrap_err();
        assert!(err.to_string().contains("data_dirr"), "{err}");
    }

    #[test]
    fn the_shipped_example_parses() {
        let config: Config =
            serde_json::from_str(include_str!("../config.example.json")).expect("example parses");
        assert_eq!(config.data_dir, PathBuf::from("./var"));
        assert_eq!(config.http.bind.port(), 8080);
        assert_eq!(config.gateway.bind.port(), 8081);
    }

    #[test]
    fn unimplemented_placeholders_are_rejected() {
        for (key, text) in [
            ("url", r#"{"database": {"url": null}}"#),
            ("tls", r#"{"http": {"tls": null}}"#),
            ("ota", r#"{"ota": {"enabled": false}}"#),
        ] {
            let err =
                serde_json::from_str::<Config>(text).expect_err("placeholder must be removed");
            assert!(err.to_string().contains(key), "{err}");
        }
    }

    #[test]
    fn runas_is_optional_and_group_may_be_left_out() {
        let config: Config = serde_json::from_str(r#"{"runas": {"user": "nobody"}}"#)
            .expect("runas without a group parses");
        let runas = config.runas.expect("runas parsed");
        assert_eq!(runas.user, "nobody");
        assert_eq!(runas.group, None);
        assert!(Config::default().runas.is_none());
    }

    #[test]
    fn runas_without_a_user_is_rejected() {
        let err = serde_json::from_str::<Config>(r#"{"runas": {"group": "nobody"}}"#)
            .expect_err("a group without a user is a typo");
        assert!(err.to_string().contains("user"), "{err}");
    }

    #[test]
    fn a_typo_inside_runas_is_reported() {
        let err = serde_json::from_str::<Config>(r#"{"runas": {"username": "nobody"}}"#)
            .expect_err("an unknown runas key is a typo");
        assert!(err.to_string().contains("username"), "{err}");
    }

    /// `sqlite://<path>` as a `PathBuf`, so the tests do not care about the
    /// platform's separator.
    fn sqlite_path(url: &str) -> PathBuf {
        PathBuf::from(url.strip_prefix("sqlite://").expect("sqlite URL"))
    }
}
