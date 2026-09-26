//! Filesystem adapter for the per-repo feedback log the dashboard writes and
//! the review workflow reads (`reviews/feedback.json`).

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use anyhow::{Result, anyhow};

use crate::adapters::report_store::{iso8601, write_atomic};
use crate::domain::feedback::{FeedbackEntry, FeedbackLog};

/// Serializes read-modify-write appends within this process.
static APPEND: Mutex<()> = Mutex::new(());

/// Defaults to `./reviews/feedback.json`. Override with `MOMUS_FEEDBACK=path`.
pub fn feedback_path() -> PathBuf {
    if let Ok(p) = std::env::var("MOMUS_FEEDBACK")
        && !p.trim().is_empty()
    {
        return PathBuf::from(p);
    }
    PathBuf::from("reviews").join("feedback.json")
}

/// Reads the feedback log. A missing file is an empty log; an unreadable or
/// malformed one is an error.
pub fn read_feedback(path: &Path) -> Result<FeedbackLog> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| anyhow!("{} is not a feedback log: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(FeedbackLog::default()),
        Err(e) => Err(anyhow!("read {}: {e}", path.display())),
    }
}

/// Appends one entry (timestamped now) and rewrites the log atomically.
pub fn append_feedback(path: &Path, mut entry: FeedbackEntry) -> Result<FeedbackLog> {
    let _guard = APPEND.lock().map_err(|_| anyhow!("feedback lock poisoned"))?;
    let mut log = read_feedback(path)?;
    entry.at = iso8601(SystemTime::now());
    log.entries.push(entry);
    write_atomic(path, &format!("{}\n", serde_json::to_string_pretty(&log)?))?;
    Ok(log)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::feedback::Vote;
    use crate::domain::policy::Dimension;

    #[test]
    fn missing_file_is_empty_and_append_round_trips() {
        let dir = std::env::temp_dir().join(format!("momus-feedback-{}", std::process::id()));
        let path = dir.join("feedback.json");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(read_feedback(&path).unwrap().entries.is_empty());

        let entry = FeedbackEntry {
            fingerprint: "00ff".into(),
            file: "src/a.rs".into(),
            line: 3,
            dimension: Dimension::Security,
            mechanism: "xss".into(),
            probability: 0.8,
            vote: Vote::Down,
            suppress: true,
            at: String::new(),
        };
        append_feedback(&path, entry).unwrap();
        let log = read_feedback(&path).unwrap();
        assert_eq!(log.entries.len(), 1);
        assert!(!log.entries[0].at.is_empty());
        assert!(log.suppressed().contains("00ff"));

        std::fs::write(&path, "not json").unwrap();
        assert!(read_feedback(&path).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
