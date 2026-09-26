//! Shared staged orchestration. Each review mode owns discovery and judgments;
//! this module owns concurrency, thresholds, ranking, and report assembly.
//! Mirrors `review/workflow.ts`.

use std::cmp::Ordering;
use std::path::PathBuf;

use anyhow::{Result, anyhow};
use futures::{StreamExt, stream};

use std::collections::HashMap;

use serde_json::json;

use crate::domain::feedback::{FeedbackLog, fingerprint};
use crate::domain::policy::{
    CONCURRENCY, DIMENSIONS, MAX_PROFILES, SCREEN_THRESHOLD, SEVERITY_MAX, Dimension,
    dimension_metadata,
};
use crate::domain::report::{
    ConfigSnapshot, FileProfile, Finding, MatrixRow, ReviewReport, WorkflowCounts,
};
use crate::review::explain::{self, MAX_ENRICH};
use crate::review::merge_confidence;
use crate::review::refine::{self, RefineCounts, Refiner};
use crate::review::strategy::{Discovery, FileEntry, ReviewStrategy, Screening, Signal};

/// Per-run knobs beyond the scope and strategy.
#[derive(Debug, Default)]
pub struct ReviewOptions {
    /// Cap follow-ups per `select_follow_ups`; `None` follows every signal.
    pub max_follow_ups: Option<usize>,
    /// Run the post-locate refinement stages (`review::refine`).
    pub refine: bool,
    /// Reviewer feedback: tunes per-dimension thresholds and suppresses
    /// findings (`domain::feedback`).
    pub feedback: FeedbackLog,
}

/// Runs the staged funnel for a strategy. Owns concurrency, thresholding,
/// ranking, and report assembly; the strategy owns discovery and judgments.
pub async fn run_review<S: ReviewStrategy>(
    scopes: &[PathBuf],
    log: &dyn Fn(&str),
    options: ReviewOptions,
    strategy: S,
) -> Result<ReviewReport> {
    let ReviewOptions { max_follow_ups, refine, feedback } = options;
    let thresholds = feedback.thresholds();
    let suppressed = feedback.suppressed();
    for (dimension, threshold) in &thresholds {
        if (*threshold - SCREEN_THRESHOLD).abs() > f64::EPSILON {
            log(&format!(
                "Feedback tuned the {} threshold to {threshold:.2}",
                dimension.key()
            ));
        }
    }

    let scope_label = scopes
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(" ");

    let Discovery { files, context_files } = strategy.discover(scopes)?;
    if files.is_empty() {
        return Err(anyhow!(
            "No {} files found under {}",
            strategy.subject(),
            scope_label
        ));
    }

    log(&format!(
        "Screening {} {} files with {} {} files as context...",
        files.len(),
        strategy.subject(),
        context_files.len(),
        strategy.context_label()
    ));

    // 1. Screen: ordered, bounded concurrency (CONCURRENCY).
    let matrix: Vec<Screening<S::File>> = stream::iter(files)
        .map(|file| {
            let strategy = &strategy;
            let context = &context_files;
            async move {
                log(&format!("  screen {}", file.path()));
                strategy
                    .screen(&file, context)
                    .await
                    .map_err(|e| anyhow!("Screening {} failed: {e:#}", file.path()))
            }
        })
        .buffered(CONCURRENCY)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
    let screened_files = matrix.len();

    // 2. Rank: flatten to signals, keep those at/above the (feedback-tuned)
    //    per-dimension threshold, sort desc.
    let mut signals: Vec<Signal<S::File>> = Vec::new();
    for screening in &matrix {
        for (dimension, probability) in &screening.probabilities {
            let threshold = thresholds.get(dimension).copied().unwrap_or(SCREEN_THRESHOLD);
            if *probability >= threshold {
                signals.push(Signal {
                    file: screening.file.clone(),
                    dimension: *dimension,
                    probability: *probability,
                });
            }
        }
    }
    signals.sort_by(|a, b| b.probability.partial_cmp(&a.probability).unwrap_or(Ordering::Equal));
    let threshold_signals = signals.len();

    // 3. Profile: top MAX_PROFILES by max probability.
    let mut profile_candidates: Vec<&Screening<S::File>> = matrix.iter().collect();
    profile_candidates.sort_by(|a, b| {
        max_probability(b)
            .partial_cmp(&max_probability(a))
            .unwrap_or(Ordering::Equal)
    });
    profile_candidates.truncate(MAX_PROFILES);

    log(&format!("Profiling {} files...", profile_candidates.len()));
    let profiles: Vec<FileProfile> = stream::iter(profile_candidates)
        .map(|screening| {
            let strategy = &strategy;
            async move {
                log(&format!("  profile {}", screening.file.path()));
                strategy.profile(&screening.file, &screening.probabilities).await
            }
        })
        .buffered(CONCURRENCY)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
    let profiled_files = profiles.len();

    // 4. Locate: follow up every threshold signal (unlimited), or cap with a
    //    per-dimension budget when `max_follow_ups` is set.
    let follow_ups = select_follow_ups(&signals, max_follow_ups);
    let followed_signals = follow_ups.len();
    log(&format!(
        "Following {} of {} threshold signals...",
        followed_signals, threshold_signals
    ));
    let located: Vec<Option<Finding>> = stream::iter(follow_ups)
        .map(|signal| {
            let strategy = &strategy;
            async move {
                log(&format!(
                    "  inspect {} [{}={:.2}]",
                    signal.file.path(),
                    signal.dimension.key(),
                    signal.probability
                ));
                strategy.locate(&signal).await
            }
        })
        .buffered(CONCURRENCY)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>>>()?;

    let mut findings: Vec<Finding> = located.into_iter().flatten().collect();
    let located_findings = findings.len();

    // 5. Suppress: drop findings a reviewer asked to hide (by fingerprint).
    for finding in &mut findings {
        finding.fingerprint = fingerprint(finding);
    }
    findings.retain(|f| !suppressed.contains(&f.fingerprint));
    let suppressed_findings = located_findings - findings.len();

    // 6. Refine: dedupe, taint, counterfactual, ensemble, pairwise rank.
    let mut refine_counts = RefineCounts::default();
    if refine && !findings.is_empty() {
        let files: HashMap<&str, &S::File> =
            matrix.iter().map(|s| (s.file.path(), &s.file)).collect();
        let context = |finding: &Finding| {
            json!({
                "fileContext": files.get(finding.file.as_str()).map(|f| f.context_around(finding.line)),
                "neighbors": strategy.neighbor_context(&finding.file),
            })
        };
        let refiner = Refiner {
            client: strategy.client(),
            context: &context,
            log,
            concurrency: CONCURRENCY,
        };
        (findings, refine_counts) = refine::refine(&refiner, findings).await;
    } else {
        findings.sort_by(|a, b| b.severity.partial_cmp(&a.severity).unwrap_or(Ordering::Equal));
    }
    let routed_findings = findings.iter().filter(|f| f.owner.is_some()).count();

    // 7. Enrich: title/why for every finding (deterministic); fix/test via one
    //    narrow Jev call each, for at most MAX_ENRICH findings by severity.
    for finding in &mut findings {
        explain::apply_context(finding);
    }
    let enrich_cap = MAX_ENRICH.min(findings.len());
    if enrich_cap > 0 {
        let suggestions: Vec<(Option<String>, Option<String>)> = stream::iter(findings.iter().take(enrich_cap))
            .map(|finding| {
                let strategy = &strategy;
                async move {
                    match strategy.suggestions(finding).await {
                        Ok(suggestions) => suggestions,
                        Err(err) => {
                            log(&format!(
                                "  enrich {} failed (fix/test left empty): {err:#}",
                                finding.file
                            ));
                            (None, None)
                        }
                    }
                }
            })
            .buffered(CONCURRENCY)
            .collect::<Vec<_>>()
            .await;
        for (finding, (fix, test)) in findings.iter_mut().zip(suggestions) {
            finding.fix = fix;
            finding.test = test;
        }
    }

    let matrix_rows: Vec<MatrixRow> = matrix
        .iter()
        .map(|s| MatrixRow {
            file: s.file.path().to_string(),
            probabilities: s.probabilities.clone(),
        })
        .collect();
    let context_paths = context_files.iter().map(|f| f.path().to_string()).collect();

    let p_revert = merge_confidence::p_revert(&findings, &matrix_rows);

    Ok(ReviewReport {
        mode: strategy.mode(),
        scope: scope_label,
        dimensions: dimension_metadata(),
        config: ConfigSnapshot {
            screen_threshold: SCREEN_THRESHOLD,
            screen_thresholds: thresholds,
            severity_max: SEVERITY_MAX,
            max_follow_ups,
            max_profiles: MAX_PROFILES,
        },
        screened_files,
        context_files: context_paths,
        matrix: matrix_rows,
        followed_signals,
        profiles,
        workflow: WorkflowCounts {
            screened_cells: screened_files * DIMENSIONS.len(),
            threshold_signals,
            profiled_files,
            followed_signals,
            located_findings,
            routed_findings,
            suppressed_findings,
            clustered_findings: refine_counts.clustered,
            exonerated_findings: refine_counts.exonerated,
            needs_human_findings: refine_counts.needs_human,
        },
        findings,
        p_revert,
    })
}

fn max_probability<F: crate::review::strategy::FileEntry>(s: &Screening<F>) -> f64 {
    s.probabilities
        .values()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max)
}

/// Selects follow-up signals. Assumes `signals` is sorted by probability
/// descending. With `budget: None` (unlimited) it returns every signal; with
/// `Some(n)` each dimension with a signal gets a slot before global
/// probability fills the remainder, so a saturated cheap dimension (e.g.
/// `testGap`) cannot starve the others.
fn select_follow_ups<F: FileEntry>(signals: &[Signal<F>], budget: Option<usize>) -> Vec<Signal<F>> {
    let Some(budget) = budget else {
        return signals.to_vec();
    };

    use std::collections::HashSet;

    let mut out: Vec<Signal<F>> = Vec::new();
    let mut chosen: HashSet<usize> = HashSet::new();
    let mut dim_done: HashSet<Dimension> = HashSet::new();

    for (i, s) in signals.iter().enumerate() {
        if out.len() >= budget || dim_done.len() == DIMENSIONS.len() {
            break;
        }
        if dim_done.insert(s.dimension) {
            chosen.insert(i);
            out.push(s.clone());
        }
    }
    for (i, s) in signals.iter().enumerate() {
        if out.len() >= budget {
            break;
        }
        if chosen.insert(i) {
            out.push(s.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::policy::Dimension;
    use crate::domain::report::ChangedFile;

    fn sig(dim: Dimension, p: f64) -> Signal<ChangedFile> {
        Signal {
            file: ChangedFile { path: format!("{dim:?}"), patch: String::new(), base: String::new() },
            dimension: dim,
            probability: p,
        }
    }

    #[test]
    fn follow_up_budget_spans_dimensions() {
        // testGap saturates at high probability, but every signaled dimension
        // must still get a follow-up slot.
        let signals = vec![
            sig(Dimension::TestGap, 0.99),
            sig(Dimension::TestGap, 0.98),
            sig(Dimension::TestGap, 0.97),
            sig(Dimension::Security, 0.95),
            sig(Dimension::Correctness, 0.90),
            sig(Dimension::Reliability, 0.88),
            sig(Dimension::Compatibility, 0.80),
            sig(Dimension::TestGap, 0.96),
        ];
        let selected = select_follow_ups(&signals, Some(8));
        assert_eq!(selected.len(), 8);
        let dims: std::collections::HashSet<Dimension> =
            selected.iter().map(|s| s.dimension).collect();
        for d in DIMENSIONS {
            assert!(dims.contains(&d), "dimension {d:?} missing from follow-ups");
        }
    }

    #[test]
    fn follow_up_budget_respects_cap() {
        let signals = vec![
            sig(Dimension::TestGap, 0.99),
            sig(Dimension::Security, 0.98),
        ];
        assert_eq!(select_follow_ups(&signals, Some(1)).len(), 1);
    }

    #[test]
    fn unrestricted_follows_every_signal() {
        let signals = vec![
            sig(Dimension::TestGap, 0.99),
            sig(Dimension::TestGap, 0.98),
            sig(Dimension::Security, 0.95),
        ];
        assert_eq!(select_follow_ups(&signals, None).len(), 3);
    }
}