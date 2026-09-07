//! The verbatim byte sink.
//!
//! Nothing here truncates, redacts or re-encodes: this is the copy that settles
//! arguments about framing and about what the device actually sent (doc/PLAN.md §8).

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::Result;

pub struct RawSink {
    root: PathBuf,
}

impl RawSink {
    pub fn new(root: PathBuf) -> RawSink {
        RawSink { root }
    }

    /// Write one unit of traffic to `raw/<date>/<seq>.<kind>.bin`.
    pub fn write(&self, today: &str, seq: u64, kind: &str, body: &[u8]) -> Result<PathBuf> {
        let dir = self.root.join(today);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{seq}.{}.bin", short_kind(kind)));
        let mut file = std::fs::File::create(&path)?;
        file.write_all(body)?;
        file.flush()?;
        Ok(path)
    }
}

fn short_kind(kind: &str) -> &str {
    match kind {
        "request" => "req",
        "response" => "resp",
        other => other,
    }
}

/// Delete dated subdirectories older than `retain_days`. `0` keeps everything.
pub fn prune(root: &Path, retain_days: u64) -> Result<usize> {
    if retain_days == 0 || !root.exists() {
        return Ok(0);
    }
    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(retain_days * 86_400))
        .unwrap_or(std::time::UNIX_EPOCH);

    let mut removed = 0;
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let modified = entry.metadata().and_then(|m| m.modified());
        if matches!(modified, Ok(at) if at < cutoff) {
            std::fs::remove_dir_all(entry.path())?;
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_bytes_untouched() {
        let root = std::env::temp_dir().join(format!("noobscenic-raw-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let sink = RawSink::new(root.clone());

        // Deliberately not valid UTF-8, and with the channel-B delimiter in it.
        let body = b"{\"a\":1}#\t#\xff\xfe";
        let path = sink.write("2026-09-07", 7, "frame", body).unwrap();
        assert_eq!(path.file_name().unwrap(), "7.frame.bin");
        assert_eq!(std::fs::read(&path).unwrap(), body);

        let path = sink.write("2026-09-07", 8, "request", b"x").unwrap();
        assert_eq!(path.file_name().unwrap(), "8.req.bin");
        let _ = std::fs::remove_dir_all(&root);
    }
}
