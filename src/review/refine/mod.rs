//! Post-locate refinement: mode-agnostic judgments over located findings.
//!
//! Each stage is a set of narrow `noul`/`choice` questions against a finding's
//! evidence plus the surrounding file (and, in codebase mode, 1-hop
//! neighbors), with its own budget from `domain::policy`:
//!
//! 1. `dedupe` (#10) — fold adjacent findings that share a root cause.
//! 2. `taint` (#13) — source → sanitized? → sink for injection-class findings;
//!    low exploitability demotes a blocking finding to a comment.
//! 3. `counterfactual` (#14) — which single fact would exonerate the finding,
//!    and does the context show it? A shown fact drops the finding.
//! 4. `ensemble` (#15) — re-screen blocking findings under varied focus; a
//!    split or negative vote flags the finding for a human.
//! 5. `pairwise` (#12) — A-vs-B "fix first?" over the top-K, ordered by
//!    Bradley-Terry strength.
//!
//! A failed judgment never fails the review: the finding is kept unrefined
//! and the failure is logged, as in the enrich stage.

mod counterfactual;
mod dedupe;
mod ensemble;
mod pairwise;
mod taint;

use std::cmp::Ordering;

use serde_json::{Value, json};

use crate::domain::policy::mechanisms;
use crate::domain::report::Finding;
use crate::review::explain::mechanism_title;
use crate::review::typesafe::TypeSafeClient;

/// Cap on each finding's evidence when several findings share one state.
const MAX_SUMMARY_EVIDENCE_CHARS: usize = 1_500;

/// What a refinement stage needs: the client, a context lookup, a logger, and
/// the request concurrency.
pub struct Refiner<'a> {
    pub client: &'a TypeSafeClient,
    /// `{ fileContext, neighbors }` for a finding.
    pub context: &'a dyn Fn(&Finding) -> Value,
    pub log: &'a dyn Fn(&str),
    pub concurrency: usize,
}

/// Counters for the report's `workflow` block.
#[derive(Debug, Default, PartialEq)]
pub struct RefineCounts {
    pub clustered: usize,
    pub exonerated: usize,
    pub needs_human: usize,
}

/// Runs every refinement stage. `findings` may be in any order; the result is
/// sorted by severity with the pairwise-ranked top-K first.
pub async fn refine(r: &Refiner<'_>, mut findings: Vec<Finding>) -> (Vec<Finding>, RefineCounts) {
    let mut counts = RefineCounts::default();
    sort_by_severity(&mut findings);

    (r.log)(&format!("Refining {} findings...", findings.len()));
    let (mut findings, clustered) = dedupe::dedupe(r, findings).await;
    counts.clustered = clustered;
    // Dedupe returns findings grouped by file; the budgeted stages below take
    // the first N, so restore severity order first.
    sort_by_severity(&mut findings);

    taint::trace(r, &mut findings).await;

    let (mut findings, exonerated) = counterfactual::calibrate(r, findings).await;
    counts.exonerated = exonerated;

    ensemble::vote(r, &mut findings).await;
    counts.needs_human = findings
        .iter()
        .filter(|f| f.ensemble.as_ref().is_some_and(|e| e.needs_human))
        .count();

    sort_by_severity(&mut findings);
    pairwise::rank(r, &mut findings).await;
    (findings, counts)
}

fn sort_by_severity(findings: &mut [Finding]) {
    findings.sort_by(|a, b| b.severity.partial_cmp(&a.severity).unwrap_or(Ordering::Equal));
}

/// The description of `finding`'s mechanism from the policy vocabulary.
fn mechanism_description(finding: &Finding) -> &'static str {
    mechanisms(finding.dimension)
        .iter()
        .find(|(key, _)| *key == finding.mechanism)
        .map(|(_, desc)| *desc)
        .unwrap_or("")
}

/// The concern a single-finding judgment is about.
fn concern(finding: &Finding) -> Value {
    json!({
        "dimension": finding.dimension,
        "definition": finding.dimension.definition(),
        "mechanism": finding.mechanism,
        "mechanismDescription": mechanism_description(finding),
        "severity": finding.severity,
    })
}

/// State for a single-finding judgment: the concern, its evidence, and the
/// surrounding code from `Refiner::context`.
fn finding_state(r: &Refiner<'_>, finding: &Finding) -> Value {
    let mut state = json!({
        "file": finding.file,
        "line": finding.line,
        "concern": concern(finding),
        "selectedEvidence": finding.evidence,
    });
    if let (Value::Object(state), Value::Object(context)) = (&mut state, (r.context)(finding)) {
        state.extend(context);
    }
    state
}

/// A compact finding for states that carry several findings at once.
fn summary(finding: &Finding) -> Value {
    let evidence: String = finding.evidence.chars().take(MAX_SUMMARY_EVIDENCE_CHARS).collect();
    json!({
        "file": finding.file,
        "line": finding.line,
        "dimension": finding.dimension,
        "mechanism": finding.mechanism,
        "title": mechanism_title(finding.dimension, &finding.mechanism),
        "severity": finding.severity,
        "evidence": evidence,
    })
}

/// Indices of the first `max` findings (in current order) matching `keep`.
fn top_indices(findings: &[Finding], max: usize, keep: impl Fn(&Finding) -> bool) -> Vec<usize> {
    findings
        .iter()
        .enumerate()
        .filter(|(_, f)| keep(f))
        .map(|(i, _)| i)
        .take(max)
        .collect()
}
