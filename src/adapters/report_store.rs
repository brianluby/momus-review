//! Filesystem adapter for the saved review report that the dashboard reads.
//! Writes are atomic: unique `O_EXCL` temp file then rename. Mirrors
//! `adapters/report-store.ts`.

use std::fs::{OpenOptions, create_dir_all, metadata, read_dir, read_to_string, remove_file, rename};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Result, anyhow, bail};

use crate::domain::report::ReviewReport;

/// The result of reading a saved report, mirroring the prototype's
/// `StoredReport` shape consumed by the dashboard client.
///
/// The `Ok` variant is large (it carries a whole report) but is read once and
/// moved straight into the dashboard response; boxing would be overkill.
#[allow(clippy::large_enum_variant)]
pub enum StoredReport {
    Ok { saved_at: String, report: ReviewReport },
    Empty,
    Error(String),
}

/// Defaults to `./reviews/latest.json`. Override with `MOMUS_REPORT=path`
/// (successor to the prototype's `REVIEW_FILE`). Never in the install dir.
pub fn report_path() -> PathBuf {
    if let Ok(p) = std::env::var("MOMUS_REPORT")
        && !p.trim().is_empty()
    {
        return PathBuf::from(p);
    }
    PathBuf::from("reviews").join("latest.json")
}

/// Reads a saved report. Tolerant: `Empty` when absent, `Error(message)` when
/// the file exists but is not valid review output.
pub fn read_report(path: &Path) -> StoredReport {
    let text = match read_to_string(path) {
        Ok(t) => t,
        Err(_) => return StoredReport::Empty,
    };
    let report: ReviewReport = match serde_json::from_str(&text) {
        Ok(r) => r,
        Err(_) => return StoredReport::Error("Report is not review output".to_string()),
    };
    let saved_at = metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .map(iso8601)
        .unwrap_or_default();
    StoredReport::Ok { saved_at, report }
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Writes to a unique temp file created with `O_EXCL` and renames into place,
/// so readers never see a partial report and a planted symlink is never
/// followed.
pub fn save_report(report: &ReviewReport, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        create_dir_all(parent)?;
    }

    let data = format!("{}\n", serde_json::to_string_pretty(report)?);

    for _ in 0..10 {
        let temp = format!("{}.{}.tmp", path.display(), unique_suffix());
        let mut f = match OpenOptions::new().write(true).create_new(true).open(&temp) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                let _ = remove_file(&temp);
                return Err(anyhow!("save report: {e}"));
            }
        };
        if let Err(e) = f.write_all(data.as_bytes()) {
            let _ = remove_file(&temp);
            return Err(anyhow!("save report: {e}"));
        }
        drop(f);
        match rename(&temp, path) {
            Ok(()) => return Ok(()),
            Err(e) => {
                let _ = remove_file(&temp);
                return Err(anyhow!("save report: {e}"));
            }
        }
    }
    bail!("Could not save report: temp file collisions")
}

fn unique_suffix() -> String {
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{pid}.{nanos}.{counter}")
}

/// One saved history snapshot. `saved_at` derives from the file's mtime so a
/// per-sha entry reflects when that review was written.
pub struct HistoryEntry {
    pub sha: String,
    pub saved_at: String,
    pub report: ReviewReport,
}

/// Fixed `./reviews/history` directory holding one report per reviewed sha.
/// Deliberately not overridden by `MOMUS_REPORT` (which points at the single
/// `latest.json` the dashboard's review view reads); history is always a dir.
pub fn history_dir() -> PathBuf {
    PathBuf::from("reviews").join("history")
}

/// Writes the report to `reviews/history/{sha}.json` using the same atomic
/// `save_report` flow (unique `O_EXCL` temp + rename). Returns the written
/// path; overwriting an existing sha replaces it.
pub fn save_history(report: &ReviewReport, sha: &str) -> Result<PathBuf> {
    let path = history_dir().join(format!("{sha}.json"));
    save_report(report, &path)?;
    Ok(path)
}

/// Lists `reviews/history/*.json` (filename stem = sha) and reads each via
/// the tolerant `read_report`; files that are not valid review output are
/// skipped. Unordered — the dashboard sorts by `saved_at`. A missing (or
/// empty) directory is empty history, not an error.
pub fn read_history() -> Result<Vec<HistoryEntry>> {
    read_history_in(&history_dir())
}

fn read_history_in(dir: &Path) -> Result<Vec<HistoryEntry>> {
    let entries = match read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(anyhow!("read history: {e}")),
    };

    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(sha) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if let StoredReport::Ok { saved_at, report } = read_report(&path) {
            out.push(HistoryEntry { sha: sha.to_string(), saved_at, report });
        }
    }
    Ok(out)
}

/// Formats a `SystemTime` as a UTC ISO-8601 string (`YYYY-MM-DDTHH:MM:SSZ`),
/// the format the dashboard client feeds to `new Date(...)`. Dependency-free
/// via the civil-from-days algorithm.
fn iso8601(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// History over a nonexistent directory is empty, not an error.
    #[test]
    fn read_history_missing_dir_is_empty() {
        let dir = std::env::temp_dir().join(format!("momus-review-no-such-history-{}", std::process::id()));
        let entries = read_history_in(&dir).unwrap();
        assert!(entries.is_empty());
    }
}