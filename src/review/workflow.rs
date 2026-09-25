//! Shared staged orchestration. Each review mode owns discovery and judgments;
//! this module owns concurrency, thresholds, ranking, and report assembly.
//! Mirrors `review/workflow.ts`.

use std::cmp::Ordering;
use std::path::Path;

use anyhow::{Result, anyhow};
use futures::{StreamExt, stream};

use crate::domain::policy::{
    CONCURRENCY, DIMENSIONS, MAX_FOLLOW_UPS, MAX_PROFILES, SCREEN_THRESHOLD, SEVERITY_MAX,
    dimension_metadata,
};
use crate::domain::report::{
    ConfigSnapshot, FileProfile, Finding, MatrixRow, ReviewReport, WorkflowCounts,
};
use crate::review::strategy::{Discovery, FileEntry, ReviewStrategy, Screening, Signal};

/// Runs the staged funnel for a strategy. Owns concurrency, thresholding,
/// ranking, and report assembly; the strategy owns discovery and judgments.
pub async fn run_review<S: ReviewStrategy>(
    scope: &Path,
    log: &dyn Fn(&str),
    strategy: S,
) -> Result<ReviewReport> {
    let Discovery { files, context_files } = strategy.discover(scope)?;
    if files.is_empty() {
        return Err(anyhow!(
            "No {} files found under {}",
            strategy.subject(),
            scope.display()
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

    // 2. Rank: flatten to signals, keep those at/above threshold, sort desc.
    let mut signals: Vec<Signal<S::File>> = Vec::new();
    for screening in &matrix {
        for (dimension, probability) in &screening.probabilities {
            if *probability >= SCREEN_THRESHOLD {
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

    // 4. Locate: follow up to MAX_FOLLOW_UPS signals.
    let follow_ups: Vec<Signal<S::File>> = signals.into_iter().take(MAX_FOLLOW_UPS).collect();
    let followed_signals = follow_ups.len();
    log(&format!(
        "Following {} of {} signals at or above {}...",
        followed_signals, threshold_signals, SCREEN_THRESHOLD
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
    findings.sort_by(|a, b| b.severity.partial_cmp(&a.severity).unwrap_or(Ordering::Equal));
    let located_findings = findings.len();
    let routed_findings = findings.iter().filter(|f| f.owner.is_some()).count();

    let matrix_rows = matrix
        .iter()
        .map(|s| MatrixRow {
            file: s.file.path().to_string(),
            probabilities: s.probabilities.clone(),
        })
        .collect();
    let context_paths = context_files.iter().map(|f| f.path().to_string()).collect();

    Ok(ReviewReport {
        mode: strategy.mode(),
        scope: scope.display().to_string(),
        dimensions: dimension_metadata(),
        config: ConfigSnapshot {
            screen_threshold: SCREEN_THRESHOLD,
            severity_max: SEVERITY_MAX,
            max_follow_ups: MAX_FOLLOW_UPS,
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
        },
        findings,
    })
}

fn max_probability<F: crate::review::strategy::FileEntry>(s: &Screening<F>) -> f64 {
    s.probabilities
        .values()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max)
}