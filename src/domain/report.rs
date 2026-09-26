//! Report shapes shared by both review modes and the saved dashboard report.
//! Serde-only: no logic, no imports beyond `serde`, `serde_json`, and the
//! policy vocabulary. Mirrors `src/domain/types.ts`.

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

/// `Hunk { id, startLine, patch }` — a single diff hunk. Serialized camelCase
/// to match the prototype's wire shape in `candidateHunks`/`selectedEvidence`.
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigSnapshot {
    pub screen_threshold: f64,
    pub severity_max: f64,
    pub max_follow_ups: Option<usize>,
    pub max_profiles: usize,
}

/// Funnel counters reported in `workflow`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowCounts {
    pub screened_cells: usize,
    pub threshold_signals: usize,
    pub profiled_files: usize,
    pub followed_signals: usize,
    pub located_findings: usize,
    pub routed_findings: usize,
}

/// The full review report. `#[serde(default)]` reproduces the prototype's
/// deliberately loose `isReviewReport`: a report saved by an older version
/// stays viewable.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ReviewReport {
    pub mode: ReviewMode,
    pub scope: String,
    pub dimensions: Vec<DimensionMeta>,
    pub config: ConfigSnapshot,
    pub screened_files: usize,
    pub context_files: Vec<String>,
    pub matrix: Vec<MatrixRow>,
    pub followed_signals: usize,
    pub profiles: Vec<FileProfile>,
    pub workflow: WorkflowCounts,
    pub findings: Vec<Finding>,
    /// Uncalibrated heuristic P(revert) in [0, 1) from the review signals.
    /// A spike (see `review/merge_confidence.rs`); calibration is #17.
    #[serde(default)]
    pub merge_confidence: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::policy::Dimension;

    /// Pins the JSON wire contract shared with the dashboard client and the
    /// TypeSafe API: camelCase dimension keys and report fields, snake_case
    /// action, and the flattened matrix rows.
    #[test]
    fn wire_shape_matches_prototype() {
        let mut probabilities = std::collections::BTreeMap::new();
        probabilities.insert(Dimension::TestGap, 0.93);

        let report = ReviewReport {
            screened_files: 1,
            matrix: vec![MatrixRow { file: "src/main.rs".into(), probabilities }],
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
                title: None,
                why: None,
                fix: None,
                test: None,
            }],
            ..Default::default()
        };

        let value = serde_json::to_value(&report).unwrap();

        assert_eq!(value["screenedFiles"], 1);
        assert_eq!(value["screened_files"], serde_json::Value::Null);
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
        assert_eq!(report.findings.len(), 0);
    }
}