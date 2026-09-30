//! The per-mode strategy abstraction and shared workflow types: each mode
//! implements `ReviewStrategy`, and `workflow::run_review` drives it.

use std::path::PathBuf;

use anyhow::Result;
use serde_json::{Value, json};

use crate::domain::patch::base_line;
use crate::domain::policy::{Dimension, Probabilities};
use crate::domain::report::{ChangedFile, FileProfile, Finding, ReviewMode, SourceFile};
use crate::review::typesafe::TypeSafeClient;

/// Lines on each side of a finding carried as refinement context.
const CONTEXT_RADIUS: usize = 60;
/// Cap on a changed file's patch carried as refinement context.
const MAX_CONTEXT_PATCH_CHARS: usize = 6_000;

/// A file payload discovered by a mode: anything with a repo-relative path.
pub trait FileEntry: std::fmt::Debug + Clone + serde::Serialize {
    /// Return the repository-relative identity used for context, cache and shard selection.
    fn path(&self) -> &str;
    /// Tier 0 never dismisses a unit too large to inspect in full.
    fn tier_state(&self) -> Option<Value> {
        None
    }

    /// The file around `line`, as `fileContext` state for refinement
    /// judgments (dedupe, taint, counterfactual, ensemble).
    fn context_around(&self, line: usize) -> Value;
}

impl FileEntry for ChangedFile {
    /// Return the repository-relative identity used for context, cache and shard selection.
    fn path(&self) -> &str {
        &self.path
    }

    /// `line` is a new-file line (from the patch's `+` ranges); the base
    /// window is centered on the matching pre-change line.
    fn context_around(&self, line: usize) -> Value {
        let patch: String = self.patch.chars().take(MAX_CONTEXT_PATCH_CHARS).collect();
        let (base_start, base) = window(&self.base, base_line(&self.patch, line), CONTEXT_RADIUS);
        json!({ "path": self.path, "patch": patch, "baseStartLine": base_start, "base": base })
    }
}

impl FileEntry for SourceFile {
    /// Return the repository-relative identity used for context, cache and shard selection.
    fn path(&self) -> &str {
        &self.path
    }

    /// Return complete source content only when it fits the configured Tier-0 budget.
    fn tier_state(&self) -> Option<Value> {
        let mut budget = crate::review::context::ContextBudget::from_env().ok()?;
        let content = budget.take(&self.content);
        if budget.drops.chars > 0 {
            return None;
        }
        Some(json!({ "file": { "path": self.path, "content": content } }))
    }

    /// Return a bounded source-line window centered on the finding.
    fn context_around(&self, line: usize) -> Value {
        let (start_line, content) = window(&self.content, line, CONTEXT_RADIUS);
        json!({ "path": self.path, "startLine": start_line, "content": content })
    }
}

/// The 1-based lines of `text` within `radius` of `line`, and the first line's
/// number.
/// Only the requested lines are collected, so large files stay cheap.
fn window(text: &str, line: usize, radius: usize) -> (usize, String) {
    let total = text.lines().count();
    if total == 0 {
        return (1, String::new());
    }
    let center = line.clamp(1, total) - 1;
    let start = center.saturating_sub(radius);
    let end = (center + radius + 1).min(total);
    let lines: Vec<&str> = text.lines().skip(start).take(end - start).collect();
    (start + 1, lines.join("\n"))
}

/// The files a mode discovers, split into review subjects and test context.
/// Both roles use the same concrete type in each mode, hence one type param.
pub struct Discovery<F> {
    pub files: Vec<F>,
    pub context_files: Vec<F>,
}

/// A screening result: a file plus its per-dimension probability matrix.
pub struct Screening<F> {
    pub file: F,
    pub probabilities: Probabilities,
    /// Context that did not fit this screen's budget (never sent).
    pub dropped: crate::review::context::ContextDrops,
}

/// A single dimension's probability for a file, above threshold → follow-up.
#[derive(Debug, Clone)]
pub struct Signal<F> {
    pub file: F,
    pub dimension: Dimension,
    pub probability: f64,
}

/// A review mode. Owns discovery and judgments; `workflow` owns concurrency,
/// thresholds, ranking, and report assembly.
///
/// `async fn` in the trait is deliberate: futures are driven concurrently in
/// a single task via `buffered` (not `tokio::spawn`), so no `Send` bound is
/// required on the futures.
#[allow(async_fn_in_trait)]
pub trait ReviewStrategy: Send + Sync {
    type File: FileEntry + Send + Sync + 'static;

    /// Identify whether the strategy reviews diffs or complete source files.
    fn mode(&self) -> ReviewMode;
    /// Name the review subjects in workflow progress messages.
    fn subject(&self) -> &'static str;
    /// Name the accompanying test-context inventory in progress messages.
    fn context_label(&self) -> &'static str;

    /// The System One client, for mode-agnostic refinement judgments.
    fn client(&self) -> &TypeSafeClient;
    /// 1-hop callers/callees of `path` as refinement context (`Null` when the
    /// mode has none).
    fn neighbor_context(&self, _path: &str) -> Value {
        Value::Null
    }

    /// Precompute incremental context once, before concurrent requests.
    fn prepass(&self, _scopes: &[PathBuf], _discovery: &Discovery<Self::File>) -> Result<()> {
        Ok(())
    }
    /// Report metadata computations, reuse and fallbacks from the pre-pass.
    fn index_stats(&self) -> crate::review::index::IndexStats {
        Default::default()
    }

    /// Discover review subjects and their test-context files under the requested scopes.
    fn discover(&self, scopes: &[PathBuf]) -> Result<Discovery<Self::File>>;
    /// Estimate per-dimension concern probabilities using bounded context.
    async fn screen(
        &self,
        file: &Self::File,
        context: &[Self::File],
    ) -> Result<Screening<Self::File>>;
    /// Produce non-gating file-role and review-priority metadata.
    async fn profile(
        &self,
        file: &Self::File,
        probabilities: &Probabilities,
    ) -> Result<FileProfile>;
    /// Locate concrete evidence for one threshold signal, or return no finding.
    async fn locate(&self, signal: &Signal<Self::File>) -> Result<Option<Finding>>;
    /// Enrich a located finding with a suggested fix and test strategy.
    async fn suggestions(&self, finding: &Finding) -> Result<(Option<String>, Option<String>)>;
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_clamps_to_the_file() {
        let text = (1..=10)
            .map(|i| format!("l{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(window(&text, 5, 2), (3, "l3\nl4\nl5\nl6\nl7".to_string()));
        assert_eq!(window(&text, 1, 2), (1, "l1\nl2\nl3".to_string()));
        assert_eq!(window(&text, 99, 1), (9, "l9\nl10".to_string()));
        assert_eq!(window("", 3, 2), (1, String::new()));
    }
}
