//! Deterministic file priorities and stable shard membership.
use super::strategy::FileEntry;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

/// Order files by role, historical hotspots and Git churn, with deterministic path ties.
pub fn prioritize<F: FileEntry>(files: &mut [F], scopes: &[PathBuf]) {
    let mut scores: HashMap<String, f64> = HashMap::new();
    // Historical profiles provide file roles; findings identify local hotspots.
    if let Ok(history) = crate::adapters::report_store::read_history() {
        for entry in history {
            for profile in entry.report.profiles {
                let role = match profile.category.as_str() {
                    "boundary" | "entrypoint" => 6.0,
                    "domain" | "persistence" => 4.0,
                    _ => 1.0,
                };
                let score = scores.entry(profile.file).or_default();
                *score = score.max(role + profile.review_priority);
            }
            for finding in entry.report.findings {
                *scores.entry(finding.file).or_default() += 2.0;
            }
        }
    }
    if let Some(scope) = scopes.first()
        && let Ok(out) = Command::new("git")
            .arg("-C")
            .arg(scope)
            .args(["log", "-n", "100", "--format=", "--name-only"])
            .output()
        && out.status.success()
    {
        for path in String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|p| !p.is_empty())
        {
            *scores.entry(path.into()).or_default() += 0.1;
        }
    }
    let priority = |path: &str| {
        let role = if ["auth", "security", "route", "handler", "main", "lib"]
            .iter()
            .any(|part| path.contains(part))
        {
            2.0
        } else {
            0.0
        };
        role + scores.get(path).copied().unwrap_or(0.0)
    };
    files.sort_by(|a, b| {
        priority(b.path())
            .total_cmp(&priority(a.path()))
            .then_with(|| a.path().cmp(b.path()))
    });
}

/// Parse calls=N, including zero for a cache-only run.
pub fn parse_budget(raw: &str) -> Result<u64, String> {
    raw.strip_prefix("calls=")
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| "budget must be calls=N (N >= 0)".into())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Shard {
    pub index: usize,
    pub count: usize,
}
impl std::str::FromStr for Shard {
    type Err = String;
    /// Parse a 1-based shard selector and reject invalid indices or counts above 256.
    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let parse = || {
            let (i, n) = raw.split_once('/')?;
            Some(Self {
                index: i.parse().ok()?,
                count: n.parse().ok()?,
            })
        };
        match parse() {
            Some(s) if s.count > 0 && s.count <= 256 && s.index > 0 && s.index <= s.count => Ok(s),
            _ => Err("shard must be i/N with 1 <= i <= N <= 256".into()),
        }
    }
}
impl Shard {
    /// Assign each path to exactly one shard using the first eight SHA-256 bytes.
    pub fn contains(&self, path: &str) -> bool {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(path.as_bytes());
        let hash = u64::from_be_bytes(digest[..8].try_into().expect("8 bytes"));
        (hash % self.count as u64) as usize + 1 == self.index
    }
}
/// Hash exact discovery inputs, not the current HEAD alone (dirty files count).
pub fn inventory_key<F: FileEntry>(files: &[F], context: &[F]) -> String {
    use sha2::{Digest, Sha256};
    let sorted = |values: &[F]| {
        let mut values: Vec<_> = values
            .iter()
            .map(|f| {
                (
                    f.path().to_string(),
                    serde_json::to_value(f).expect("file JSON"),
                )
            })
            .collect();
        values.sort_by(|a, b| a.0.cmp(&b.0));
        values
    };
    format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(sorted(files), sorted(context))).expect("inventory JSON")
        )
    )
}
#[cfg(test)]
mod shard_tests {
    use super::*;
    #[test]
    fn shards_partition_paths_exactly_once() {
        for i in 0..1000 {
            let path = format!("src/file{i}.rs");
            assert_eq!(
                (1..=7)
                    .filter(|index| Shard {
                        index: *index,
                        count: 7
                    }
                    .contains(&path))
                    .count(),
                1
            );
        }
        for invalid in ["0/2", "3/2", "1/0", "1/257", "x/2"] {
            assert!(invalid.parse::<Shard>().is_err());
        }
    }
}
