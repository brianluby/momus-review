//! Repository-local outcome assessment and opt-in approval eligibility.
//!
//! [`p_revert`] retains its historical API name, but produces a unitless,
//! **uncalibrated heuristic** from hand-picked review weights. A zero score
//! means these signals contributed nothing; it does not establish a zero
//! chance of a revert, incident or test flake.
//!
//! [`assess`] separately uses supplied history to fit four fixed score bins for
//! each [`OutcomeKind`]. Missing labels remain unknown. Training merges precede
//! the cutoff and their complete outcome windows and observations must be
//! available by that cutoff; held-out merges begin at the cutoff and must
//! mature by `as_of`. Even an early positive event waits for its full window
//! before entering evaluation. The caller must additionally establish when
//! evidence first became available: event time alone cannot prove that a label
//! was known at the split. The outcome collector retains that separate receipt
//! and withholds labels unavailable at the relevant boundary.
//!
//! Evidence strings and provenance are caller assertions, not authenticated
//! telemetry. The caller owns outcome definitions, full negative surveillance,
//! score capture before merge and compatible review pipelines. Synthetic data
//! exercises mechanics only and cannot authorize approval. Empirical estimates
//! and their sampling bounds do not establish causality or bound distribution
//! shift. This module returns an auditable eligibility decision; publication
//! and a fresh GitHub identity check belong to the approval adapter.
//!
//! The heuristic applies an exponential transform to a weighted `risk` score:
//!
//! ```text
//! blocking      = count of findings whose action == RequestChanges
//! max_severity  = max finding.severity (0.0 when there are no findings)
//! max_test_gap  = max row.probabilities[TestGap] over the matrix
//!                 (0.0 when absent)
//! risk = 0.55 * min(1, 0.5 * blocking)
//!      + 0.25 * (max_severity / SEVERITY_MAX)
//!      + 0.10 * max_test_gap
//!      + 0.10 * min(1, matrix.len() / 50)
//! p_revert = 1 - exp(-3 * risk)
//! ```
//!
//! With valid input scales, no findings and an empty matrix produce zero, and
//! the exponential term keeps the heuristic below one. The name `p_revert`
//! does not turn this transformation into an observed-outcome probability.

use crate::domain::policy::{BLOCKING_SEVERITY, DIMENSIONS, Dimension, SEVERITY_MAX};
use crate::domain::report::{Action, Finding, MatrixRow};
use crate::domain::report::{ReviewMode, ReviewReport};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

const MIN_TRAIN: usize = 40;
const MIN_HELD_OUT: usize = 20;
const MIN_BIN: usize = 20;
const BINS: usize = 4;

/// Producer identity for the heuristic formula, severity scale and fixed bins.
/// Increment whenever any of those change; existing histories then require
/// separate compatible exports rather than silently mixing score producers.
pub const HEURISTIC_VERSION: u32 = 1;

/// Separately labelled outcomes, each with its own observation window.
///
/// A revert label does not establish an incident or a flake label, including a
/// negative one. Separation here does not assume statistical independence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OutcomeKind {
    /// A qualifying revert within the caller's repository-specific definition.
    Revert,
    /// A qualifying incident attributed to this merge by the caller's evidence.
    Incident,
    /// A qualifying test flake under the caller's protocol, separate from failure.
    Flake,
}

/// A caller-supplied event or a completed negative surveillance observation.
///
/// [`validate_history`] checks timestamps and nonempty evidence; it cannot
/// verify the source or prove that surveillance covered the entire window.
/// Keep a label absent in [`OutcomeRecord`] when telemetry is missing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObservedOutcome {
    /// `true` identifies a qualifying event; `false` asserts complete surveillance
    /// found no qualifying event throughout this outcome's window.
    pub occurred: bool,
    /// Unix seconds: positive event time, or the end of negative surveillance.
    /// This is not an evidence-availability timestamp. Callers must track and
    /// enforce availability separately before exporting a historical split.
    pub observed_at: i64,
    /// Nonempty source reference/receipt; its authenticity is a caller contract.
    pub evidence: String,
}

/// One merged head with a score frozen by the caller before the merge.
///
/// Labels are optional independently. A missing incident observation must not
/// inherit a supplied revert result or be interpreted as `occurred: false`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutcomeRecord {
    /// Unique canonical lowercase, nonzero full 40/64-character Git identity
    /// for observed histories. Synthetic histories may use nonempty fixture IDs.
    pub head: String,
    /// Positive merge time in Unix seconds, no later than history `as_of`.
    pub merged_at: i64,
    /// Positive capture time in Unix seconds, no later than `merged_at`.
    /// Equal-second values pass structural validation; the caller must prove
    /// that capture preceded merge rather than infer ordering from coarse time.
    pub score_recorded_at: i64,
    /// Finite score in `[0, 1]` from the pinned producer, never a fitted probability.
    pub heuristic_score: f64,
    /// Missing revert evidence remains unknown.
    pub revert: Option<ObservedOutcome>,
    /// Missing incident evidence remains unknown even when revert evidence exists.
    pub incident: Option<ObservedOutcome>,
    /// Missing flake evidence remains unknown even when another outcome is known.
    pub flake: Option<ObservedOutcome>,
}

/// Positive observation durations in seconds, evaluated separately per outcome.
///
/// Defaults are 30 days for revert/incident and seven days for flake. They are
/// engineering defaults; callers must freeze the chosen protocol before labels
/// are collected instead of selecting windows to improve the resulting score.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutcomeWindows {
    /// Revert observation duration after merge.
    pub revert_seconds: i64,
    /// Incident observation duration after merge.
    pub incident_seconds: i64,
    /// Flake observation duration after merge.
    pub flake_seconds: i64,
}

impl Default for OutcomeWindows {
    fn default() -> Self {
        Self {
            revert_seconds: 30 * 86400,
            incident_seconds: 30 * 86400,
            flake_seconds: 7 * 86400,
        }
    }
}

/// Versioned, caller-provenanced history with an explicit chronological split.
///
/// Merges before `training_cutoff` may train only after their entire windows
/// have matured by the cutoff. Merges at/after it are held out and must mature
/// by `as_of`. Observation times are checked at those boundaries; separate
/// availability receipts and authoritative outcome definitions remain the
/// caller's responsibility. See [`validate_history`] for structural rejection.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutcomeHistory {
    /// Required on input. There is no inferred producer for legacy exports.
    pub heuristic_version: u32,
    /// Exact repository identity, compared with current-head check evidence.
    pub repository: String,
    /// Nonempty description/reference of the source and collection protocol.
    pub provenance: String,
    /// Demonstration-only history; estimates from it can never approve a merge.
    pub synthetic: bool,
    /// Unix-second knowledge boundary, strictly after `training_cutoff`.
    pub as_of: i64,
    /// Positive Unix-second training boundary, fixed before held-out observation.
    pub training_cutoff: i64,
    /// Independent positive windows; timestamp addition must not overflow.
    pub windows: OutcomeWindows,
    /// Uniquely identified merges; input ordering does not affect fitting.
    pub records: Vec<OutcomeRecord>,
}

/// Descriptive support and held-out diagnostics for a single outcome.
///
/// Metrics may be populated while the candidate estimate remains unknown due
/// to insufficient support. They are not approval evidence by themselves.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Evaluation {
    /// Mature, available labels from merges strictly before the cutoff.
    pub training_samples: usize,
    /// Mature labels from merges at/after the cutoff, never used for fitting.
    pub held_out_samples: usize,
    /// Records with no label for this outcome, rather than negative observations.
    pub unknown_labels: usize,
    /// Present labels withheld because their window or observation misses a boundary.
    pub immature_or_unavailable_labels: usize,
    /// Qualifying events among training samples.
    pub training_events: usize,
    /// Qualifying events among held-out samples.
    pub held_out_events: usize,
    /// Mean squared error on held-out labels, using training-only bin estimates.
    pub brier_score: Option<f64>,
    /// Held-out error of the smoothed training population rate for comparison.
    pub baseline_brier_score: Option<f64>,
    /// Held-out bin calibration errors weighted by their held-out sample counts.
    pub expected_calibration_error: Option<f64>,
    /// Held-out support in the candidate's bin; pooled support cannot replace it.
    pub matching_bin_held_out_samples: usize,
    /// Held-out events in the candidate's bin.
    pub matching_bin_held_out_events: usize,
    /// Absolute difference between that bin's training estimate and held-out rate.
    pub matching_bin_calibration_error: Option<f64>,
    /// Latest training observation in the candidate bin, used for freshness gates.
    pub matching_bin_training_last_observed_at: Option<i64>,
    /// Latest held-out observation in the candidate bin, separate from merge age.
    pub matching_bin_held_out_last_observed_at: Option<i64>,
    /// Latest held-out merge in the candidate bin; refreshing `as_of` cannot revive it.
    pub matching_bin_held_out_last_merged_at: Option<i64>,
}

/// Candidate-bin estimate for one outcome, with support and limitations.
///
/// Probability is absent until there are at least 40 training labels, 20
/// held-out labels, 20 labels in each candidate training/held-out bin and both
/// training outcome classes. `evaluatedEmpirical` describes supplied observed
/// history; it is not a certification that the caller's labels are authentic.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutcomeEstimate {
    /// Outcome assessed independently from the other two estimates.
    pub outcome: OutcomeKind,
    /// Declared observation duration; absent when no history was supplied.
    pub window_seconds: Option<i64>,
    /// unknown | evaluatedEmpirical | syntheticDemonstration
    pub status: String,
    /// Laplace-smoothed event frequency `(events + 1) / (samples + 2)` in the
    /// selected training bin, conditional on this supplied history and protocol.
    pub probability: Option<f64>,
    /// Wilson 95% binomial upper bound for the selected training bin.
    /// Sampling uncertainty only; it does not bound distribution shift.
    pub upper_bound_95: Option<f64>,
    /// Separate held-out sampling bound, protecting against pooled metrics
    /// masking regression in the candidate's score bin.
    pub held_out_upper_bound_95: Option<f64>,
    /// Mature training support in the candidate's score bin.
    pub matching_bin_samples: usize,
    /// Held-out diagnostics, including candidate-bin evidence age.
    pub evaluation: Evaluation,
    /// Evidence gaps and statistical assumptions the consumer must retain.
    pub limitations: Vec<String>,
}

/// Trusted caller's current-head identity, coverage and check inventory.
///
/// `Default` expresses missing evidence, not a passing review. When supplied,
/// every field is required on the JSON input; absence is not inferred from a
/// green check or an empty finding list. [`assess`] rejects approval when any
/// coverage assertion is false or an auxiliary unknown remains.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CheckEvidence {
    /// Exact repository identity matching the supplied history.
    pub repository: String,
    /// Current candidate head; approval compares this with all review/check heads.
    pub head: String,
    /// Head that produced the source review, not a subsequently fetched head.
    pub reviewed_head: String,
    /// Positive Unix-second assessment time supplied by the trusted collector.
    pub assessed_at: i64,
    /// Caller has verified full changed-file coverage, including auxiliary analyses.
    pub review_complete: bool,
    /// Caller has complete source/check provenance for this identity.
    pub evidence_complete: bool,
    /// Changed inputs are supported by the review and auxiliary analyses.
    pub supported_inputs: bool,
    /// Unresolved limitations; any entry blocks approval.
    pub auxiliary_unknowns: Vec<String>,
    /// All collected checks, including optional ones; every supplied check must
    /// be uniquely named, passed, fresh, source-backed and bound to this head.
    pub checks: Vec<NamedCheck>,
}

/// One normalized current-head check result and its provenance reference.
///
/// Missing names, unknown results, stale/future completion, missing evidence or
/// duplicate names block approval, including duplicates across check sources.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NamedCheck {
    /// Nonempty, unique check name matched exactly against required policy names.
    pub name: String,
    /// Exact candidate commit that the check evaluated.
    pub head: String,
    /// passed | failed | pending | unknown; every value except passed rejects.
    pub status: String,
    /// Positive Unix-second completion time, no later than `assessed_at`.
    pub completed_at: i64,
    /// Nonempty check source URL/receipt; the engine does not fetch it.
    pub evidence: String,
}

/// Explicit opt-in limits for observed-history approval, disabled by default.
///
/// Supplying or enabling a policy does not establish eligibility. All three
/// observed outcomes, their training/held-out sampling bounds and calibration
/// diagnostics must pass, alongside a complete immutable changes review and
/// fresh, uniquely named current-head checks. Synthetic histories, missing
/// telemetry and intrinsically blocking findings cannot be waived by a limit.
///
/// Omitted JSON fields use these defaults. An enabled policy requires at least
/// one nonempty required-check name; [`assess`] validates policy limits even
/// when approval is disabled.
///
/// ```
/// use momus_review::domain::report::ReviewReport;
/// use momus_review::review::merge_confidence::{ApprovalPolicy, assess};
///
/// let report = ReviewReport::default();
/// let mut policy = ApprovalPolicy::default();
/// let decision = assess(&report, None, Some(&policy), None)?.approval;
/// assert!(!decision.enabled && !decision.eligible);
///
/// // Opting in without naming required checks is an invalid policy.
/// policy.enabled = true;
/// assert!(assess(&report, None, Some(&policy), None).is_err());
/// # Ok::<(), anyhow::Error>(())
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalPolicy {
    /// Explicit opt-in; false leaves the decision disabled regardless of evidence.
    pub enabled: bool,
    /// Exact required names. Every supplied check must also pass, even if optional.
    pub required_checks: Vec<String>,
    /// Maximum training and held-out 95% sampling upper bounds for revert.
    pub max_revert_probability: f64,
    /// Maximum training and held-out 95% sampling upper bounds for incident.
    pub max_incident_probability: f64,
    /// Maximum training and held-out 95% sampling upper bounds for flake.
    pub max_flake_probability: f64,
    /// Inclusive limit for pooled and candidate-bin held-out calibration error.
    /// Held-out Brier error must additionally be no worse than the baseline.
    pub max_calibration_error: f64,
    /// A finding at or above this severity rejects approval. The intrinsic
    /// blocking threshold and `RequestChanges` action remain independent vetoes.
    pub max_finding_severity: f64,
    /// Positive maximum check age relative to the supplied assessment time.
    pub max_check_age_seconds: i64,
    /// Positive maximum history/observation age. Candidate-bin cohort checks
    /// additionally account for the outcome duration; refreshing `as_of` alone
    /// does not refresh old evidence or old review pipelines.
    pub max_history_age_seconds: i64,
}

impl Default for ApprovalPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            required_checks: Vec::new(),
            max_revert_probability: 0.1,
            max_incident_probability: 0.1,
            max_flake_probability: 0.1,
            max_calibration_error: 0.1,
            max_finding_severity: 1.0,
            max_check_age_seconds: 3600,
            max_history_age_seconds: 30 * 86400,
        }
    }
}

/// Auditable policy decision, not a GitHub approval or a merge instruction.
///
/// Eligibility means all gates passed for the supplied assessment inputs. A
/// publishing adapter must fetch fresh evidence and recheck repository identity;
/// this value is not a durable authorization for a later changed head.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalDecision {
    /// Whether a supplied policy explicitly opted in to automatic approval.
    pub enabled: bool,
    /// True only when enabled and no rejection reason remains.
    pub eligible: bool,
    /// Human-readable explanations; do not treat their text as a stable enum.
    pub reasons: Vec<String>,
}

/// Serialized assessment tying a heuristic producer to separate outcome results.
///
/// Preserve `limitations`, outcome status, provenance and the approval decision
/// when displaying estimates. The heuristic score alone is never merge approval
/// evidence, and a readable legacy artifact is not a compatible history export.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeConfidenceSummary {
    /// Assessment artifact format version, distinct from score producer identity.
    pub version: u32,
    /// Zero means a saved legacy artifact omitted producer provenance. It is
    /// never used for fitting or approval; assess emits the current version.
    #[serde(default)]
    pub heuristic_version: u32,
    /// Unitless hand-weighted review heuristic recomputed from the supplied report.
    pub heuristic_score: f64,
    /// Display label explicitly distinguishing the heuristic from probabilities.
    pub heuristic_label: String,
    /// Declared history repository, absent when no history was supplied.
    pub repository: Option<String>,
    /// Declared history source/protocol, not independently authenticated here.
    pub provenance: Option<String>,
    /// Whether supplied history is demonstration-only; absent without history.
    pub synthetic: Option<bool>,
    /// Declared chronological fitting boundary, absent without history.
    pub training_cutoff: Option<i64>,
    /// Declared knowledge boundary for this history, absent without history.
    pub as_of: Option<i64>,
    /// Revert, incident and flake estimates in that order, with separate unknowns.
    pub outcomes: Vec<OutcomeEstimate>,
    /// Fail-closed decision over the supplied report, policy, history and checks.
    pub approval: ApprovalDecision,
}

fn label(record: &OutcomeRecord, kind: OutcomeKind) -> Option<&ObservedOutcome> {
    match kind {
        OutcomeKind::Revert => record.revert.as_ref(),
        OutcomeKind::Incident => record.incident.as_ref(),
        OutcomeKind::Flake => record.flake.as_ref(),
    }
}

fn window(windows: &OutcomeWindows, kind: OutcomeKind) -> i64 {
    match kind {
        OutcomeKind::Revert => windows.revert_seconds,
        OutcomeKind::Incident => windows.incident_seconds,
        OutcomeKind::Flake => windows.flake_seconds,
    }
}

/// Check the structural and temporal contract of a supplied history.
///
/// Empty histories and absent labels are valid inputs; they establish no
/// observed-outcome estimate. This function does not collect telemetry, inspect
/// evidence references, authenticate provenance or determine when an event was
/// first discovered. Callers must supply those assurances, including complete
/// negative surveillance and score capture before merge.
///
/// # Errors
///
/// Returns an error for incompatible producer identity, empty provenance,
/// invalid chronological boundaries, missing/duplicate/noncanonical observed
/// heads, nonfinite/out-of-range scores, invalid or overflowing windows, capture
/// after merge, future observations or labels outside their required window.
/// Positive events must occur within the window; a negative observation must
/// reach the full window. Equal capture/merge seconds are structurally accepted
/// and cannot by themselves prove pre-merge capture.
///
/// This tiny fabricated input demonstrates validation only; it is deliberately
/// marked synthetic and provides no real-world calibration evidence.
///
/// ```
/// use momus_review::review::merge_confidence::{
///     HEURISTIC_VERSION, ObservedOutcome, OutcomeHistory, OutcomeRecord,
///     OutcomeWindows, validate_history,
/// };
///
/// let mut history = OutcomeHistory {
///     heuristic_version: HEURISTIC_VERSION,
///     repository: "example/repository".into(),
///     provenance: "offline validation fixture; no observed production history".into(),
///     synthetic: true,
///     as_of: 2_000,
///     training_cutoff: 1_000,
///     windows: OutcomeWindows {
///         revert_seconds: 100, incident_seconds: 100, flake_seconds: 100,
///     },
///     records: vec![OutcomeRecord {
///         head: "fixture-unknown".into(),
///         merged_at: 100,
///         score_recorded_at: 99,
///         heuristic_score: 0.2,
///         revert: None, incident: None, flake: None,
///     }],
/// };
/// validate_history(&history)?; // Missing telemetry remains unknown.
/// history.records[0].revert = Some(ObservedOutcome {
///     occurred: false,
///     observed_at: 150,
///     evidence: "surveillance stopped before the window ended".into(),
/// });
/// assert!(validate_history(&history).is_err()); // Window matures at second 200.
/// # Ok::<(), anyhow::Error>(())
/// ```
pub fn validate_history(history: &OutcomeHistory) -> Result<()> {
    ensure!(
        history.heuristic_version == HEURISTIC_VERSION,
        "history heuristicVersion is unsupported; expected {HEURISTIC_VERSION}"
    );
    ensure!(
        !history.repository.trim().is_empty() && !history.provenance.trim().is_empty(),
        "history requires repository and source provenance"
    );
    ensure!(
        history.training_cutoff > 0 && history.as_of > history.training_cutoff,
        "history needs a positive chronological training cutoff before asOf"
    );
    let mut heads = BTreeSet::new();
    for record in &history.records {
        ensure!(
            history.synthetic || canonical_git_head(&record.head),
            "observed history requires canonical full 40/64-character lowercase Git heads"
        );
        ensure!(
            !record.head.trim().is_empty() && heads.insert(&record.head),
            "history head is missing or duplicated"
        );
        ensure!(
            record.merged_at > 0
                && record.merged_at <= history.as_of
                && record.score_recorded_at > 0
                && record.score_recorded_at <= record.merged_at,
            "score must be recorded before merge; merge must precede asOf"
        );
        ensure!(
            record.heuristic_score.is_finite() && (0.0..=1.0).contains(&record.heuristic_score),
            "historical heuristic score must be finite in [0,1]"
        );
        for kind in [
            OutcomeKind::Revert,
            OutcomeKind::Incident,
            OutcomeKind::Flake,
        ] {
            let duration = window(&history.windows, kind);
            ensure!(duration > 0, "outcome windows must be positive");
            let maturity = record
                .merged_at
                .checked_add(duration)
                .ok_or_else(|| anyhow::anyhow!("outcome window timestamp overflow"))?;
            if let Some(observed) = label(record, kind) {
                ensure!(
                    !observed.evidence.trim().is_empty(),
                    "every observed outcome requires evidence"
                );
                ensure!(
                    observed.observed_at >= record.merged_at
                        && observed.observed_at <= history.as_of,
                    "outcome observation is before merge or after asOf"
                );
                ensure!(
                    if observed.occurred {
                        observed.observed_at <= maturity
                    } else {
                        observed.observed_at >= maturity
                    },
                    "positive labels must fall within the window; negative labels require the full window"
                );
            }
        }
    }
    for kind in [
        OutcomeKind::Revert,
        OutcomeKind::Incident,
        OutcomeKind::Flake,
    ] {
        ensure!(
            window(&history.windows, kind) > 0,
            "outcome windows must be positive"
        );
    }
    Ok(())
}

fn canonical_git_head(head: &str) -> bool {
    matches!(head.len(), 40 | 64)
        && head
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && head.bytes().any(|byte| byte != b'0')
}

fn latest(timestamp: &mut Option<i64>, observed: i64) {
    *timestamp = Some(timestamp.map_or(observed, |current| current.max(observed)));
}

fn bin(score: f64) -> usize {
    ((score * BINS as f64) as usize).min(BINS - 1)
}

fn empirical(events: usize, samples: usize) -> f64 {
    (events as f64 + 1.0) / (samples as f64 + 2.0)
}

fn upper_bound(events: usize, samples: usize) -> f64 {
    let n = samples as f64;
    let p = events as f64 / n;
    let z2 = 1.96_f64.powi(2);
    (p + z2 / (2.0 * n) + 1.96 * ((p * (1.0 - p) + z2 / (4.0 * n)) / n).sqrt()) / (1.0 + z2 / n)
}

fn estimate(history: Option<&OutcomeHistory>, score: f64, kind: OutcomeKind) -> OutcomeEstimate {
    let mut result = OutcomeEstimate {
        outcome: kind, window_seconds: history.map(|h| window(&h.windows, kind)), status: "unknown".into(),
        probability: None, upper_bound_95: None, held_out_upper_bound_95: None, matching_bin_samples: 0, evaluation: Evaluation::default(),
        limitations: vec!["Repository-local, fixed-bin empirical model; observational labels and source provenance are caller supplied. No causal guarantee or distribution-shift bound.".into()],
    };
    let Some(history) = history else {
        result.limitations.push(
            "No outcome history supplied; absence of findings does not establish low merge risk."
                .into(),
        );
        return result;
    };
    let duration = window(&history.windows, kind);
    let mut counts = [0_usize; BINS];
    let mut events = [0_usize; BINS];
    let mut held_out = Vec::new();
    // Stable ordering also fixes floating-point reduction order for reproducible
    // evaluation JSON when the input export arrives in a different order.
    let mut records: Vec<_> = history.records.iter().collect();
    records.sort_by(|a, b| {
        a.merged_at
            .cmp(&b.merged_at)
            .then_with(|| a.head.cmp(&b.head))
    });
    for record in records {
        let Some(observed) = label(record, kind) else {
            result.evaluation.unknown_labels += 1;
            continue;
        };
        let maturity = record.merged_at + duration; // validated checked addition
        if record.merged_at < history.training_cutoff {
            if maturity > history.training_cutoff || observed.observed_at > history.training_cutoff
            {
                result.evaluation.immature_or_unavailable_labels += 1;
                continue;
            }
            counts[bin(record.heuristic_score)] += 1;
            events[bin(record.heuristic_score)] += usize::from(observed.occurred);
            if bin(record.heuristic_score) == bin(score) {
                latest(
                    &mut result.evaluation.matching_bin_training_last_observed_at,
                    observed.observed_at,
                );
            }
        } else if maturity <= history.as_of {
            if bin(record.heuristic_score) == bin(score) {
                latest(
                    &mut result.evaluation.matching_bin_held_out_last_observed_at,
                    observed.observed_at,
                );
                latest(
                    &mut result.evaluation.matching_bin_held_out_last_merged_at,
                    record.merged_at,
                );
            }
            held_out.push((
                bin(record.heuristic_score),
                if observed.occurred { 1.0 } else { 0.0 },
            ));
        } else {
            result.evaluation.immature_or_unavailable_labels += 1;
        }
    }
    let samples: usize = counts.iter().sum();
    let total_events: usize = events.iter().sum();
    result.evaluation.training_samples = samples;
    result.evaluation.training_events = total_events;
    result.evaluation.held_out_samples = held_out.len();
    result.evaluation.held_out_events = held_out.iter().filter(|(_, y)| *y == 1.0).count();
    result.matching_bin_samples = counts[bin(score)];
    if samples == 0 || held_out.is_empty() {
        result
            .limitations
            .push("No usable mature training or held-out labels; outcomes remain unknown.".into());
        return result;
    }
    let baseline = empirical(total_events, samples);
    let probabilities: [f64; BINS] = std::array::from_fn(|b| {
        if counts[b] > 0 {
            empirical(events[b], counts[b])
        } else {
            baseline
        }
    });
    let n = held_out.len() as f64;
    result.evaluation.brier_score = Some(
        held_out
            .iter()
            .map(|(b, y)| (probabilities[*b] - y).powi(2))
            .sum::<f64>()
            / n,
    );
    result.evaluation.baseline_brier_score = Some(
        held_out
            .iter()
            .map(|(_, y)| (baseline - y).powi(2))
            .sum::<f64>()
            / n,
    );
    let mut eval_counts = [0_usize; BINS];
    let mut eval_events = [0_usize; BINS];
    for (b, y) in &held_out {
        eval_counts[*b] += 1;
        eval_events[*b] += usize::from(*y == 1.0);
    }
    result.evaluation.matching_bin_held_out_samples = eval_counts[bin(score)];
    result.evaluation.matching_bin_held_out_events = eval_events[bin(score)];
    if eval_counts[bin(score)] > 0 {
        result.evaluation.matching_bin_calibration_error = Some(
            (probabilities[bin(score)]
                - eval_events[bin(score)] as f64 / eval_counts[bin(score)] as f64)
                .abs(),
        );
    }
    result.evaluation.expected_calibration_error = Some(
        (0..BINS)
            .filter(|b| eval_counts[*b] > 0)
            .map(|b| {
                (probabilities[b] - eval_events[b] as f64 / eval_counts[b] as f64).abs()
                    * eval_counts[b] as f64
                    / n
            })
            .sum(),
    );
    if samples < MIN_TRAIN
        || held_out.len() < MIN_HELD_OUT
        || result.matching_bin_samples < MIN_BIN
        || total_events == 0
        || total_events == samples
    {
        result.limitations.push(format!("Insufficient outcome support: need {MIN_TRAIN} training, {MIN_HELD_OUT} held-out, {MIN_BIN} matching-bin labels and both training outcome classes."));
        return result;
    }
    // The relevant score bin must also have held-out evidence, not merely the pooled population.
    if eval_counts[bin(score)] < MIN_BIN {
        result.limitations.push(format!(
            "Current score bin needs {MIN_BIN} held-out labels; pooled evaluation is insufficient."
        ));
        return result;
    }
    result.probability = Some(probabilities[bin(score)]);
    result.upper_bound_95 = Some(upper_bound(events[bin(score)], counts[bin(score)]));
    result.held_out_upper_bound_95 = Some(upper_bound(
        eval_events[bin(score)],
        eval_counts[bin(score)],
    ));
    result.status = if history.synthetic {
        "syntheticDemonstration"
    } else {
        "evaluatedEmpirical"
    }
    .into();
    result.limitations.push("Chronological held-out evaluation is descriptive; small samples, incomplete surveillance and changing review pipelines can invalidate calibration. Thresholds are engineering guardrails, not a certification.".into());
    if history.synthetic {
        result.limitations.push("Synthetic fixtures demonstrate mechanics only; they provide no real-world calibration and cannot authorize automatic approval.".into());
    }
    result
}

fn validate_policy(policy: &ApprovalPolicy) -> Result<()> {
    for value in [
        policy.max_revert_probability,
        policy.max_incident_probability,
        policy.max_flake_probability,
        policy.max_calibration_error,
    ] {
        ensure!(
            value.is_finite() && (0.0..=1.0).contains(&value),
            "approval probability and calibration limits must be finite in [0,1]"
        );
    }
    ensure!(
        policy.max_finding_severity.is_finite()
            && (0.0..=SEVERITY_MAX).contains(&policy.max_finding_severity),
        "approval severity limit is invalid"
    );
    ensure!(
        policy.max_check_age_seconds > 0 && policy.max_history_age_seconds > 0,
        "approval evidence freshness limits must be positive"
    );
    ensure!(
        !policy.enabled
            || (!policy.required_checks.is_empty()
                && policy
                    .required_checks
                    .iter()
                    .all(|name| !name.trim().is_empty())),
        "enabled automatic approval requires named checks"
    );
    Ok(())
}

fn incomplete_spec_evidence(report: &ReviewReport) -> bool {
    use crate::review::spec_drift::SpecCheckStatus;
    let Some(summary) = report.spec_drift.as_ref() else {
        return false;
    };
    if summary.documents.is_empty() || summary.checks.is_empty() {
        return true;
    }
    let valid_excerpt = |evidence: &crate::review::spec_drift::SpecEvidence| {
        !evidence.path.trim().is_empty()
            && evidence.start_line > 0
            && evidence.end_line >= evidence.start_line
            && !evidence.text.trim().is_empty()
    };
    summary.checks.iter().any(|check| {
        if !check.confidence.is_finite()
            || !(0.85..=1.0).contains(&check.confidence)
            || check.reason.trim().is_empty()
            || !report.matrix.iter().any(|row| row.file == check.file)
        {
            return true;
        }
        match check.status {
            SpecCheckStatus::NotApplicable => false,
            SpecCheckStatus::Matches => {
                check
                    .source
                    .as_ref()
                    .is_none_or(|source| !valid_excerpt(source) || source.path != check.file)
                    || check.spec.as_ref().is_none_or(|spec| {
                        !valid_excerpt(spec)
                            || !summary.documents.iter().any(|document| {
                                document.path == spec.path
                                    && !document.content_hash.trim().is_empty()
                                    && document.bytes > 0
                                    && document.candidates > 0
                            })
                    })
            }
            SpecCheckStatus::Drift | SpecCheckStatus::Uncertain | SpecCheckStatus::Deferred => true,
        }
    })
}

fn approval(
    report: &ReviewReport,
    history: Option<&OutcomeHistory>,
    policy: Option<&ApprovalPolicy>,
    checks: Option<&CheckEvidence>,
    outcomes: &[OutcomeEstimate],
) -> ApprovalDecision {
    let mut decision = ApprovalDecision {
        enabled: policy.is_some_and(|p| p.enabled),
        eligible: false,
        reasons: Vec::new(),
    };
    if !decision.enabled {
        decision
            .reasons
            .push("Automatic approval is disabled; explicit enabled policy required.".into());
        return decision;
    }
    let Some(policy) = policy else {
        return decision;
    };
    if report.mode != ReviewMode::Changes {
        decision
            .reasons
            .push("Automatic approval requires a changes review.".into());
    }
    if report.workflow.suppressed_findings > 0 {
        decision
            .reasons
            .push("Suppressed findings prevent proving absence of high-risk evidence.".into());
    }
    if incomplete_spec_evidence(report) {
        decision.reasons.push("Specification checks indicate drift, uncertainty, deferred work or insufficient evidence.".into());
    }
    if !report.reviewed_clean
        || !report.reviewed_committed
        || report
            .reviewed_base
            .as_deref()
            .is_none_or(|base| base.trim().is_empty())
    {
        decision.reasons.push(
            "Review requires committed-only evidence, a clean immutable checkout and recorded merge-base identity.".into(),
        );
    }
    if report
        .upgrades
        .as_ref()
        .is_some_and(|summary| !summary.unknowns.is_empty())
        || report
            .docs_drift
            .as_ref()
            .is_some_and(|summary| !summary.unknowns.is_empty())
    {
        decision
            .reasons
            .push("Upgrade or documentation analysis has unknown evidence.".into());
    }
    let mut matrix_files = BTreeSet::new();
    if DIMENSIONS
        .iter()
        .any(|dimension| !report.dimensions.iter().any(|meta| meta.key == *dimension))
        || report.matrix.iter().any(|row| {
            row.file.trim().is_empty()
                || !matrix_files.insert(row.file.as_str())
                || DIMENSIONS
                    .iter()
                    .any(|dimension| !row.probabilities.contains_key(dimension))
        })
    {
        decision.reasons.push(
            "Approval requires all review dimensions and unique evidenced file coverage.".into(),
        );
    }
    if report.partial
        || !report.skipped.is_empty()
        || report.budget.deferred > 0
        || report.workflow.dropped_context_chars > 0
        || report.workflow.dropped_context_items > 0
        || !report.tier.dismissed.is_empty()
        || report.shard.is_some()
        || report.workflow.followed_signals < report.workflow.threshold_signals
        || report.screened_files == 0
        || report.matrix.len() != report.screened_files
    {
        decision.reasons.push(
            "Review is incomplete, truncated, deferred, sharded, skipped or lacks file coverage."
                .into(),
        );
    }
    if report.workflow.needs_human_findings > 0
        || report
            .findings
            .iter()
            .any(|f| f.ensemble.as_ref().is_some_and(|e| e.needs_human))
    {
        decision
            .reasons
            .push("Findings require human judgment.".into());
    }
    if report.findings.iter().any(|f| {
        f.action == Action::RequestChanges
            || f.severity >= BLOCKING_SEVERITY
            || f.severity >= policy.max_finding_severity
    }) {
        decision
            .reasons
            .push("Blocking or high-risk findings prevent automatic approval.".into());
    }
    if report
        .findings
        .iter()
        .any(|f| f.evidence.trim().is_empty() || f.file.trim().is_empty() || f.line == 0)
    {
        decision
            .reasons
            .push("Finding evidence or location is missing.".into());
    }
    if history.is_none() {
        decision
            .reasons
            .push("Observed outcome history is missing.".into());
    }
    if history.is_some_and(|h| h.synthetic) {
        decision
            .reasons
            .push("Synthetic outcomes cannot authorize approval.".into());
    }
    for outcome in outcomes {
        let limit = match outcome.outcome {
            OutcomeKind::Revert => policy.max_revert_probability,
            OutcomeKind::Incident => policy.max_incident_probability,
            OutcomeKind::Flake => policy.max_flake_probability,
        };
        if outcome.status != "evaluatedEmpirical"
            || outcome.probability.is_none()
            || outcome.upper_bound_95.is_none_or(|p| p > limit)
            || outcome.held_out_upper_bound_95.is_none_or(|p| p > limit)
        {
            decision.reasons.push(format!(
                "{:?} risk is unknown, unsupported or exceeds the probability uncertainty bound.",
                outcome.outcome
            ));
        }
        if outcome
            .evaluation
            .expected_calibration_error
            .is_none_or(|ece| ece > policy.max_calibration_error)
            || outcome
                .evaluation
                .matching_bin_calibration_error
                .is_none_or(|ece| ece > policy.max_calibration_error)
            || outcome
                .evaluation
                .brier_score
                .zip(outcome.evaluation.baseline_brier_score)
                .is_none_or(|(actual, baseline)| actual > baseline)
        {
            decision.reasons.push(format!(
                "{:?} held-out calibration or baseline comparison is insufficient.",
                outcome.outcome
            ));
        }
    }
    let Some(checks) = checks else {
        decision
            .reasons
            .push("Current-head checks and completeness evidence are missing.".into());
        return decision;
    };
    if checks.repository.trim().is_empty()
        || checks.head.trim().is_empty()
        || checks.head != checks.reviewed_head
        || report.reviewed_head.as_deref() != Some(checks.head.as_str())
        || checks.assessed_at <= 0
    {
        decision.reasons.push(
            "Current repository/head identity and review timestamp are missing or mismatched."
                .into(),
        );
    }
    if history.is_some_and(|history| !history.synthetic) && !canonical_git_head(&checks.head) {
        decision.reasons.push(
            "Observed-history approval requires a canonical full lowercase Git candidate head."
                .into(),
        );
    }
    if !checks.review_complete
        || !checks.evidence_complete
        || !checks.supported_inputs
        || !checks.auxiliary_unknowns.is_empty()
    {
        decision
            .reasons
            .push("Review or auxiliary evidence is incomplete, unsupported or unknown.".into());
    }
    if let Some(history) = history
        && (history.repository != checks.repository
            || history.as_of > checks.assessed_at
            || checks.assessed_at.saturating_sub(history.as_of) > policy.max_history_age_seconds
            || history.records.iter().any(|r| r.head == checks.head))
    {
        decision.reasons.push("Outcome history is stale, from another repository, future-dated or includes the candidate head.".into());
    }
    for outcome in outcomes {
        let Some(duration) = outcome.window_seconds else {
            continue;
        };
        let maturity_allowance = policy.max_history_age_seconds.saturating_add(duration);
        let recent = |timestamp: Option<i64>, allowed: i64| {
            timestamp.is_some_and(|timestamp| {
                timestamp > 0
                    && timestamp <= checks.assessed_at
                    && checks.assessed_at.saturating_sub(timestamp) <= allowed
            })
        };
        if !recent(
            outcome.evaluation.matching_bin_training_last_observed_at,
            maturity_allowance,
        ) || !recent(
            outcome.evaluation.matching_bin_held_out_last_observed_at,
            policy.max_history_age_seconds,
        ) || !recent(
            outcome.evaluation.matching_bin_held_out_last_merged_at,
            maturity_allowance,
        ) {
            decision.reasons.push(format!("{:?} candidate-bin training/evaluation observations or held-out merge cohorts are stale or missing; a refreshed asOf cannot establish current evidence.", outcome.outcome));
        }
    }
    let mut names = BTreeSet::new();
    for check in &checks.checks {
        if !names.insert(&check.name)
            || check.name.trim().is_empty()
            || check.head != checks.head
            || check.status != "passed"
            || check.evidence.trim().is_empty()
            || check.completed_at <= 0
            || check.completed_at > checks.assessed_at
            || checks.assessed_at.saturating_sub(check.completed_at) > policy.max_check_age_seconds
        {
            decision.reasons.push(format!(
                "Check '{}' is failed, pending, duplicated, stale or lacks current-head evidence.",
                check.name
            ));
        }
    }
    for required in &policy.required_checks {
        if !names.contains(required) {
            decision
                .reasons
                .push(format!("Required check '{required}' is missing."));
        }
    }
    decision.eligible = decision.reasons.is_empty();
    decision
}

/// Assess separate outcomes and opt-in eligibility without publishing anything.
///
/// Recomputes the legacy heuristic from the report rather than trusting a saved
/// score, validates supplied history/policy and evaluates chronological held-out
/// labels. Valid but insufficient evidence produces an `Ok` summary with unknown
/// estimates and/or rejection reasons; missing history never becomes a negative
/// outcome. Approval requires all three supported observed estimates, fresh
/// candidate-bin evidence and current-head checks, plus complete committed
/// review inputs. Neither an absent nor a disabled policy can approve.
///
/// # Errors
///
/// Returns an error for invalid history or policy, a nonfinite/out-of-range
/// finding severity/confidence or invalid screening values. Passing structural
/// validation does not authenticate caller-supplied surveillance, provenance or
/// evidence availability. No network or publication side effect occurs here.
///
/// ```
/// use momus_review::domain::report::ReviewReport;
/// use momus_review::review::merge_confidence::assess;
///
/// let summary = assess(&ReviewReport::default(), None, None, None)?;
/// assert_eq!(summary.heuristic_score, 0.0);
/// assert_eq!(summary.outcomes.len(), 3);
/// assert!(summary.outcomes.iter().all(|outcome| {
///     outcome.status == "unknown" && outcome.probability.is_none()
/// }));
/// assert!(!summary.approval.enabled && !summary.approval.eligible);
/// // No findings is not evidence that real outcome risk is zero.
/// # Ok::<(), anyhow::Error>(())
/// ```
pub fn assess(
    report: &ReviewReport,
    history: Option<&OutcomeHistory>,
    policy: Option<&ApprovalPolicy>,
    checks: Option<&CheckEvidence>,
) -> Result<MergeConfidenceSummary> {
    if let Some(history) = history {
        validate_history(history)?;
    }
    if let Some(policy) = policy {
        validate_policy(policy)?;
    }
    for finding in &report.findings {
        ensure!(
            finding.severity.is_finite() && (0.0..=SEVERITY_MAX).contains(&finding.severity),
            "finding severity is invalid"
        );
        ensure!(
            [
                finding.probability,
                finding.location_confidence,
                finding.mechanism_confidence,
                finding.severity_confidence
            ]
            .iter()
            .all(|v| v.is_finite() && (0.0..=1.0).contains(v)),
            "finding confidence is invalid"
        );
    }
    ensure!(
        report
            .matrix
            .iter()
            .flat_map(|r| r.probabilities.values())
            .all(|p| p.is_finite() && (0.0..=1.0).contains(p)),
        "review matrix probabilities are invalid"
    );
    let heuristic = p_revert(&report.findings, &report.matrix);
    let outcomes: Vec<_> = [
        OutcomeKind::Revert,
        OutcomeKind::Incident,
        OutcomeKind::Flake,
    ]
    .into_iter()
    .map(|kind| estimate(history, heuristic, kind))
    .collect();
    Ok(MergeConfidenceSummary {
        version: 1,
        heuristic_version: HEURISTIC_VERSION,
        heuristic_score: heuristic,
        heuristic_label: "Uncalibrated hand-weighted review score; not a probability.".into(),
        repository: history.map(|h| h.repository.clone()),
        provenance: history.map(|h| h.provenance.clone()),
        synthetic: history.map(|h| h.synthetic),
        training_cutoff: history.map(|h| h.training_cutoff),
        as_of: history.map(|h| h.as_of),
        approval: approval(report, history, policy, checks, &outcomes),
        outcomes,
    })
}

/// Compute the unitless hand-weighted review heuristic under its legacy name.
///
/// With finite confidences in `[0, 1]` and severities in `[0, SEVERITY_MAX]`, the
/// exponential transform returns a value in `[0, 1)`. Empty inputs contribute
/// zero. This function performs no input validation, fitting, observation or
/// probability calibration; use [`assess`] for validation and explicit unknowns.
/// Its numerical range and name do not make it a probability of reverting.
pub fn p_revert(findings: &[Finding], matrix: &[MatrixRow]) -> f64 {
    let blocking = findings
        .iter()
        .filter(|f| f.action == Action::RequestChanges)
        .count() as f64;

    let max_severity = findings.iter().map(|f| f.severity).fold(0.0, f64::max);

    let max_test_gap = matrix
        .iter()
        .filter_map(|row| row.probabilities.get(&Dimension::TestGap).copied())
        .fold(0.0, f64::max);

    let risk = 0.55 * (0.5 * blocking).min(1.0)
        + 0.25 * (max_severity / SEVERITY_MAX)
        + 0.10 * max_test_gap
        + 0.10 * (matrix.len() as f64 / 50.0).min(1.0);

    1.0 - (-3.0 * risk).exp()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn finding_with(action: Action, severity: f64) -> Finding {
        Finding {
            file: "src/main.rs".into(),
            line: 1,
            dimension: Dimension::TestGap,
            probability: 0.93,
            location_confidence: 0.9,
            mechanism: "boundary".into(),
            mechanism_confidence: 0.5,
            severity,
            severity_confidence: 0.5,
            owner: None,
            owner_confidence: None,
            action,
            evidence: "an excerpt".into(),
            ..Default::default()
        }
    }

    fn matrix_row_with(test_gap: f64) -> MatrixRow {
        MatrixRow {
            file: "src/main.rs".into(),
            probabilities: BTreeMap::from([(Dimension::TestGap, test_gap)]),
        }
    }

    #[test]
    fn empty_is_zero() {
        assert!((p_revert(&[], &[]) - 0.0).abs() < 1e-9);
    }

    #[test]
    fn blocking_count_increases_p_revert() {
        // Equal severity, so the increase comes from the blocking term alone
        // (0.5 → 1.0), not from a higher max severity.
        let one = vec![finding_with(Action::RequestChanges, 2.0)];
        let two = vec![
            finding_with(Action::RequestChanges, 2.0),
            finding_with(Action::RequestChanges, 2.0),
        ];
        assert!(p_revert(&one, &[]) < p_revert(&two, &[]));
    }

    #[test]
    fn estimate_is_bounded() {
        let findings = (0..5)
            .map(|_| finding_with(Action::RequestChanges, SEVERITY_MAX))
            .collect::<Vec<_>>();
        let matrix = (0..60).map(|_| matrix_row_with(1.0)).collect::<Vec<_>>();
        let p = p_revert(&findings, &matrix);
        assert!(p >= 0.0);
        assert!(p < 1.0);
    }

    fn routine_report() -> ReviewReport {
        ReviewReport {
            reviewed_head: Some("ffffffffffffffffffffffffffffffffffffffff".into()),
            reviewed_clean: true,
            reviewed_committed: true,
            reviewed_base: Some("fixture-base".into()),
            screened_files: 1,
            dimensions: crate::domain::policy::dimension_metadata(),
            matrix: vec![MatrixRow {
                file: "src/main.rs".into(),
                probabilities: DIMENSIONS
                    .into_iter()
                    .map(|dimension| (dimension, 0.0))
                    .collect(),
            }],
            ..Default::default()
        }
    }

    fn observed_history() -> OutcomeHistory {
        let records = (0..150).map(|i| {
            let training = i < 100;
            let event = if training { i >= 80 } else { i >= 140 };
            let merged_at = if training { 100 + i } else { 2000 + i };
            let observation = ObservedOutcome { occurred: event, observed_at: merged_at + 100, evidence: format!("test-only surveillance record {i}; fabricated, not a real calibration claim") };
            OutcomeRecord { head: format!("{:040x}", i + 1), merged_at, score_recorded_at: merged_at - 1, heuristic_score: if event { 0.9 } else { 0.0 }, revert: Some(observation.clone()), incident: Some(observation.clone()), flake: Some(observation) }
        }).collect();
        // The synthetic flag is false ONLY to exercise the trusted-observation code path.
        // These unit-test records are fabricated and make no empirical product claim.
        OutcomeHistory {
            heuristic_version: HEURISTIC_VERSION,
            repository: "test/repo".into(),
            provenance: "fabricated unit-test observed-source contract".into(),
            synthetic: false,
            as_of: 5000,
            training_cutoff: 1000,
            windows: OutcomeWindows {
                revert_seconds: 100,
                incident_seconds: 100,
                flake_seconds: 100,
            },
            records,
        }
    }

    fn policy() -> ApprovalPolicy {
        ApprovalPolicy {
            enabled: true,
            required_checks: vec!["test".into()],
            ..Default::default()
        }
    }

    fn check_evidence() -> CheckEvidence {
        CheckEvidence {
            repository: "test/repo".into(),
            head: "ffffffffffffffffffffffffffffffffffffffff".into(),
            reviewed_head: "ffffffffffffffffffffffffffffffffffffffff".into(),
            assessed_at: 5000,
            review_complete: true,
            evidence_complete: true,
            supported_inputs: true,
            auxiliary_unknowns: Vec::new(),
            checks: vec![NamedCheck {
                name: "test".into(),
                head: "ffffffffffffffffffffffffffffffffffffffff".into(),
                status: "passed".into(),
                completed_at: 4999,
                evidence: "test-only check receipt".into(),
            }],
        }
    }

    #[test]
    fn missing_history_is_unknown_even_for_empty_heuristic() {
        let result = assess(&ReviewReport::default(), None, None, None).unwrap();
        assert_eq!(result.heuristic_score, 0.0);
        assert!(
            result
                .outcomes
                .iter()
                .all(|o| o.probability.is_none() && o.status == "unknown")
        );
        assert!(!result.approval.eligible && !result.approval.enabled);
    }

    #[test]
    fn routine_change_can_pass_explicit_policy_with_source_contract_evidence() {
        let summary = assess(
            &routine_report(),
            Some(&observed_history()),
            Some(&policy()),
            Some(&check_evidence()),
        )
        .unwrap();
        assert!(summary.approval.eligible, "{:?}", summary.approval.reasons);
        for outcome in summary.outcomes {
            assert_eq!(outcome.status, "evaluatedEmpirical");
            assert_eq!(outcome.evaluation.training_samples, 100);
            assert_eq!(outcome.evaluation.held_out_samples, 50);
            assert!(outcome.evaluation.brier_score < outcome.evaluation.baseline_brier_score);
            assert!(outcome.upper_bound_95.unwrap() < 0.1);
        }
    }

    #[test]
    fn synthetic_estimates_are_demonstrations_and_cannot_approve() {
        let mut history = observed_history();
        history.synthetic = true;
        let summary = assess(
            &routine_report(),
            Some(&history),
            Some(&policy()),
            Some(&check_evidence()),
        )
        .unwrap();
        assert!(
            summary
                .outcomes
                .iter()
                .all(|o| o.status == "syntheticDemonstration")
        );
        assert!(!summary.approval.eligible);
    }

    #[test]
    fn labels_are_independent_and_null_is_not_negative() {
        let mut history = observed_history();
        for record in &mut history.records {
            record.incident = None;
        }
        let summary = assess(
            &routine_report(),
            Some(&history),
            Some(&policy()),
            Some(&check_evidence()),
        )
        .unwrap();
        assert!(summary.outcomes[0].probability.is_some());
        assert!(summary.outcomes[1].probability.is_none());
        assert_eq!(summary.outcomes[1].evaluation.unknown_labels, 150);
        assert!(!summary.approval.eligible);
    }

    #[test]
    fn training_never_uses_future_labels_or_immature_windows() {
        let mut history = observed_history();
        history.records[0].revert.as_mut().unwrap().observed_at = 1100;
        history.records[1].merged_at = 950;
        history.records[1].revert.as_mut().unwrap().observed_at = 1050;
        history.records[1].incident.as_mut().unwrap().observed_at = 1050;
        history.records[1].flake.as_mut().unwrap().observed_at = 1050;
        let summary = assess(&routine_report(), Some(&history), None, None).unwrap();
        assert_eq!(summary.outcomes[0].evaluation.training_samples, 98);
        assert_eq!(
            summary.outcomes[0]
                .evaluation
                .immature_or_unavailable_labels,
            2
        );
        assert_eq!(summary.outcomes[1].evaluation.training_samples, 99);
    }

    #[test]
    fn chronological_evaluation_is_order_independent_and_keeps_heldout_out_of_fit() {
        let history = observed_history();
        let baseline = assess(&routine_report(), Some(&history), None, None).unwrap();
        let mut changed = history.clone();
        changed.records.reverse();
        let reordered = assess(&routine_report(), Some(&changed), None, None).unwrap();
        assert_eq!(
            serde_json::to_value(&baseline).unwrap(),
            serde_json::to_value(&reordered).unwrap()
        );
        for record in changed
            .records
            .iter_mut()
            .filter(|r| r.merged_at >= changed.training_cutoff)
        {
            record.revert.as_mut().unwrap().occurred = true;
        }
        let adverse = assess(
            &routine_report(),
            Some(&changed),
            Some(&policy()),
            Some(&check_evidence()),
        )
        .unwrap();
        assert_eq!(
            baseline.outcomes[0].probability,
            adverse.outcomes[0].probability
        );
        assert!(
            adverse.outcomes[0].evaluation.brier_score
                > baseline.outcomes[0].evaluation.brier_score
        );
        assert!(!adverse.approval.eligible);
    }

    #[test]
    fn validation_rejects_corrupt_history_and_invalid_policy() {
        let base = observed_history();
        let mut cases = Vec::new();
        let mut h = base.clone();
        h.records[0].heuristic_score = f64::NAN;
        cases.push(h);
        let mut h = base.clone();
        h.records[0].score_recorded_at = h.records[0].merged_at + 1;
        cases.push(h);
        let mut h = base.clone();
        h.records.push(h.records[0].clone());
        cases.push(h);
        let mut h = base.clone();
        h.records[0].revert.as_mut().unwrap().observed_at = 101;
        cases.push(h);
        let mut h = base.clone();
        h.records[0].revert.as_mut().unwrap().evidence.clear();
        cases.push(h);
        let mut h = base.clone();
        h.records[0].revert.as_mut().unwrap().observed_at = h.as_of + 1;
        cases.push(h);
        let mut h = base.clone();
        h.windows.flake_seconds = 0;
        cases.push(h);
        let mut h = base.clone();
        h.windows.flake_seconds = i64::MAX;
        cases.push(h);
        let mut h = base.clone();
        h.provenance.clear();
        cases.push(h);
        let mut h = base.clone();
        h.training_cutoff = h.as_of;
        cases.push(h);
        for history in cases {
            assert!(validate_history(&history).is_err());
        }
        let mut p = policy();
        p.max_flake_probability = f64::NAN;
        assert!(assess(&routine_report(), Some(&base), Some(&p), None).is_err());
        let mut p = policy();
        p.required_checks.clear();
        assert!(assess(&routine_report(), Some(&base), Some(&p), None).is_err());
    }

    #[test]
    fn producer_version_is_required_and_incompatible_histories_are_rejected() {
        let mut encoded = serde_json::to_value(observed_history()).unwrap();
        encoded.as_object_mut().unwrap().remove("heuristicVersion");
        assert!(serde_json::from_value::<OutcomeHistory>(encoded).is_err());
        let mut history = observed_history();
        history.heuristic_version = HEURISTIC_VERSION + 1;
        assert!(validate_history(&history).is_err());
        assert!(
            assess(
                &routine_report(),
                Some(&history),
                Some(&policy()),
                Some(&check_evidence())
            )
            .is_err()
        );
        let summary = assess(&routine_report(), None, None, None).unwrap();
        assert_eq!(summary.heuristic_version, HEURISTIC_VERSION);
        let mut legacy = serde_json::to_value(summary).unwrap();
        legacy.as_object_mut().unwrap().remove("heuristicVersion");
        assert_eq!(
            serde_json::from_value::<MergeConfidenceSummary>(legacy)
                .unwrap()
                .heuristic_version,
            0
        );
    }

    #[test]
    fn observed_heads_are_canonical_and_candidate_aliases_cannot_leak() {
        let candidate = "abcdefabcdefabcdefabcdefabcdefabcdefabcd";
        for alias in [
            candidate[..7].into(),
            candidate.to_uppercase(),
            format!(" {candidate} "),
            "synthetic-id".into(),
            "0".repeat(40),
        ] {
            let mut history = observed_history();
            history.records[0].head = alias;
            assert!(validate_history(&history).is_err());
        }
        let mut history = observed_history();
        history.records[0].head = "a".repeat(64);
        assert!(validate_history(&history).is_ok());
        history.synthetic = true;
        history.records[0].head = "opaque-synthetic-id".into();
        assert!(validate_history(&history).is_ok());
        let mut history = observed_history();
        history.records[0].head = candidate.into();
        for alias in [
            candidate.into(),
            candidate[..7].into(),
            candidate.to_uppercase(),
            format!(" {candidate} "),
        ] {
            let mut report = routine_report();
            report.reviewed_head = Some(alias.clone());
            let mut checks = check_evidence();
            checks.head = alias.clone();
            checks.reviewed_head = alias.clone();
            checks.checks[0].head = alias;
            let summary = assess(&report, Some(&history), Some(&policy()), Some(&checks)).unwrap();
            assert!(!summary.approval.eligible, "{:?}", summary.approval.reasons);
        }
    }

    #[test]
    fn check_evidence_requires_every_explicit_completeness_field() {
        let encoded = serde_json::to_value(check_evidence()).unwrap();
        assert!(serde_json::from_value::<CheckEvidence>(encoded.clone()).is_ok());
        for field in [
            "repository",
            "head",
            "reviewedHead",
            "assessedAt",
            "reviewComplete",
            "evidenceComplete",
            "supportedInputs",
            "auxiliaryUnknowns",
            "checks",
        ] {
            let mut missing = encoded.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(
                serde_json::from_value::<CheckEvidence>(missing).is_err(),
                "{field}"
            );
        }
    }

    fn recent_history() -> OutcomeHistory {
        let mut history = observed_history();
        history.training_cutoff = 4500;
        for (index, record) in history.records.iter_mut().enumerate() {
            record.merged_at = if index < 100 {
                4300 + index as i64
            } else {
                4600 + index as i64
            };
            record.score_recorded_at = record.merged_at - 1;
            for observed in [&mut record.revert, &mut record.incident, &mut record.flake]
                .into_iter()
                .flatten()
            {
                observed.observed_at = record.merged_at + 100;
            }
        }
        history
    }

    #[test]
    fn refreshed_asof_cannot_refresh_stale_observation_or_merge_cohorts() {
        let mut history = recent_history();
        let mut p = policy();
        p.max_history_age_seconds = 1000;
        let fresh = assess(
            &routine_report(),
            Some(&history),
            Some(&p),
            Some(&check_evidence()),
        )
        .unwrap();
        assert!(fresh.approval.eligible, "{:?}", fresh.approval.reasons);
        assert_eq!(
            fresh.outcomes[0]
                .evaluation
                .matching_bin_held_out_last_merged_at,
            Some(4739)
        );
        history.as_of = 100_000;
        let mut checks = check_evidence();
        checks.assessed_at = history.as_of;
        checks.checks[0].completed_at = history.as_of - 1;
        let stale = assess(&routine_report(), Some(&history), Some(&p), Some(&checks)).unwrap();
        assert!(!stale.approval.eligible);
        assert!(
            stale
                .outcomes
                .iter()
                .all(|outcome| outcome.status == "evaluatedEmpirical")
        );
        assert!(
            stale
                .approval
                .reasons
                .iter()
                .any(|reason| reason.contains("cohorts are stale"))
        );
        // Extending negative surveillance on ancient held-out merges does not
        // update the score producer/cohort or revive stale evaluation evidence.
        for record in history
            .records
            .iter_mut()
            .filter(|record| record.merged_at >= history.training_cutoff)
        {
            for observed in [&mut record.revert, &mut record.incident, &mut record.flake]
                .into_iter()
                .flatten()
            {
                if !observed.occurred {
                    observed.observed_at = history.as_of;
                }
            }
        }
        let refreshed_surveillance =
            assess(&routine_report(), Some(&history), Some(&p), Some(&checks)).unwrap();
        assert!(!refreshed_surveillance.approval.eligible);
        assert_eq!(
            refreshed_surveillance.outcomes[0]
                .evaluation
                .matching_bin_held_out_last_observed_at,
            Some(100_000)
        );
        assert_eq!(
            refreshed_surveillance.outcomes[0]
                .evaluation
                .matching_bin_held_out_last_merged_at,
            Some(4739)
        );
        // A genuinely recent, mature cohort and label export can pass the same
        // policy; this fabricated contract exercises mechanics only.
        let mut recent = recent_history();
        recent.as_of += 95_000;
        recent.training_cutoff += 95_000;
        for record in &mut recent.records {
            record.merged_at += 95_000;
            record.score_recorded_at += 95_000;
            for observed in [&mut record.revert, &mut record.incident, &mut record.flake]
                .into_iter()
                .flatten()
            {
                observed.observed_at += 95_000;
            }
        }
        let result = assess(&routine_report(), Some(&recent), Some(&p), Some(&checks)).unwrap();
        assert!(result.approval.eligible, "{:?}", result.approval.reasons);
    }

    #[test]
    fn sparse_history_and_single_outcome_class_remain_unknown() {
        let mut history = observed_history();
        history.records.truncate(2);
        assert!(
            assess(&routine_report(), Some(&history), None, None)
                .unwrap()
                .outcomes
                .iter()
                .all(|o| o.probability.is_none())
        );
        let mut history = observed_history();
        for record in &mut history.records {
            record.revert.as_mut().unwrap().occurred = false;
        }
        assert!(
            assess(&routine_report(), Some(&history), None, None)
                .unwrap()
                .outcomes[0]
                .probability
                .is_none()
        );
        history.records.clear();
        assert!(
            assess(&routine_report(), Some(&history), None, None)
                .unwrap()
                .outcomes
                .iter()
                .all(|o| o.probability.is_none())
        );
    }

    #[test]
    fn heldout_evidence_must_cover_current_score_bin() {
        let mut history = observed_history();
        for record in history
            .records
            .iter_mut()
            .filter(|r| r.merged_at >= history.training_cutoff)
        {
            record.heuristic_score = 0.9;
        }
        let summary = assess(&routine_report(), Some(&history), None, None).unwrap();
        assert!(summary.outcomes.iter().all(|o| o.probability.is_none()));
    }

    #[test]
    fn pooled_calibration_cannot_hide_current_bin_drift() {
        let mut history = observed_history();
        for record in history
            .records
            .iter_mut()
            .filter(|r| r.merged_at >= history.training_cutoff)
            .take(20)
        {
            record.revert.as_mut().unwrap().occurred = true;
        }
        for i in 0..1000 {
            let merged_at = 3000 + i;
            let observed = ObservedOutcome {
                occurred: true,
                observed_at: merged_at + 100,
                evidence: "fabricated held-out positive".into(),
            };
            history.records.push(OutcomeRecord {
                head: format!("{:040x}", i + 1000),
                merged_at,
                score_recorded_at: merged_at - 1,
                heuristic_score: 0.9,
                revert: Some(observed.clone()),
                incident: Some(observed.clone()),
                flake: Some(observed),
            });
        }
        let summary = assess(
            &routine_report(),
            Some(&history),
            Some(&policy()),
            Some(&check_evidence()),
        )
        .unwrap();
        let reverted = &summary.outcomes[0];
        assert!(
            reverted.evaluation.expected_calibration_error.unwrap()
                < policy().max_calibration_error
        );
        assert!(
            reverted.evaluation.matching_bin_calibration_error.unwrap()
                > policy().max_calibration_error
        );
        assert!(!summary.approval.eligible);
    }

    #[test]
    fn specification_drift_unknowns_and_missing_evidence_block_approval() {
        use crate::review::spec_drift::{
            SpecCheck, SpecCheckStatus, SpecDocument, SpecEvidence, SpecSummary,
        };
        let source = SpecEvidence {
            path: "src/main.rs".into(),
            start_line: 1,
            end_line: 1,
            text: "pub fn answer() -> i32 { 7 }".into(),
        };
        let spec = SpecEvidence {
            path: "spec/API.md".into(),
            start_line: 1,
            end_line: 1,
            text: "answer returns seven".into(),
        };
        let base = SpecSummary {
            documents: vec![SpecDocument {
                path: spec.path.clone(),
                content_hash: "fixture-hash".into(),
                bytes: 20,
                candidates: 1,
            }],
            checks: vec![SpecCheck {
                file: source.path.clone(),
                status: SpecCheckStatus::Matches,
                confidence: 0.9,
                spec: Some(spec),
                source: Some(source),
                reason: "Explicit source matches the stated return value".into(),
            }],
        };
        let mut report = routine_report();
        report.spec_drift = Some(base.clone());
        assert!(
            assess(
                &report,
                Some(&observed_history()),
                Some(&policy()),
                Some(&check_evidence())
            )
            .unwrap()
            .approval
            .eligible
        );
        for status in [
            SpecCheckStatus::Drift,
            SpecCheckStatus::Uncertain,
            SpecCheckStatus::Deferred,
        ] {
            let mut summary = base.clone();
            summary.checks[0].status = status;
            report.spec_drift = Some(summary);
            assert!(
                !assess(
                    &report,
                    Some(&observed_history()),
                    Some(&policy()),
                    Some(&check_evidence())
                )
                .unwrap()
                .approval
                .eligible
            );
        }
        let mut cases = Vec::new();
        let mut summary = base.clone();
        summary.checks[0].spec = None;
        cases.push(summary);
        let mut summary = base.clone();
        summary.checks[0].source = None;
        cases.push(summary);
        let mut summary = base.clone();
        summary.checks[0].confidence = f64::NAN;
        cases.push(summary);
        let mut summary = base.clone();
        summary.checks[0].source.as_mut().unwrap().start_line = 0;
        cases.push(summary);
        let mut summary = base.clone();
        summary.checks[0].source.as_mut().unwrap().path = "different.rs".into();
        cases.push(summary);
        let mut summary = base.clone();
        summary.documents.clear();
        cases.push(summary);
        let mut summary = base.clone();
        summary.checks.clear();
        cases.push(summary);
        for summary in cases {
            report.spec_drift = Some(summary);
            assert!(
                !assess(
                    &report,
                    Some(&observed_history()),
                    Some(&policy()),
                    Some(&check_evidence())
                )
                .unwrap()
                .approval
                .eligible
            );
        }
        let mut summary = base;
        summary.checks[0].status = SpecCheckStatus::NotApplicable;
        summary.checks[0].source = None;
        summary.checks[0].spec = None;
        report.spec_drift = Some(summary);
        assert!(
            assess(
                &report,
                Some(&observed_history()),
                Some(&policy()),
                Some(&check_evidence())
            )
            .unwrap()
            .approval
            .eligible
        );
    }

    #[test]
    fn all_incomplete_or_high_risk_review_paths_reject() {
        let mut cases = Vec::new();
        let mut r = routine_report();
        r.partial = true;
        cases.push(r);
        let mut r = routine_report();
        r.budget.deferred = 1;
        cases.push(r);
        let mut r = routine_report();
        r.workflow.dropped_context_chars = 1;
        cases.push(r);
        let mut r = routine_report();
        r.workflow.dropped_context_items = 1;
        cases.push(r);
        let mut r = routine_report();
        r.workflow.threshold_signals = 1;
        cases.push(r);
        let mut r = routine_report();
        r.workflow.needs_human_findings = 1;
        cases.push(r);
        let mut r = routine_report();
        r.workflow.suppressed_findings = 1;
        cases.push(r);
        let mut r = routine_report();
        r.tier.dismissed.push("ignored.rs".into());
        cases.push(r);
        let mut r = routine_report();
        r.mode = ReviewMode::Codebase;
        cases.push(r);
        let mut r = routine_report();
        r.reviewed_head = None;
        cases.push(r);
        let mut r = routine_report();
        r.reviewed_clean = false;
        cases.push(r);
        let mut r = routine_report();
        r.reviewed_committed = false;
        cases.push(r);
        let mut r = routine_report();
        r.reviewed_base = None;
        cases.push(r);
        let mut r = routine_report();
        r.dimensions.pop();
        cases.push(r);
        let mut r = routine_report();
        r.matrix[0].probabilities.remove(&Dimension::Security);
        cases.push(r);
        let mut r = routine_report();
        r.matrix.push(r.matrix[0].clone());
        r.screened_files = 2;
        cases.push(r);
        let mut r = routine_report();
        r.upgrades = Some(crate::review::upgrades::UpgradeSummary {
            unknowns: vec!["unsupported ecosystem".into()],
            ..Default::default()
        });
        cases.push(r);
        let mut r = routine_report();
        r.docs_drift = Some(crate::review::docs_drift::DocsDriftSummary {
            unknowns: vec!["insufficient context".into()],
            ..Default::default()
        });
        cases.push(r);
        let mut r = routine_report();
        r.findings.push(finding_with(Action::RequestChanges, 0.0));
        cases.push(r);
        let mut r = routine_report();
        r.findings.push(finding_with(Action::Comment, 2.0));
        cases.push(r);
        let mut r = routine_report();
        let mut f = finding_with(Action::Comment, 0.0);
        f.evidence.clear();
        r.findings.push(f);
        cases.push(r);
        cases.push(ReviewReport::default());
        for report in cases {
            assert!(
                !assess(
                    &report,
                    Some(&observed_history()),
                    Some(&policy()),
                    Some(&check_evidence())
                )
                .unwrap()
                .approval
                .eligible
            );
        }
    }

    #[test]
    fn missing_failed_stale_and_unsupported_check_evidence_rejects() {
        let mut cases = Vec::new();
        let mut c = check_evidence();
        c.checks.clear();
        cases.push(c);
        let mut c = check_evidence();
        c.checks[0].status = "failed".into();
        cases.push(c);
        let mut c = check_evidence();
        c.checks[0].status = "pending".into();
        cases.push(c);
        let mut c = check_evidence();
        c.checks[0].status = "unknown".into();
        cases.push(c);
        let mut c = check_evidence();
        c.checks[0].head = "other".into();
        cases.push(c);
        let mut c = check_evidence();
        c.reviewed_head = "other".into();
        cases.push(c);
        let mut c = check_evidence();
        c.head.clear();
        cases.push(c);
        let mut c = check_evidence();
        c.repository = "other/repo".into();
        cases.push(c);
        let mut c = check_evidence();
        c.review_complete = false;
        cases.push(c);
        let mut c = check_evidence();
        c.evidence_complete = false;
        cases.push(c);
        let mut c = check_evidence();
        c.supported_inputs = false;
        cases.push(c);
        let mut c = check_evidence();
        c.auxiliary_unknowns.push("missing changelog".into());
        cases.push(c);
        let mut c = check_evidence();
        c.checks[0].evidence.clear();
        cases.push(c);
        let mut c = check_evidence();
        c.checks[0].completed_at = 1;
        cases.push(c);
        let mut c = check_evidence();
        c.checks[0].completed_at = 5001;
        cases.push(c);
        let mut c = check_evidence();
        c.checks.push(c.checks[0].clone());
        cases.push(c);
        let mut c = check_evidence();
        c.head = observed_history().records[0].head.clone();
        c.reviewed_head = c.head.clone();
        c.checks[0].head = c.head.clone();
        cases.push(c);
        for checks in cases {
            assert!(
                !assess(
                    &routine_report(),
                    Some(&observed_history()),
                    Some(&policy()),
                    Some(&checks)
                )
                .unwrap()
                .approval
                .eligible
            );
        }
        assert!(
            !assess(
                &routine_report(),
                Some(&observed_history()),
                Some(&policy()),
                None
            )
            .unwrap()
            .approval
            .eligible
        );
        assert!(
            !assess(
                &routine_report(),
                None,
                Some(&policy()),
                Some(&check_evidence())
            )
            .unwrap()
            .approval
            .eligible
        );
        let mut checks = check_evidence();
        checks.assessed_at = 5000 + 31 * 86400;
        checks.checks[0].completed_at = checks.assessed_at;
        assert!(
            !assess(
                &routine_report(),
                Some(&observed_history()),
                Some(&policy()),
                Some(&checks)
            )
            .unwrap()
            .approval
            .eligible
        );
    }

    #[test]
    fn invalid_matrix_and_severity_never_produce_approval() {
        let mut report = routine_report();
        report.matrix[0]
            .probabilities
            .insert(Dimension::TestGap, f64::NAN);
        assert!(assess(&report, None, None, None).is_err());
        let mut report = routine_report();
        report
            .findings
            .push(finding_with(Action::Comment, f64::INFINITY));
        assert!(assess(&report, None, None, None).is_err());
    }

    #[test]
    fn supplied_but_disabled_policy_never_approves() {
        let mut p = policy();
        p.enabled = false;
        assert!(
            !assess(
                &routine_report(),
                Some(&observed_history()),
                Some(&p),
                Some(&check_evidence())
            )
            .unwrap()
            .approval
            .enabled
        );
        assert!(
            !assess(
                &routine_report(),
                Some(&observed_history()),
                None,
                Some(&check_evidence())
            )
            .unwrap()
            .approval
            .enabled
        );
    }

    #[test]
    fn policy_cannot_override_intrinsically_blocking_severity() {
        let mut p = policy();
        p.max_finding_severity = SEVERITY_MAX;
        let mut report = routine_report();
        report
            .findings
            .push(finding_with(Action::Comment, BLOCKING_SEVERITY));
        let summary = assess(
            &report,
            Some(&observed_history()),
            Some(&p),
            Some(&check_evidence()),
        )
        .unwrap();
        assert!(!summary.approval.eligible);
        assert!(
            summary
                .approval
                .reasons
                .iter()
                .any(|reason| reason.contains("Blocking or high-risk"))
        );
    }
}
