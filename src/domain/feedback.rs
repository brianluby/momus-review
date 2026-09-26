//! Finding feedback: stable fingerprints, the suppression list, and
//! per-dimension screen-threshold tuning from thumbs up/down. Pure logic; the
//! store lives in `adapters::feedback_store`.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::domain::policy::{
    DIMENSIONS, Dimension, HIGH_PRECISION, MAX_TUNED_THRESHOLD, MIN_FEEDBACK_VOTES,
    MIN_TUNED_THRESHOLD, SCREEN_THRESHOLD, TARGET_PRECISION, THRESHOLD_STEP,
};
use crate::domain::report::Finding;

/// A reviewer's verdict on a finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Vote {
    Up,
    Down,
}

/// One recorded vote. Later entries for the same fingerprint supersede
/// earlier ones, so a reviewer can change their mind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackEntry {
    pub fingerprint: String,
    pub file: String,
    #[serde(default)]
    pub line: usize,
    pub dimension: Dimension,
    #[serde(default)]
    pub mechanism: String,
    /// The finding's screen probability when voted on.
    pub probability: f64,
    pub vote: Vote,
    /// Hide this finding in future runs.
    #[serde(default)]
    pub suppress: bool,
    #[serde(default)]
    pub at: String,
}

/// The per-repo feedback log (`reviews/feedback.json`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FeedbackLog {
    pub entries: Vec<FeedbackEntry>,
}

impl FeedbackLog {
    /// The latest entry per fingerprint, in first-seen order.
    pub fn latest(&self) -> Vec<&FeedbackEntry> {
        let mut index: HashMap<&str, usize> = HashMap::new();
        let mut out: Vec<&FeedbackEntry> = Vec::new();
        for entry in &self.entries {
            match index.get(entry.fingerprint.as_str()) {
                Some(&i) => out[i] = entry,
                None => {
                    index.insert(&entry.fingerprint, out.len());
                    out.push(entry);
                }
            }
        }
        out
    }

    /// Fingerprints whose latest entry asks for suppression.
    pub fn suppressed(&self) -> HashSet<String> {
        self.latest()
            .into_iter()
            .filter(|e| e.suppress)
            .map(|e| e.fingerprint.clone())
            .collect()
    }

    /// Per-dimension screen thresholds tuned from the latest votes.
    pub fn thresholds(&self) -> BTreeMap<Dimension, f64> {
        let latest = self.latest();
        DIMENSIONS
            .iter()
            .map(|&d| {
                let votes: Vec<(f64, bool)> = latest
                    .iter()
                    .filter(|e| e.dimension == d)
                    .map(|e| (e.probability, e.vote == Vote::Up))
                    .collect();
                (d, tune_threshold(&votes))
            })
            .collect()
    }
}

/// Picks a screen threshold from `(probability, was_useful)` votes.
///
/// Votes only exist for findings that cleared the old threshold, so there is
/// no evidence below it: with high precision at the default the threshold
/// drops one step, otherwise it rises to the lowest step whose precision
/// reaches `TARGET_PRECISION` (or the ceiling if none does). Too few votes
/// keep the default.
pub fn tune_threshold(votes: &[(f64, bool)]) -> f64 {
    if votes.len() < MIN_FEEDBACK_VOTES {
        return SCREEN_THRESHOLD;
    }
    let precision_at = |t: f64| {
        let kept: Vec<bool> = votes.iter().filter(|(p, _)| *p >= t - 1e-9).map(|(_, u)| *u).collect();
        (!kept.is_empty()).then(|| kept.iter().filter(|u| **u).count() as f64 / kept.len() as f64)
    };
    match precision_at(SCREEN_THRESHOLD) {
        Some(p) if p >= HIGH_PRECISION => {
            return round_step((SCREEN_THRESHOLD - THRESHOLD_STEP).max(MIN_TUNED_THRESHOLD));
        }
        Some(p) if p >= TARGET_PRECISION => return SCREEN_THRESHOLD,
        _ => {}
    }
    let mut t = SCREEN_THRESHOLD;
    while t < MAX_TUNED_THRESHOLD - 1e-9 {
        t = round_step(t + THRESHOLD_STEP);
        if precision_at(t).is_some_and(|p| p >= TARGET_PRECISION) {
            return t;
        }
    }
    MAX_TUNED_THRESHOLD
}

fn round_step(t: f64) -> f64 {
    (t * 100.0).round() / 100.0
}

/// A line-independent identity for a finding: file, dimension, mechanism, and
/// the evidence text with diff hunk headers dropped and whitespace collapsed,
/// hashed with 64-bit FNV-1a. Survives edits elsewhere in the file.
pub fn fingerprint(finding: &Finding) -> String {
    let evidence: String = finding
        .evidence
        .lines()
        .filter(|l| !l.starts_with("@@"))
        .flat_map(|l| l.split_whitespace())
        .collect::<Vec<_>>()
        .join(" ");
    let key = format!(
        "{}\u{0}{}\u{0}{}\u{0}{}",
        finding.file,
        finding.dimension.key(),
        finding.mechanism,
        evidence
    );
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in key.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(fp: &str, dim: Dimension, p: f64, vote: Vote, suppress: bool) -> FeedbackEntry {
        FeedbackEntry {
            fingerprint: fp.into(),
            file: "src/a.rs".into(),
            line: 1,
            dimension: dim,
            mechanism: "other".into(),
            probability: p,
            vote,
            suppress,
            at: String::new(),
        }
    }

    #[test]
    fn fingerprint_ignores_line_numbers_and_whitespace() {
        let a = Finding {
            file: "src/a.rs".into(),
            line: 10,
            mechanism: "sqlInjection".into(),
            evidence: "@@ -1,2 +1,2 @@\n+  query(x)\n".into(),
            ..Default::default()
        };
        let b = Finding {
            line: 99,
            evidence: "@@ -40,2 +41,2 @@\n+ query(x)".into(),
            ..a.clone()
        };
        let c = Finding { mechanism: "xss".into(), ..a.clone() };
        assert_eq!(fingerprint(&a), fingerprint(&b));
        assert_ne!(fingerprint(&a), fingerprint(&c));
        assert_eq!(fingerprint(&a).len(), 16);
    }

    #[test]
    fn latest_vote_wins_for_suppression() {
        let log = FeedbackLog {
            entries: vec![
                entry("a", Dimension::Security, 0.9, Vote::Down, true),
                entry("b", Dimension::Security, 0.9, Vote::Down, true),
                entry("a", Dimension::Security, 0.9, Vote::Up, false),
            ],
        };
        let suppressed = log.suppressed();
        assert!(!suppressed.contains("a"));
        assert!(suppressed.contains("b"));
        assert_eq!(log.latest().len(), 2);
    }

    #[test]
    fn too_few_votes_keep_the_default() {
        let votes = vec![(0.9, false); MIN_FEEDBACK_VOTES - 1];
        assert_eq!(tune_threshold(&votes), SCREEN_THRESHOLD);
    }

    #[test]
    fn high_precision_lowers_one_step() {
        let votes = vec![(0.8, true); 6];
        assert_eq!(tune_threshold(&votes), 0.65);
    }

    #[test]
    fn acceptable_precision_keeps_the_default() {
        let mut votes = vec![(0.8, true); 4];
        votes.extend([(0.8, false), (0.75, false)]);
        assert_eq!(tune_threshold(&votes), SCREEN_THRESHOLD);
    }

    #[test]
    fn noisy_low_band_raises_to_first_precise_step() {
        // Everything below 0.85 was noise; at >= 0.85 precision is 3/4.
        let votes = vec![
            (0.72, false),
            (0.75, false),
            (0.78, false),
            (0.82, false),
            (0.84, false),
            (0.86, true),
            (0.9, true),
            (0.92, true),
            (0.95, false),
        ];
        assert_eq!(tune_threshold(&votes), 0.85);
    }

    #[test]
    fn hopeless_dimension_hits_the_ceiling() {
        let votes = vec![(0.8, false); 6];
        assert_eq!(tune_threshold(&votes), MAX_TUNED_THRESHOLD);
    }

    #[test]
    fn thresholds_are_per_dimension() {
        let mut entries: Vec<FeedbackEntry> = (0..6)
            .map(|i| entry(&format!("s{i}"), Dimension::Security, 0.8, Vote::Down, false))
            .collect();
        entries.extend((0..6).map(|i| entry(&format!("c{i}"), Dimension::Correctness, 0.8, Vote::Up, false)));
        let t = FeedbackLog { entries }.thresholds();
        assert_eq!(t[&Dimension::Security], MAX_TUNED_THRESHOLD);
        assert_eq!(t[&Dimension::Correctness], 0.65);
        assert_eq!(t[&Dimension::TestGap], SCREEN_THRESHOLD);
    }
}
