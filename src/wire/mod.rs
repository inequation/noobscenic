//! The wire tap — see doc/PLAN.md §8.
//!
//! Every byte the server reads from or writes to a device passes through here. The
//! JSONL sink is the queryable timeline; the raw sink is the verbatim copy. Database
//! rows keep a `trace_ref` of `"<file>#<seq>"` so any stored fact can be traced back
//! to the exact bytes that produced it.
//!
//! Writes are synchronous. At the volumes a household vacuum produces that is not
//! worth an extra thread and a channel, and it keeps the trace ordered and complete
//! up to the instant of a crash.

pub mod jsonl;
pub mod raw;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

use base64::Engine as _;
use serde_json::{Map, Value};

use crate::config::{Wire, WireFormat};
use crate::error::Result;

const REDACTED_KEYS: [&str; 3] = ["session", "cookies", "sig"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    In,
    Out,
}

impl Direction {
    fn as_str(self) -> &'static str {
        match self {
            Direction::In => "in",
            Direction::Out => "out",
        }
    }
}

/// One unit of traffic on its way to the trace.
pub struct Record<'a> {
    pub channel: &'static str,
    pub direction: Direction,
    pub kind: &'static str,
    pub conn_id: Option<String>,
    pub peer: Option<String>,
    pub sn: Option<String>,
    pub meta: Map<String, Value>,
    pub body: &'a [u8],
}

impl<'a> Record<'a> {
    pub fn new(
        channel: &'static str,
        direction: Direction,
        kind: &'static str,
        body: &'a [u8],
    ) -> Record<'a> {
        Record {
            channel,
            direction,
            kind,
            conn_id: None,
            peer: None,
            sn: None,
            meta: Map::new(),
            body,
        }
    }

    pub fn peer(mut self, peer: impl Into<String>) -> Self {
        self.peer = Some(peer.into());
        self
    }

    pub fn conn_id(mut self, conn_id: impl Into<String>) -> Self {
        self.conn_id = Some(conn_id.into());
        self
    }

    pub fn sn(mut self, sn: impl Into<String>) -> Self {
        self.sn = Some(sn.into());
        self
    }

    pub fn meta(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.meta.insert(key.to_string(), value.into());
        self
    }
}

/// What a recorded event can be referred to by, later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded {
    pub seq: u64,
    /// `"wire-2026-09-07.jsonl#1841"`, or just `"#1841"` when only the raw sink is on.
    pub reference: String,
}

pub struct Tap {
    inner: Option<Inner>,
}

struct Inner {
    seq: AtomicU64,
    enabled: AtomicBool,
    format: WireFormat,
    max_body_bytes: usize,
    redact: bool,
    jsonl: Mutex<jsonl::JsonlSink>,
    raw: raw::RawSink,
}

impl Tap {
    /// A tap that records nothing, at no cost.
    pub fn disabled() -> Tap {
        Tap { inner: None }
    }

    pub fn new(config: &Wire, dir: std::path::PathBuf) -> Result<Tap> {
        if !config.enabled {
            return Ok(Tap::disabled());
        }
        std::fs::create_dir_all(&dir)?;
        let raw_root = dir.join("raw");

        let pruned_jsonl = jsonl::prune(&dir, config.retain_days)?;
        let pruned_raw = raw::prune(&raw_root, config.retain_days)?;
        if pruned_jsonl + pruned_raw > 0 {
            tracing::info!(
                jsonl = pruned_jsonl,
                raw_dirs = pruned_raw,
                "pruned wire traces past the retention window"
            );
        }

        Ok(Tap {
            inner: Some(Inner {
                seq: AtomicU64::new(0),
                enabled: AtomicBool::new(true),
                format: config.format,
                max_body_bytes: config.max_body_bytes,
                redact: config.redact_secrets,
                jsonl: Mutex::new(jsonl::JsonlSink::new(dir, config.rotate_mb)),
                raw: raw::RawSink::new(raw_root),
            }),
        })
    }

    pub fn is_enabled(&self) -> bool {
        self.inner.as_ref().is_some_and(|i| i.enabled.load(Ordering::Relaxed))
    }

    /// Runtime toggle, for the console's `trace on|off`.
    pub fn set_enabled(&self, on: bool) {
        if let Some(inner) = &self.inner {
            inner.enabled.store(on, Ordering::Relaxed);
        }
    }

    /// Record one unit of traffic. Returns how to refer to it later.
    ///
    /// Sink failures are logged and swallowed: losing a trace line must never break
    /// the conversation with the robot.
    pub fn record(&self, record: Record<'_>) -> Option<Recorded> {
        let inner = self.inner.as_ref()?;
        if !inner.enabled.load(Ordering::Relaxed) {
            return None;
        }

        let seq = inner.seq.fetch_add(1, Ordering::Relaxed) + 1;
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let mut reference = format!("#{seq}");

        if inner.format.raw()
            && let Err(error) = inner.raw.write(&today, seq, record.kind, record.body)
        {
            tracing::warn!(%error, seq, "could not write the raw wire trace");
        }

        if inner.format.jsonl() {
            let line = inner.render(seq, &record);
            match inner.jsonl.lock() {
                Ok(mut sink) => {
                    if let Err(error) = sink.write_line(&today, &line) {
                        tracing::warn!(%error, seq, "could not write the JSONL wire trace");
                    } else {
                        reference = format!("{}#{seq}", sink.current_name());
                    }
                }
                Err(poisoned) => {
                    // A panic elsewhere poisoned the lock; keep tracing regardless.
                    let mut sink = poisoned.into_inner();
                    let _ = sink.write_line(&today, &line);
                }
            }
        }

        Some(Recorded { seq, reference })
    }
}

impl Inner {
    fn render(&self, seq: u64, record: &Record<'_>) -> String {
        let mut object = Map::new();
        object.insert("seq".into(), seq.into());
        object.insert(
            "ts".into(),
            chrono::Utc::now()
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
                .into(),
        );
        object.insert("channel".into(), record.channel.into());
        object.insert("direction".into(), record.direction.as_str().into());
        object.insert("kind".into(), record.kind.into());
        if let Some(conn_id) = &record.conn_id {
            object.insert("conn_id".into(), conn_id.clone().into());
        }
        if let Some(peer) = &record.peer {
            object.insert("peer".into(), peer.clone().into());
        }
        if let Some(sn) = &record.sn {
            object.insert("sn".into(), sn.clone().into());
        }
        for (key, value) in &record.meta {
            object.insert(key.clone(), value.clone());
        }

        object.insert("len".into(), record.body.len().into());
        let truncated = self.max_body_bytes > 0 && record.body.len() > self.max_body_bytes;
        let body = if truncated { &record.body[..self.max_body_bytes] } else { record.body };
        if truncated {
            object.insert("truncated".into(), true.into());
        }

        match std::str::from_utf8(body) {
            Ok(text) if self.redact => {
                object.insert("body".into(), redact_secrets(text).into());
            }
            Ok(text) => {
                object.insert("body".into(), text.into());
            }
            Err(_) => {
                object.insert(
                    "body_b64".into(),
                    base64::engine::general_purpose::STANDARD.encode(body).into(),
                );
            }
        }

        Value::Object(object).to_string()
    }
}

/// Mask session material in a trace line, in both the form-urlencoded and JSON shapes.
///
/// The raw sink never sees this — it is verbatim by definition.
fn redact_secrets(text: &str) -> String {
    let mut out = text.to_string();
    for key in REDACTED_KEYS {
        out = mask_form_value(&out, key);
        out = mask_json_value(&out, key);
    }
    out
}

/// `sig=abc&ts=0` -> `sig=***&ts=0`
fn mask_form_value(text: &str, key: &str) -> String {
    let needle = format!("{key}=");
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(&needle) {
        let starts_field = at == 0 || matches!(rest.as_bytes()[at - 1], b'&' | b'?');
        out.push_str(&rest[..at + needle.len()]);
        rest = &rest[at + needle.len()..];
        if !starts_field {
            continue;
        }
        let end = rest.find('&').unwrap_or(rest.len());
        if end > 0 {
            out.push_str("***");
            rest = &rest[end..];
        }
    }
    out.push_str(rest);
    out
}

/// `"session":"abc"` -> `"session":"***"`
fn mask_json_value(text: &str, key: &str) -> String {
    let needle = format!("\"{key}\"");
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(&needle) {
        out.push_str(&rest[..at + needle.len()]);
        rest = &rest[at + needle.len()..];

        let after = rest.trim_start();
        let gap = rest.len() - after.len();
        let Some(after) = after.strip_prefix(':') else { continue };
        let value = after.trim_start();
        let value_gap = after.len() - value.len();
        let Some(value) = value.strip_prefix('"') else { continue };
        let Some(end) = value.find('"') else { continue };

        out.push_str(&rest[..gap]);
        out.push(':');
        out.push_str(&after[..value_gap]);
        out.push_str("\"***\"");
        rest = &value[end + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_form_fields() {
        assert_eq!(
            mask_form_value("sn=ABC&sig=SECRET&ts=0", "sig"),
            "sn=ABC&sig=***&ts=0"
        );
        assert_eq!(mask_form_value("sig=SECRET", "sig"), "sig=***");
        // A field that merely ends in the key name is left alone.
        assert_eq!(mask_form_value("mysig=SECRET", "sig"), "mysig=SECRET");
    }

    #[test]
    fn masks_json_fields() {
        assert_eq!(
            mask_json_value(r#"{"session":"KEY","code":0}"#, "session"),
            r#"{"session":"***","code":0}"#
        );
        assert_eq!(
            mask_json_value(r#"{"cookies": "SID"}"#, "cookies"),
            r#"{"cookies": "***"}"#
        );
        // Non-string values are left alone rather than mangled.
        assert_eq!(mask_json_value(r#"{"session":0}"#, "session"), r#"{"session":0}"#);
    }

    #[test]
    fn redaction_covers_every_secret_key() {
        let line = r#"{"session":"A","cookies":"B"}&sig=C"#;
        let masked = redact_secrets(line);
        assert!(!masked.contains('A') && !masked.contains('B') && !masked.contains('C'), "{masked}");
    }

    #[test]
    fn a_disabled_tap_records_nothing() {
        let tap = Tap::disabled();
        assert!(!tap.is_enabled());
        assert_eq!(tap.record(Record::new("A", Direction::In, "request", b"x")), None);
    }

    #[test]
    fn records_carry_a_usable_reference() {
        let dir = std::env::temp_dir().join(format!("noobscenic-tap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let config = Wire { redact_secrets: true, ..Wire::default() };
        let tap = Tap::new(&config, dir.clone()).unwrap();

        let first = tap
            .record(
                Record::new("A", Direction::In, "request", b"sn=ABC&sig=SECRET")
                    .peer("192.168.1.243:41022")
                    .meta("path", "/cleanPack/register"),
            )
            .expect("enabled tap records");
        assert_eq!(first.seq, 1);
        assert!(first.reference.ends_with("#1"), "{}", first.reference);

        let second = tap.record(Record::new("B", Direction::Out, "frame", b"{}")).unwrap();
        assert_eq!(second.seq, 2);

        let name = first.reference.split('#').next().unwrap();
        let text = std::fs::read_to_string(dir.join(name)).unwrap();
        assert!(text.contains("\"path\":\"/cleanPack/register\""), "{text}");
        assert!(text.contains("sig=***"), "redaction applies to the JSONL sink: {text}");
        // ...but never to the raw copy.
        let raw = std::fs::read(
            dir.join("raw")
                .join(chrono::Utc::now().format("%Y-%m-%d").to_string())
                .join("1.req.bin"),
        )
        .unwrap();
        assert_eq!(raw, b"sn=ABC&sig=SECRET");

        tap.set_enabled(false);
        assert!(tap.record(Record::new("A", Direction::In, "request", b"x")).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn oversized_bodies_are_truncated_in_jsonl_only() {
        let dir = std::env::temp_dir().join(format!("noobscenic-trunc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let config = Wire { max_body_bytes: 8, ..Wire::default() };
        let tap = Tap::new(&config, dir.clone()).unwrap();

        let body = vec![b'x'; 64];
        let recorded = tap.record(Record::new("B", Direction::In, "frame", &body)).unwrap();
        let name = recorded.reference.split('#').next().unwrap();
        let text = std::fs::read_to_string(dir.join(name)).unwrap();
        assert!(text.contains("\"truncated\":true"), "{text}");
        assert!(text.contains("\"len\":64"), "{text}");

        let raw = std::fs::read(
            dir.join("raw")
                .join(chrono::Utc::now().format("%Y-%m-%d").to_string())
                .join("1.frame.bin"),
        )
        .unwrap();
        assert_eq!(raw.len(), 64, "the raw sink keeps the whole body");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
