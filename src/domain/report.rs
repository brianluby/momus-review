//! Report shapes shared by both review modes and the saved dashboard report.
//!
//! These public structs carry data; construction and Serde deserialization do
//! not validate source identity, line bounds, probability ranges, score
//! consistency, or completeness. The review workflow and its adapters establish
//! those properties where applicable. Consumers of saved or externally supplied
//! reports must perform their own checks before publishing or making decisions.
//!
//! Where Serde defaults are enabled, older reports with missing fields remain
//! readable. A successfully deserialized report, an empty finding list, or `partial: false`
//! alone is not evidence that every input was reviewed. Keep skipped requests,
//! budget deferrals, context trimming, shard metadata, and optional artifacts'
//! unknowns visible to callers. Scores and confidence values describe judgments;
//! the wire types provide no empirical calibration guarantee.

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

/// A per-file change and its pre-change source, used for change review.
///
/// The Git adapter supplies a repository-relative path, a plain unified diff,
/// and `base` from the selected comparison revision; that revision need not be
/// HEAD. A new file has an empty base, and an untracked file has a synthetic
/// all-additions patch. Callers constructing this struct must keep these three
/// values aligned: neither construction nor deserialization checks them. An
/// empty base is also valid content and does not by itself identify a new file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangedFile {
    pub path: String,
    pub patch: String,
    #[serde(default)]
    pub base: String,
}

/// A complete source file as supplied by the repository adapter for codebase review.
///
/// The path identifies the file in the repository inventory. This plain value
/// does not prove that `content` came from that path or a particular revision;
/// callers supplying source must bind that identity separately.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceFile {
    pub path: String,
    pub content: String,
}

/// Selectable diff evidence sent as `candidateHunks` or `selectedEvidence`.
///
/// `start_line` names the new-file position in the `@@` header, generally
/// one-based; a zero-length range can name the position 0 before the file.
/// `patch` includes the header and diff markers, so text offsets into it are
/// not file positions. IDs identify candidates within one selection request,
/// not findings across runs. [`super::patch::split_hunk`] leaves IDs empty on
/// actual split pieces, and its caller assigns unique candidate IDs.
/// Deserialization does not check that the header, start, or ID agree.
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

/// Suggested review action, serialized as `comment` or `request_changes`.
///
/// The ordinary finding producer chooses `RequestChanges` at or above
/// [`super::policy::BLOCKING_SEVERITY`]. This enum neither recomputes that choice
/// from a finding's severity nor grants permission to publish or approve a PR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    #[default]
    Comment,
    RequestChanges,
}

/// A located, classified, scored, and routed finding.
///
/// Producers choose `line` in the new file for change review or in the source
/// file for codebase review. The change-review locator uses the first added
/// line of selected evidence, with a hunk-start fallback for deletions; that
/// fallback need not be an available RIGHT-side comment anchor. Publishing
/// callers must check the file inventory and commentable lines independently.
///
/// An ordinary model finding has passed the locator's evidence and skeptical
/// judgment gates; a rejected or unsupported signal produces no finding.
/// Locally produced advisory findings have their own evidence rules. This
/// struct records the result, not the rejected signals or a proof of the claim.
/// Deserialized findings need not satisfy either producer's guarantees: numeric
/// ranges, mechanism labels, action/severity consistency, and source bindings
/// are not validated here. Optional fix, test, and test-plan text is guidance,
/// not an applied change or executed test result.
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
    /// Opt-in, unfinished scaffold and assertion guidance; never applied or executed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_plan: Option<crate::review::test_planner::TestPlan>,
    /// Identity computed by the feedback fingerprint function from file,
    /// dimension, mechanism, and normalized evidence, excluding line numbers
    /// and diff headers and collapsing whitespace. Producers use it for
    /// feedback and suppression; meaningful evidence changes can change the
    /// identity. Empty on unpopulated or legacy values, and not verified when
    /// a finding is deserialized.
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
///
/// These are model judgments over visible context, not a static-analysis proof
/// of a path. The refinement producer calculates `exploitability` from its
/// components, but deserialization does not recompute or range-check it.
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
///
/// A missing check is different from a performed check that rejects the fact.
/// `holds` concerns supplied context; it does not establish facts outside it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Exoneration {
    /// Fact label from `policy::EXONERATING_FACTS`.
    pub fact: String,
    pub fact_confidence: f64,
    /// P(the visible context establishes the fact).
    pub holds: f64,
}

/// Repeated screens of a high-stakes finding under varied focus.
///
/// The producer calculates `mean`, `spread`, and `needs_human` from the votes
/// using policy thresholds. Repetition does not establish statistical
/// independence or calibrated accuracy, and Serde does not verify that stored
/// aggregates agree with the stored votes.
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
///
/// The ordinary workflow inserts a row for each successful source screen. A
/// missing row can represent skipped, excluded, or undiscovered input, rather
/// than a zero probability. The map permits a subset of dimensions and does
/// not validate values or establish that all policy dimensions were screened.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MatrixRow {
    pub file: String,
    #[serde(flatten)]
    pub probabilities: BTreeMap<Dimension, f64>,
}

/// The `config` snapshot embedded in a report (thresholds/budget ceilings).
/// `max_follow_ups: None` removes the follow-up-count cap; a separate call
/// budget or request failure can still prevent following every threshold signal.
/// `screen_thresholds` holds the per-dimension thresholds actually applied
/// (feedback-tuned; `screen_threshold` stays the policy default).
/// `Default` and missing Serde fields are compatibility defaults, not a
/// substitute for the configured workflow's actual policy snapshot.
///
/// The 0.2 Rust API adds `follow_up_strategy`; construct unspecified fields
/// through `Default` when migrating an exhaustive 0.1 struct literal:
///
/// ```
/// use momus_review::domain::report::ConfigSnapshot;
/// use momus_review::review::voi::FollowUpStrategy;
/// let config = ConfigSnapshot { screen_threshold: 0.7, ..Default::default() };
/// assert_eq!(config.follow_up_strategy, FollowUpStrategy::Probability);
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ConfigSnapshot {
    pub screen_threshold: f64,
    pub screen_thresholds: BTreeMap<Dimension, f64>,
    pub severity_max: f64,
    pub max_follow_ups: Option<usize>,
    pub max_profiles: usize,
    /// Selection policy; probability remains the default for older reports.
    pub follow_up_strategy: crate::review::voi::FollowUpStrategy,
}

/// Funnel counters reported in `workflow`.
///
/// The workflow supplies these stage totals and refinement/drop counts. They
/// need not equal the final finding count: suppression and refinement remove
/// findings, and auxiliary checks can append findings separately. Positive
/// context-drop counters mean some supplied context was unavailable to a
/// judgment even if `ReviewReport::partial` is false. Zero or defaulted counters
/// alone do not prove complete review, and Serde enforces no cross-field sums.
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
    /// not fit screening or attached test-plan context (`review::context`).
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
    TestPlan,
}

impl ReviewStage {
    /// Return the stable serialized label of the failed workflow stage.
    pub fn key(self) -> &'static str {
        match self {
            ReviewStage::Screen => "screen",
            ReviewStage::Profile => "profile",
            ReviewStage::Locate => "locate",
            ReviewStage::TestPlan => "testplan",
        }
    }
}

/// Work that failed or was deferred and was skipped while the review continued.
///
/// This can follow exhausted client retries, a budget refusal before an HTTP
/// attempt, or an optional test-plan failure. A screen failure loses that
/// file's screen; profile and locate failures lose their stage's result; a
/// test-plan failure preserves the finding without attaching a plan. The
/// workflow marks reports with skipped work partial, rather than interpreting
/// the absent result as a clean finding or an evidence-based rejection.
/// `reason` is shortened diagnostic text, not a stable machine error code.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkippedFile {
    /// The file; for a locate, also the signal's dimension (`a.rs [security]`).
    pub file: String,
    pub stage: ReviewStage,
    /// The error, shortened.
    pub reason: String,
}

/// Successful System One responses and reported token totals across client clones.
///
/// `calls` excludes cache hits and failed HTTP attempts, including failed
/// retries. Token totals accumulate only when a successful server response
/// includes usage; zero does not distinguish absent usage from measured zero.
/// Use [`BudgetSummary`] to inspect reserved attempts and deferred work, rather
/// than treating this value as a complete request or cost ledger.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UsageSummary {
    pub calls: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache: CacheSummary,
}

/// Result-cache counters inside `usage`: work units answered from the
/// local cache versus work that did not yield a usable cached response.
///
/// A hit avoids a System One request. A miss can still be refused by the call
/// budget or fail, and is counted even when the cache is disabled; it does not
/// prove that an HTTP request succeeded or was sent. `UsageSummary::calls`
/// counts successful uncached responses, not misses or retry attempts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CacheSummary {
    pub hits: u64,
    pub misses: u64,
}

/// Repository-index work reused, recomputed, or handled by fallback this run.
///
/// These are optimization diagnostics, not source-coverage or validity counts.
/// Defaulted zeros do not establish that an index lookup was attempted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct IndexStats {
    pub computed: usize,
    pub reused: usize,
    pub fallbacks: usize,
}

/// Snapshot of the shared HTTP-attempt reservation budget.
///
/// `limit: None` removes the attempt ceiling. `reserved` counts permits obtained
/// before sending, including retry attempts, whereas `deferred` counts refused
/// reservations. A reservation does not prove a successful response. Cached
/// work does not reserve an attempt; these counters are distinct from usage.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BudgetSummary {
    pub limit: Option<u64>,
    pub reserved: u64,
    pub deferred: u64,
}
/// Optional tier-screen results, including paths dismissed before full screening.
///
/// A dismissed path is reduced review coverage, not a fully screened clean file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TierSummary {
    pub screened: usize,
    pub dismissed: Vec<String>,
}

/// Provenance needed to combine a complete set of codebase-review shards.
///
/// Producers use a one-based `index` within `count` and carry the full discovered
/// path inventory, inventory key, refinement choice, and model identity into
/// every shard. One shard alone is partial. Deserialization validates none of
/// these relationships; the shard merge operation rejects missing, duplicate,
/// wrongly assigned, or incompatible parts before producing a combined report.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShardMetadata {
    pub index: usize,
    pub count: usize,
    pub inventory_key: String,
    pub expected_paths: Vec<String>,
    pub refine: bool,
    pub model: String,
}

/// The full review report. `#[serde(default)]` keeps reads deliberately
/// tolerant: a report saved by an older version stays viewable.
///
/// This is an interchange container, not a validated review receipt. Defaults
/// include absent source identities, no findings, and `partial: false`, so even
/// an empty JSON object can deserialize. Before treating absence of findings
/// as review evidence, a caller must establish the intended scope and revision,
/// inventory and dimensions, successful required stages, and absence of relevant
/// deferrals, context drops, shard omissions, and auxiliary unknowns. Approval
/// decisions additionally use their dedicated evidence and policy gates.
///
/// The ordinary workflow's `partial` flag covers known skipped, deferred,
/// capped, tier-dismissed, and shard work; attached checks can add uncertainty.
/// It is not an attestation that every possible defect was assessed. Likewise,
/// missing optional artifacts mean unassessed or disabled work, not a negative
/// judgment. Raw local evidence may contain credentials: redaction of outgoing
/// requests does not automatically sanitize this report for publication.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ReviewReport {
    /// Source-review identity recorded by the repository review producer;
    /// absent in legacy reports. Auxiliary evidence cannot replace it and
    /// invalidates producer verification on mismatch. Stored strings alone are
    /// not trusted evidence of which revision was read.
    pub reviewed_head: Option<String>,
    pub reviewed_base: Option<String>,
    /// The producer sets this only when cleanliness was verified for the
    /// bound identity.
    /// False includes missing verification; it does not establish dirtiness.
    pub reviewed_clean: bool,
    /// The producer sets this only for source inputs verified against the
    /// pinned committed tree.
    /// False includes missing verification and always rejects approval.
    pub reviewed_committed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upgrades: Option<crate::review::upgrades::UpgradeSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docs_drift: Option<crate::review::docs_drift::DocsDriftSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Assessment artifact, including explicit unknowns when history is absent.
    /// Presence does not mean historical calibration or approval was enabled.
    pub merge_confidence: Option<crate::review::merge_confidence::MergeConfidenceSummary>,
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
    pub budget: BudgetSummary,
    /// Auditable heuristic follow-up priorities, when VOI was requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub follow_up_plan: Option<crate::review::voi::VoiSummary>,
    /// Advisory comparison with explicitly supplied local requirements.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec_drift: Option<crate::review::spec_drift::SpecSummary>,
    /// Known missing work or evidence in the producing workflow; false alone
    /// does not establish completeness, especially for legacy/defaulted data.
    pub partial: bool,
    pub tier: TierSummary,
    pub shard: Option<ShardMetadata>,
    /// Hash-deduplicated values matched by outgoing-request redaction, per rule.
    /// Empty when nothing matched or redaction was off; neither case proves
    /// absence of secrets. This does not describe sanitization of saved evidence.
    pub redactions: BTreeMap<String, usize>,
    /// Failed or deferred requests that were skipped rather than ending the run.
    /// An empty list can also come from defaults and is not completeness proof.
    pub skipped: Vec<SkippedFile>,
    pub findings: Vec<Finding>,
    /// Uncalibrated revert-risk heuristic produced from review signals.
    /// The producer returns a value in [0, 1); individual shard reports use 0
    /// until merging. It is not a measured failure probability, and a stored
    /// value is neither range-checked nor recomputed by Serde.
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

    /// Legacy report/config fields retain their values without any new artifacts.
    #[test]
    fn legacy_report_keeps_probability_policy_and_optional_artifacts_absent() {
        let mut legacy_finding = serde_json::to_value(Finding {
            file: "src/lib.rs".into(),
            line: 7,
            dimension: Dimension::Security,
            mechanism: "sqlInjection".into(),
            evidence: "query(input)".into(),
            ..Default::default()
        })
        .unwrap();
        legacy_finding.as_object_mut().unwrap().remove("testPlan");
        let report: ReviewReport = serde_json::from_value(serde_json::json!({
            "scope":"src", "screenedFiles":1, "findings":[legacy_finding],
            "config": { "screenThreshold":0.7, "screenThresholds":{"security":0.8},
                "severityMax":3.0, "maxFollowUps":4, "maxProfiles":8 }
        }))
        .unwrap();
        assert_eq!(
            report.config.follow_up_strategy,
            crate::review::voi::FollowUpStrategy::Probability
        );
        assert_eq!(report.config.screen_thresholds[&Dimension::Security], 0.8);
        assert_eq!(report.config.max_follow_ups, Some(4));
        assert_eq!(report.findings[0].file, "src/lib.rs");
        assert_eq!(report.findings[0].line, 7);
        assert_eq!(report.findings[0].mechanism, "sqlInjection");
        assert!(report.findings[0].test_plan.is_none());
        assert!(report.follow_up_plan.is_none() && report.spec_drift.is_none());
    }
}
