//! Auditable, opt-in follow-up priorities under limited review resources.
//!
//! This is a fixed decision-policy heuristic, not a calibrated estimate of
//! information gain or dollars saved. Eligibility still comes from the
//! dimension thresholds. Actual HTTP attempts, retries and cache hits remain
//! the responsibility of the request client and its hard call budget.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::domain::policy::{DIMENSIONS, Dimension, SCREEN_THRESHOLD};
use crate::review::strategy::{FileEntry, Signal};

/// Choose the existing probability policy or the opt-in information-value policy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "camelCase")]
pub enum FollowUpStrategy {
    #[default]
    Probability,
    Voi,
}

/// The components of one priority decision, retained even when a cap omits it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiCandidate {
    pub file: String,
    pub dimension: Dimension,
    pub probability: f64,
    pub threshold: f64,
    pub impact_weight: f64,
    pub uncertainty: f64,
    /// Four normal evidence judgments plus one possible routing judgment.
    /// This excludes retries; cache hits and early abstention can cost less.
    pub estimated_calls: u64,
    pub score: f64,
    pub selected: bool,
}

/// Explain the selected order and any optional metadata work skipped for evidence.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct VoiSummary {
    pub candidates: Vec<VoiCandidate>,
    pub skipped_optional_profiles: bool,
}

/// Parse a dimension-specific eligibility override; reject non-finite probabilities.
pub fn parse_threshold(raw: &str) -> Result<(Dimension, f64), String> {
    let (key, value) = raw
        .split_once('=')
        .ok_or_else(|| "threshold must be DIMENSION=P (0 <= P <= 1)".to_string())?;
    let dimension = DIMENSIONS
        .into_iter()
        .find(|d| d.key() == key)
        .ok_or_else(|| format!("unknown dimension '{key}'; use correctness, security, reliability, compatibility or testGap"))?;
    let probability: f64 = value
        .parse()
        .map_err(|_| "threshold probability must be a number from 0 to 1".to_string())?;
    if !probability.is_finite() || !(0.0..=1.0).contains(&probability) {
        return Err("threshold probability must be finite and from 0 to 1".into());
    }
    Ok((dimension, probability))
}

/// Fixed impact weights prioritize runtime consequences over missing test evidence.
fn impact_weight(dimension: Dimension) -> f64 {
    match dimension {
        Dimension::Security => 3.0,
        Dimension::Correctness | Dimension::Reliability => 2.0,
        Dimension::Compatibility => 1.5,
        Dimension::TestGap => 1.0,
    }
}

/// Rank eligible signals and preserve dimension coverage before filling a finite cap.
/// Unlimited planning retains every signal. Estimated costs never gate admission:
/// even a zero-attempt budget can complete a cached pipeline.
pub fn select<F: FileEntry>(
    signals: &[Signal<F>],
    cap: Option<usize>,
    thresholds: &BTreeMap<Dimension, f64>,
    skipped_optional_profiles: bool,
) -> (Vec<Signal<F>>, VoiSummary) {
    let mut candidates: Vec<_> = signals
        .iter()
        .enumerate()
        .map(|(index, signal)| {
            let probability = signal.probability.clamp(0.0, 1.0);
            let uncertainty = 4.0 * probability * (1.0 - probability);
            let impact_weight = impact_weight(signal.dimension);
            // Without severity yet, reserve a conservative routing estimate of
            // one additional judgment. The client measures the actual cost.
            let estimated_calls = 5;
            let score = impact_weight * probability * (1.0 + uncertainty) / estimated_calls as f64;
            (
                index,
                VoiCandidate {
                    file: signal.file.path().to_string(),
                    dimension: signal.dimension,
                    probability,
                    threshold: thresholds
                        .get(&signal.dimension)
                        .copied()
                        .unwrap_or(SCREEN_THRESHOLD),
                    impact_weight,
                    uncertainty,
                    estimated_calls,
                    score,
                    selected: false,
                },
            )
        })
        .collect();
    candidates.sort_by(|(_, a), (_, b)| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.file.cmp(&b.file))
            .then_with(|| a.dimension.key().cmp(b.dimension.key()))
    });

    let limit = cap.unwrap_or(candidates.len());
    let mut selected_indices = Vec::new();
    let mut chosen = HashSet::new();
    if cap.is_some() {
        let mut dimensions = HashSet::new();
        for (index, candidate) in &candidates {
            if selected_indices.len() >= limit {
                break;
            }
            if dimensions.insert(candidate.dimension) {
                chosen.insert(*index);
                selected_indices.push(*index);
            }
        }
    }
    for (index, _) in &candidates {
        if selected_indices.len() >= limit {
            break;
        }
        if chosen.insert(*index) {
            selected_indices.push(*index);
        }
    }
    for (index, candidate) in &mut candidates {
        candidate.selected = chosen.contains(index);
    }
    let selected = selected_indices
        .into_iter()
        .map(|index| signals[index].clone())
        .collect();
    (
        selected,
        VoiSummary {
            candidates: candidates
                .into_iter()
                .map(|(_, candidate)| candidate)
                .collect(),
            skipped_optional_profiles,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::report::ChangedFile;

    /// Create an eligible signal with an explicit path for ordering assertions.
    fn signal(path: &str, dimension: Dimension, probability: f64) -> Signal<ChangedFile> {
        Signal {
            file: ChangedFile {
                path: path.into(),
                patch: String::new(),
                base: String::new(),
            },
            dimension,
            probability,
        }
    }

    #[test]
    fn impact_and_uncertainty_change_probability_only_selection() {
        let signals = vec![
            signal("tests.rs", Dimension::TestGap, 0.99),
            signal("auth.rs", Dimension::Security, 0.75),
            signal("confirmed.rs", Dimension::Security, 0.99),
        ];
        let (selected, plan) = select(&signals, Some(1), &BTreeMap::new(), true);
        assert_eq!(selected[0].file.path, "auth.rs");
        assert_eq!(plan.candidates[0].estimated_calls, 5);
        assert_eq!(plan.candidates[0].threshold, SCREEN_THRESHOLD);
        assert_eq!(plan.candidates.iter().filter(|c| c.selected).count(), 1);
        assert!(plan.skipped_optional_profiles);
        // High certainty still has positive value, rather than being suppressed.
        assert!(
            plan.candidates
                .iter()
                .find(|c| c.file == "confirmed.rs")
                .unwrap()
                .score
                > 0.0
        );
    }

    #[test]
    fn finite_cap_preserves_available_dimensions_before_duplicate_concerns() {
        let signals = vec![
            signal("auth_a.rs", Dimension::Security, 0.75),
            signal("auth_b.rs", Dimension::Security, 0.75),
            signal("coverage.rs", Dimension::TestGap, 0.75),
        ];
        let (selected, _) = select(&signals, Some(2), &BTreeMap::new(), false);
        assert_eq!(selected[0].file.path, "auth_a.rs");
        assert_eq!(selected[1].dimension, Dimension::TestGap);
    }

    #[test]
    fn unlimited_and_zero_caps_are_explicit_and_ties_are_stable() {
        let signals = vec![
            signal("z.rs", Dimension::Security, 0.8),
            signal("a.rs", Dimension::Security, 0.8),
        ];
        let (all, plan) = select(&signals, None, &BTreeMap::new(), false);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].file.path, "a.rs");
        assert!(plan.candidates.iter().all(|c| c.selected));
        let (none, plan) = select(&signals, Some(0), &BTreeMap::new(), false);
        assert!(none.is_empty());
        assert!(plan.candidates.iter().all(|c| !c.selected));
    }

    #[test]
    fn threshold_overrides_are_audited_and_invalid_values_rejected() {
        let parsed = parse_threshold("security=0.9").unwrap();
        let thresholds = BTreeMap::from([parsed]);
        let signals = [signal("auth.rs", Dimension::Security, 0.95)];
        let (_, plan) = select(&signals, None, &thresholds, false);
        assert_eq!(plan.candidates[0].threshold, 0.9);
        for raw in [
            "security=NaN",
            "security=inf",
            "security=-0.1",
            "security=1.1",
            "unknown=0.7",
            "0.7",
        ] {
            assert!(parse_threshold(raw).is_err(), "accepted {raw}");
        }
        assert_eq!(
            parse_threshold("testGap=0").unwrap(),
            (Dimension::TestGap, 0.0)
        );
    }
}
