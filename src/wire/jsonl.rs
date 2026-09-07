//! The rotating JSONL sink: one JSON object per line, per wire event.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::Result;

pub struct JsonlSink {
    dir: PathBuf,
    rotate_bytes: u64,
    date: String,
    index: u32,
    file: Option<File>,
    name: String,
    written: u64,
}

impl JsonlSink {
    pub fn new(dir: PathBuf, rotate_mb: u64) -> JsonlSink {
        JsonlSink {
            dir,
            rotate_bytes: rotate_mb.saturating_mul(1024 * 1024),
            date: String::new(),
            index: 0,
            file: None,
            name: String::new(),
            written: 0,
        }
    }

    /// The file the next line will land in, for building a `trace_ref`.
    pub fn current_name(&self) -> &str {
        &self.name
    }

    pub fn write_line(&mut self, today: &str, line: &str) -> Result<()> {
        self.ensure_open(today)?;

        let needed = line.len() as u64 + 1;
        if self.rotate_bytes > 0 && self.written > 0 && self.written + needed > self.rotate_bytes {
            self.index += 1;
            self.open(today, self.index)?;
        }

        if let Some(file) = self.file.as_mut() {
            file.write_all(line.as_bytes())?;
            file.write_all(b"\n")?;
            // Flushed per line on purpose: a trace that loses its tail when the
            // process dies is a trace that fails exactly when it is most needed.
            file.flush()?;
            self.written += needed;
        }
        Ok(())
    }

    fn ensure_open(&mut self, today: &str) -> Result<()> {
        if self.file.is_some() && self.date == today {
            return Ok(());
        }
        // A new day starts at index 0 and skips past whatever is already full, so a
        // restart mid-day appends rather than clobbering.
        let mut index = 0;
        while self.rotate_bytes > 0 {
            let candidate = self.dir.join(file_name(today, index));
            match std::fs::metadata(&candidate) {
                Ok(meta) if meta.len() >= self.rotate_bytes => index += 1,
                _ => break,
            }
        }
        self.open(today, index)
    }

    fn open(&mut self, today: &str, index: u32) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let name = file_name(today, index);
        let path = self.dir.join(&name);
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        self.written = file.metadata().map(|m| m.len()).unwrap_or(0);
        self.file = Some(file);
        self.name = name;
        self.date = today.to_string();
        self.index = index;
        Ok(())
    }
}

fn file_name(date: &str, index: u32) -> String {
    if index == 0 {
        format!("wire-{date}.jsonl")
    } else {
        format!("wire-{date}.{index}.jsonl")
    }
}

/// Delete trace files last modified more than `retain_days` ago. `0` keeps everything.
pub fn prune(dir: &Path, retain_days: u64) -> Result<usize> {
    if retain_days == 0 || !dir.exists() {
        return Ok(0);
    }
    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(retain_days * 86_400))
        .unwrap_or(std::time::UNIX_EPOCH);

    let mut removed = 0;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("wire-") || !name.contains(".jsonl") {
            continue;
        }
        let modified = entry.metadata().and_then(|m| m.modified());
        if matches!(modified, Ok(at) if at < cutoff) {
            std::fs::remove_file(entry.path())?;
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("noobscenic-jsonl-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn writes_and_names_the_current_file() {
        let dir = scratch("write");
        let mut sink = JsonlSink::new(dir.clone(), 64);
        sink.write_line("2026-09-07", "{\"a\":1}").unwrap();
        assert_eq!(sink.current_name(), "wire-2026-09-07.jsonl");
        let text = std::fs::read_to_string(dir.join("wire-2026-09-07.jsonl")).unwrap();
        assert_eq!(text, "{\"a\":1}\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotates_on_size_and_on_a_new_day() {
        let dir = scratch("rotate");
        let mut sink = JsonlSink::new(dir.clone(), 0);
        sink.rotate_bytes = 32; // a size no real line will fit under twice
        for _ in 0..4 {
            sink.write_line("2026-09-07", "0123456789012345678901234").unwrap();
        }
        assert!(dir.join("wire-2026-09-07.jsonl").exists());
        assert!(dir.join("wire-2026-09-07.1.jsonl").exists(), "should have rotated");

        sink.write_line("2026-09-08", "{}").unwrap();
        assert_eq!(sink.current_name(), "wire-2026-09-08.jsonl");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pruning_leaves_fresh_files_alone() {
        let dir = scratch("prune");
        std::fs::write(dir.join("wire-2026-09-07.jsonl"), b"{}\n").unwrap();
        assert_eq!(prune(&dir, 14).unwrap(), 0);
        assert_eq!(prune(&dir, 0).unwrap(), 0, "0 means keep everything");
        assert!(dir.join("wire-2026-09-07.jsonl").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
