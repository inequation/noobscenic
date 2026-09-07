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
    pub registration: Registration,
    pub ota: Ota,
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
            registration: Registration::default(),
            ota: Ota::default(),
            logging: Logging::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Database {
    /// Defaults to `sqlite://<data_dir>/noobscenic.db`.
    pub url: Option<String>,
    pub max_connections: u32,
}

impl Default for Database {
    fn default() -> Self {
        Database { url: None, max_connections: 5 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Http {
    pub bind: SocketAddr,
    /// Reserved for a DNS-override deployment; the robot validates no certificates,
    /// so plain HTTP with `setUrl` is the v1 path (doc/PLAN.md §6).
    pub tls: Option<serde_json::Value>,
}

impl Default for Http {
    fn default() -> Self {
        Http { bind: "0.0.0.0:8080".parse().expect("valid default"), tls: None }
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
        Registration { accept_all: true, session_ttl_secs: None }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Ota {
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Logging {
    pub level: String,
    /// Defaults to `<data_dir>/logs/noobscenic.log`; `null` in a file disables it only
    /// if `file_enabled` is false — see `log_file()`.
    pub file: Option<PathBuf>,
    pub file_enabled: bool,
    pub wire: Wire,
}

impl Default for Logging {
    fn default() -> Self {
        Logging {
            level: "info".to_string(),
            file: None,
            file_enabled: true,
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
            let text = std::fs::read_to_string(path)
                .map_err(|source| Error::ConfigRead { path: path.to_path_buf(), source })?;
            serde_json::from_str(&text)
                .map_err(|source| Error::ConfigParse { path: path.to_path_buf(), source })?
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
            return Err(Error::config("gateway.max_frame_bytes is implausibly small"));
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

    /// `sqlite://<data_dir>/noobscenic.db` unless overridden.
    pub fn database_url(&self) -> String {
        self.database.url.clone().unwrap_or_else(|| {
            format!("sqlite://{}", self.data_dir.join("noobscenic.db").display())
        })
    }

    pub fn log_file(&self) -> Option<PathBuf> {
        if !self.logging.file_enabled {
            return None;
        }
        Some(
            self.logging
                .file
                .clone()
                .unwrap_or_else(|| self.data_dir.join("logs").join("noobscenic.log")),
        )
    }

    pub fn wire_dir(&self) -> PathBuf {
        self.logging.wire.dir.clone().unwrap_or_else(|| self.data_dir.join("traces"))
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
        assert_eq!(config.database_url(), "sqlite://./var/noobscenic.db");
        assert_eq!(config.wire_dir(), PathBuf::from("./var/traces"));
        assert!(config.registration.accept_all);
    }

    #[test]
    fn data_dir_moves_the_derived_paths_together() {
        let mut config = Config::default();
        config.apply(&Overrides { data_dir: Some("/srv/nb".into()), ..Default::default() });
        assert_eq!(config.database_url(), "sqlite:///srv/nb/noobscenic.db");
        assert_eq!(config.wire_dir(), PathBuf::from("/srv/nb/traces"));
        assert_eq!(config.log_file(), Some(PathBuf::from("/srv/nb/logs/noobscenic.log")));
    }

    #[test]
    fn explicit_paths_survive_a_data_dir_override() {
        let mut config = Config::default();
        config.logging.wire.dir = Some("/var/log/nb-traces".into());
        config.apply(&Overrides { data_dir: Some("/srv/nb".into()), ..Default::default() });
        assert_eq!(config.wire_dir(), PathBuf::from("/var/log/nb-traces"));
    }

    #[test]
    fn a_typo_in_the_config_is_reported() {
        let err = serde_json::from_str::<Config>(r#"{"data_dirr": "./x"}"#).unwrap_err();
        assert!(err.to_string().contains("data_dirr"), "{err}");
    }

    #[test]
    fn the_documented_example_parses() {
        // The exact shape printed in doc/PLAN.md §6.
        let text = r#"{
          "data_dir": "./var",
          "database": { "url": "sqlite://./var/noobscenic.db", "max_connections": 5 },
          "http":    { "bind": "0.0.0.0:8080", "tls": null },
          "gateway": { "bind": "0.0.0.0:8081", "advertise": null,
                       "ping_timeout_secs": 120, "max_frame_bytes": 8388608,
                       "ack_handshake": true, "encrypt_commands": false,
                       "command_ttl_secs": 300 },
          "console": { "enabled": true },
          "registration": { "accept_all": true, "session_ttl_secs": null },
          "ota":     { "enabled": false },
          "logging": { "level": "info",
                       "file": "./var/logs/noobscenic.log",
                       "wire": { "enabled": true, "dir": "./var/traces", "format": "both",
                                 "max_body_bytes": 262144, "rotate_mb": 64,
                                 "retain_days": 14, "redact_secrets": false } }
        }"#;
        let config: Config = serde_json::from_str(text).expect("documented example must parse");
        assert_eq!(config.gateway.ping_timeout_secs, 120);
        assert!(config.logging.wire.format.jsonl() && config.logging.wire.format.raw());
    }
}
