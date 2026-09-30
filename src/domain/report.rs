//! Report shapes shared by both review modes and the saved dashboard report.
//! Serde-only: no logic, no imports beyond `serde`, `serde_json`, and the
//! policy vocabulary.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::domain::policy::{Dimension, DimensionMeta};

/// `ReviewMode = "changes" | "codebase"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReviewMode {
    #[default]
    Changes,
    Codebase,
}

/// `ChangedFile { path, patch, base }` — a tracked/untracked diff. `base` is
/// the pre-change (HEAD) content; empty for a newly added file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangedFile {
    pub path: String,
    pub patch: String,
    #[serde(default)]
    pub base: String,
}

/// `SourceFile { path, content }` — a complete source file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceFile {
    pub path: String,
    pub content: String,
}

/// `Hunk { id, startLine, patch }` — a single diff hunk. Serialized camelCase,
/// the wire shape judgments send in `candidateHunks`/`selectedEvidence`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hunk {
    pub id: String,
    pub start_line: usize,
    pub patch: String,
}

/// `FileProfile` — per-file triage aid, not gating.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileProfile {
    pub file: String,
    pub category: String,
    pub category_confidence: f64,
    pub review_priority: f64,
    pub review_priority_confidence: f64,
}

/// The action derived from severity: ≥ BLOCKING_SEVERITY requests changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    #[default]
    Comment,
    RequestChanges,
}

/// A located, classified, scored, and routed finding.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub file: String,
    pub line: usize,
    pub dimension: Dimension,
    pub probability: f64,
    pub location_confidence: f64,
    pub mechanism: String,
    pub mechanism_confidence: f64,
    pub severity: f64,
    pub severity_confidence: f64,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub owner_confidence: Option<f64>,
    pub action: Action,
    /// The concrete evidence excerpt (diff hunk or source region) that
    /// supports this finding.
    #[serde(default)]
    pub evidence: String,
    /// A concise human title derived deterministically from the mechanism.
    #[serde(default)]
    pub title: Option<String>,
    /// The matching mechanism description (why this concern matters).
    #[serde(default)]
    pub why: Option<String>,
    /// A model-selected fix strategy (actionable description).
    #[serde(default)]
    pub fix: Option<String>,
    /// A model-selected test strategy (actionable description).
    #[serde(default)]
    pub test: Option<String>,
    /// Stable identity across runs (file, dimension, mechanism, evidence text;
    /// not line numbers), keying feedback and the suppression list.
    #[serde(default)]
    pub fingerprint: String,
    /// Other located findings judged to share this finding's root cause and
    /// folded into it by the dedupe stage.
    #[serde(default)]
    pub related: Vec<RelatedFinding>,
    /// 1-based position in the pairwise (Bradley-Terry) fix-first ranking;
    /// `None` outside the ranked top-K.
    #[serde(default)]
    pub rank: Option<usize>,
    /// Source → sanitizer → sink chain for injection-class security findings.
    #[serde(default)]
    pub taint: Option<TaintChain>,
    /// The single fact that would exonerate this finding, and whether the
    /// visible context shows it.
    #[serde(default)]
    pub exoneration: Option<Exoneration>,
    /// Re-asked screen votes for a high-stakes finding.
    #[serde(default)]
    pub ensemble: Option<Ensemble>,
}

/// A finding folded into another as the same root cause.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelatedFinding {
    pub file: String,
    pub line: usize,
    pub dimension: Dimension,
    pub mechanism: String,
    /// The dedupe judgment's confidence that the root cause is shared.
    pub confidence: f64,
}

/// A composed taint judgment: where the data comes from, whether it is
/// neutralized, and whether it reaches the dangerous sink.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaintChain {
    /// Source label from `policy::TAINT_SOURCES`.
    pub source: String,
    pub source_confidence: f64,
    /// P(the data originates outside the trust boundary).
    pub untrusted: f64,
    /// P(the data reaches the mechanism's sink).
    pub reaches_sink: f64,
    /// P(the data is validated, escaped, or parameterized for that sink);
    /// `None` when not assessed because the sink is unlikely to be reached.
    pub sanitized: Option<f64>,
    /// untrusted × reaches_sink × (1 − sanitized), with an unassessed
    /// sanitizer counted as 0 (the low `reaches_sink` already dominates).
    pub exploitability: f64,
}

/// A counterfactual check: the exonerating fact and P(the context shows it).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Exoneration {
    /// Fact label from `policy::EXONERATING_FACTS`.
    pub fact: String,
    pub fact_confidence: f64,
    /// P(the visible context establishes the fact).
    pub holds: f64,
}

/// Independent re-screens of a high-stakes finding under varied focus.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ensemble {
    /// One probability per focus variant, in `policy::ENSEMBLE_FOCI` order.
    pub votes: Vec<f64>,
    pub mean: f64,
    /// max − min over `votes`.
    pub spread: f64,
    /// The votes disagree, or on balance reject the finding: a human decides.
    pub needs_human: bool,
}

/// `matrix: Array<{ file } & Record<Dimension, number>>`. The per-file
/// probability matrix flattens `Record<Dimension, number>` onto the row.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MatrixRow {
    pub file: String,
    #[serde(flatten)]
    pub probabilities: BTreeMap<Dimension, f64>,
}

/// The `config` snapshot embedded in a report (thresholds/budget ceilings).
/// `max_follow_ups: None` means unlimited (follow up every threshold signal).
/// `screen_thresholds` holds the per-dimension thresholds actually applied
/// (feedback-tuned; `screen_threshold` stays the policy default).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ConfigSnapshot {
    pub screen_threshold: f64,
    pub screen_thresholds: BTreeMap<Dimension, f64>,
    pub severity_max: f64,
    pub max_follow_ups: Option<usize>,
    pub max_profiles: usize,
}

/// Funnel counters reported in `workflow`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WorkflowCounts {
    pub screened_cells: usize,
    pub threshold_signals: usize,
    pub profiled_files: usize,
    pub followed_signals: usize,
    pub located_findings: usize,
    pub routed_findings: usize,
    /// Dropped by the feedback suppression list.
    pub suppressed_findings: usize,
    /// Folded into another finding as the same root cause.
    pub clustered_findings: usize,
    /// Dropped because the context shows an exonerating fact.
    pub exonerated_findings: usize,
    /// Flagged for a human by ensemble disagreement.
    pub needs_human_findings: usize,
    /// Context characters trimmed off (or wholly dropped) because they did
    /// not fit a screen request's budget (`review::context`).
    pub dropped_context_chars: usize,
    /// Context items (unit, base, test, neighbor, region) so trimmed.
    pub dropped_context_items: usize,
}

/// The funnel stage a request belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReviewStage {
    Screen,
    Profile,
    Locate,
}

impl ReviewStage {
    pub fn key(self) -> &'static str {
        match self {
            ReviewStage::Screen => "screen",
            ReviewStage::Profile => "profile",
            ReviewStage::Locate => "locate",
        }
    }
}

/// A request that failed after the client's retries and was skipped, so the
/// rest of the review could go on: that file (screen), triage aid (profile),
/// or signal (locate) is missing from the report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkippedFile {
    /// The file; for a locate, also the signal's dimension (`a.rs [security]`).
    pub file: String,
    pub stage: ReviewStage,
    /// The error, shortened.
    pub reason: String,
}

/// Per-run System One usage accumulated across the whole review: successful
/// calls plus token totals when the server reports them (hosted Jev omits
/// usage, so only `calls` fills in there).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UsageSummary {
    pub calls: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache: CacheSummary,
}

/// Result-cache counters inside `usage`: work units answered from the
/// local cache vs sent to the API (`adapters::cache`). A hit is not a Jev
/// call — `usage.calls` counts only requests that left the machine.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CacheSummary {
    pub hits: u64,
    pub misses: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct IndexStats {
    pub computed: usize,
    pub reused: usize,
    pub fallbacks: usize,
}

/// The full review report. `#[serde(default)]` keeps reads deliberately
/// tolerant: a report saved by an older version stays viewable.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ReviewReport {
    pub mode: ReviewMode,
    pub scope: String,
    pub dimensions: Vec<DimensionMeta>,
    pub config: ConfigSnapshot,
    /// Review wall time, excluding report serialization/publication.
    pub wall_time_ms: u64,
    pub screened_files: usize,
    pub context_files: Vec<String>,
    pub matrix: Vec<MatrixRow>,
    pub followed_signals: usize,
    pub profiles: Vec<FileProfile>,
    pub workflow: WorkflowCounts,
    pub usage: UsageSummary,
    pub index: IndexStats,
    /// Distinct secret values redacted from outgoing requests, per rule
    /// (`domain::redact`); empty when nothing matched or redaction was off.
    pub redactions: BTreeMap<String, usize>,
    /// Requests that failed and were skipped rather than ending the run;
    /// empty on a clean run.
    pub skipped: Vec<SkippedFile>,
    pub findings: Vec<Finding>,
    /// Uncalibrated heuristic P(revert) in [0, 1) from the review signals.
    /// A spike (see `review/merge_confidence.rs`); calibration is #17.
    #[serde(default)]
    pub p_revert: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::policy::Dimension;

    /// Pins the JSON wire contract shared with the dashboard client and the
    /// TypeSafe API: camelCase dimension keys and report fields, snake_case
    /// action, and the flattened matrix rows.
    #[test]
    fn wire_shape_is_pinned() {
        let mut probabilities = std::collections::BTreeMap::new();
        probabilities.insert(Dimension::TestGap, 0.93);

        let report = ReviewReport {
            screened_files: 1,
            matrix: vec![MatrixRow {
                file: "src/main.rs".into(),
                probabilities,
            }],
            findings: vec![Finding {
                file: "src/main.rs".into(),
                line: 1,
                dimension: Dimension::TestGap,
                probability: 0.93,
                location_confidence: 0.9,
                mechanism: "boundary".into(),
                mechanism_confidence: 0.5,
                severity: 2.1,
                severity_confidence: 0.8,
                owner: Some("testing".into()),
                owner_confidence: Some(0.7),
                action: Action::RequestChanges,
                evidence: "@@ -1,2 +1,2 @@\n-foo\n+bar".into(),
                ..Default::default()
            }],
            ..Default::default()
        };

        let value = serde_json::to_value(&report).unwrap();

        assert_eq!(value["usage"]["calls"], 0);
        assert_eq!(value["redactions"], serde_json::json!({}));
        assert_eq!(value["usage"]["inputTokens"], 0);
        assert_eq!(value["screenedFiles"], 1);
        assert_eq!(value["screened_files"], serde_json::Value::Null);
        assert_eq!(value["usage"]["input_tokens"], serde_json::Value::Null);
        assert_eq!(value["matrix"][0]["testGap"], 0.93);
        assert_eq!(value["findings"][0]["dimension"], "testGap");
        assert_eq!(value["findings"][0]["action"], "request_changes");
        assert_eq!(value["findings"][0]["locationConfidence"], 0.9);
    }

    /// A report saved by an older version (missing fields) still deserializes.
    #[test]
    fn tolerant_read_of_partial_report() {
        let report: ReviewReport = serde_json::from_str(
            r#"{ "scope": "x", "screenedFiles": 3, "matrix": [], "followedSignals": 0, "findings": [] }"#,
        )
        .unwrap();
        assert_eq!(report.scope, "x");
        assert_eq!(report.screened_files, 3);
        assert_eq!(report.usage.calls, 0);
        assert_eq!(report.findings.len(), 0);
    }
}
