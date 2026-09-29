//! Shared staged orchestration. Each review mode owns discovery and judgments;
//! this module owns concurrency, thresholds, ranking, and report assembly.

use std::cmp::Ordering;
use std::path::PathBuf;

use anyhow::{Result, anyhow};
use futures::{Stream, StreamExt, stream};
use std::future::Future;

use std::collections::HashMap;

use serde_json::json;

use crate::domain::feedback::{FeedbackLog, fingerprint};
use crate::domain::policy::{
    CONCURRENCY, DIMENSIONS, MAX_PROFILES, SCREEN_THRESHOLD, SEVERITY_MAX, Dimension,
    dimension_metadata,
};
use crate::review::context::ContextDrops;
use crate::domain::report::{
    ConfigSnapshot, FileProfile, Finding, MatrixRow, ReviewReport, ReviewStage, SkippedFile,
    WorkflowCounts,
};
use crate::review::explain::{self, MAX_ENRICH};
use crate::review::merge_confidence;
use crate::review::refine::{self, RefineCounts, Refiner};
use crate::review::strategy::{Discovery, FileEntry, ReviewStrategy, Screening, Signal};
use crate::review::typesafe::parse_positive;

/// Parallel System One requests: `MOMUS_CONCURRENCY` if set, else the policy
/// default. A local server usually wants 1; the hosted API handles more.
fn concurrency_from_env() -> Result<usize> {
    match std::env::var("MOMUS_CONCURRENCY") {
        Ok(raw) if !raw.trim().is_empty() => parse_positive("MOMUS_CONCURRENCY", &raw),
        _ => Ok(CONCURRENCY),
    }
}

/// A stage aborts when its first this-many results all failed: the failure
/// is systemic (a bad key, an unreachable server, requests that can never
/// fit), so sending the rest would only repeat it.
const CIRCUIT_BREAKER: usize = 5;

/// A skipped request's reason is shortened to this many characters.
const MAX_SKIP_REASON_CHARS: usize = 300;

/// Drains a stage's `(file, result)` stream in order, keeping successes and
/// recording each failure in `skipped` so one bad file cannot end the run.
/// With `breaker` on, when the first `CIRCUIT_BREAKER` results all failed,
/// stops at once (dropping the stream cancels the requests still in flight)
/// and returns the error; every failure, the last one too, is in `skipped`.
async fn tolerate<O>(
    results: impl Stream<Item = (String, Result<O>)>,
    stage: ReviewStage,
    breaker: bool,
    log: &dyn Fn(&str),
    skipped: &mut Vec<SkippedFile>,
) -> Result<Vec<O>> {
    let mut results = std::pin::pin!(results);
    let mut kept = Vec::new();
    let mut failures = 0usize;
    while let Some((file, result)) = results.next().await {
        match result {
            Ok(value) => kept.push(value),
            Err(error) => {
                failures += 1;
                let reason: String = format!("{error:#}").chars().take(MAX_SKIP_REASON_CHARS).collect();
                skipped.push(SkippedFile { file: file.clone(), stage, reason: reason.clone() });
                if breaker && kept.is_empty() && failures >= CIRCUIT_BREAKER {
                    return Err(error.context(format!(
                        "the first {CIRCUIT_BREAKER} {} requests all failed; stopping instead of sending the rest",
                        stage.key()
                    )));
                }
                log(&format!("  {} {file} failed, skipped: {reason}", stage.key()));
            }
        }
    }
    Ok(kept)
}

/// Runs a stage's requests in order with failure isolation (`tolerate`).
/// The first `CIRCUIT_BREAKER` requests go out at most that many at a time;
/// the rest start at full `concurrency` only once those include a success,
/// so a systemic failure never sends more than `CIRCUIT_BREAKER` requests
/// whatever `MOMUS_CONCURRENCY` is.
async fn run_stage<T, O, F, Fut>(
    items: Vec<T>,
    concurrency: usize,
    stage: ReviewStage,
    log: &dyn Fn(&str),
    skipped: &mut Vec<SkippedFile>,
    request: F,
) -> Result<Vec<O>>
where
    F: Fn(T) -> Fut,
    Fut: Future<Output = (String, Result<O>)>,
{
    let mut rest = items;
    let probe: Vec<T> = rest.drain(..rest.len().min(CIRCUIT_BREAKER)).collect();
    let probing = stream::iter(probe).map(&request).buffered(concurrency.min(CIRCUIT_BREAKER));
    let mut kept = tolerate(probing, stage, true, log, skipped).await?;
    if !rest.is_empty() {
        // The probe had a success (or the breaker would have tripped), so
        // later failures are the files', not the service's.
        let remaining = stream::iter(rest).map(&request).buffered(concurrency);
        kept.extend(tolerate(remaining, stage, false, log, skipped).await?);
    }
    Ok(kept)
}

/// Per-run knobs beyond the scope and strategy. The default matches the CLI:
/// unlimited follow-ups, refinement on, no feedback.
#[derive(Debug)]
pub struct ReviewOptions {
    /// Cap follow-ups per `select_follow_ups`; `None` follows every signal.
    pub max_follow_ups: Option<usize>,
    /// Run the post-locate refinement stages (`review::refine`).
    pub refine: bool,
    /// Reviewer feedback: tunes per-dimension thresholds and suppresses
    /// findings (`domain::feedback`).
    pub feedback: FeedbackLog,
    /// No files to review is an empty report, not an error (`--allow-empty`):
    /// in CI, a docs- or config-only pull request has nothing to screen.
    pub allow_empty: bool,
}

impl Default for ReviewOptions {
    fn default() -> Self {
        Self { max_follow_ups: None, refine: true, feedback: FeedbackLog::default(), allow_empty: false }
    }
}

/// Runs the staged funnel for a strategy. Owns concurrency, thresholding,
/// ranking, and report assembly; the strategy owns discovery and judgments.
pub async fn run_review<S: ReviewStrategy>(
    scopes: &[PathBuf],
    log: &dyn Fn(&str),
    options: ReviewOptions,
    strategy: S,
) -> Result<ReviewReport> {
    let ReviewOptions { max_follow_ups, refine, feedback, allow_empty } = options;
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

    let concurrency = concurrency_from_env()?;
    let Discovery { files, context_files } = strategy.discover(scopes)?;
    if files.is_empty() && allow_empty {
        log(&format!("No {} files to review under {scope_label}", strategy.subject()));
        return Ok(ReviewReport {
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
            context_files: context_files.iter().map(|f| f.path().to_string()).collect(),
            ..Default::default()
        });
    }
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

    // 1. Screen: ordered, bounded concurrency (CONCURRENCY, or MOMUS_CONCURRENCY).
    //    A file whose screen fails is skipped and recorded; a review needs at
    //    least one screened file.
    let mut skipped: Vec<SkippedFile> = Vec::new();
    let matrix: Vec<Screening<S::File>> =
        run_stage(files, concurrency, ReviewStage::Screen, log, &mut skipped, |file| {
            let strategy = &strategy;
            let context = &context_files;
            async move {
                log(&format!("  screen {}", file.path()));
                (file.path().to_string(), strategy.screen(&file, context).await)
            }
        })
        .await
        .map_err(|e| anyhow!("Screening failed: {e:#}"))?;
    if matrix.is_empty() {
        let first = skipped.first().map(|s| s.reason.as_str()).unwrap_or("");
        return Err(anyhow!("Screening failed for every file: {first}"));
    }
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
    // Profiles are a triage aid and never gate findings: even the breaker
    // tripping here only costs the profiles, not the review.
    let profiles: Vec<FileProfile> =
        match run_stage(profile_candidates, concurrency, ReviewStage::Profile, log, &mut skipped, |screening| {
            let strategy = &strategy;
            async move {
                log(&format!("  profile {}", screening.file.path()));
                let path = screening.file.path().to_string();
                (path, strategy.profile(&screening.file, &screening.probabilities).await)
            }
        })
        .await
        {
            Ok(profiles) => profiles,
            Err(error) => {
                log(&format!("Profiling stopped, continuing without profiles: {error:#}"));
                Vec::new()
            }
        };
    let profiled_files = profiles.len();

    // 4. Locate: follow up every threshold signal (unlimited), or cap with a
    //    per-dimension budget when `max_follow_ups` is set.
    let follow_ups = select_follow_ups(&signals, max_follow_ups);
    let followed_signals = follow_ups.len();
    log(&format!(
        "Following {} of {} threshold signals...",
        followed_signals, threshold_signals
    ));
    let located: Vec<Option<Finding>> =
        run_stage(follow_ups, concurrency, ReviewStage::Locate, log, &mut skipped, |signal| {
            let strategy = &strategy;
            async move {
                log(&format!(
                    "  inspect {} [{}={:.2}]",
                    signal.file.path(),
                    signal.dimension.key(),
                    signal.probability
                ));
                let label = format!("{} [{}]", signal.file.path(), signal.dimension.key());
                (label, strategy.locate(&signal).await)
            }
        })
        .await?;

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
            concurrency,
        };
        (findings, refine_counts) = refine::refine(&refiner, findings).await;
    } else {
        findings.sort_by(|a, b| b.severity.partial_cmp(&a.severity).unwrap_or(Ordering::Equal));
    }
    let routed_findings = findings.iter().filter(|f| f.owner.is_some()).count();

    // 7. Enrich: title/why for every finding (deterministic); fix/test via one
    //    narrow Jev call each, for the first MAX_ENRICH findings in report
    //    order: by severity, with refinement's pairwise-ranked top-K (K <=
    //    MAX_ENRICH) re-ordered within it, so the same findings are chosen.
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
            .buffered(concurrency)
            .collect::<Vec<_>>()
            .await;
        for (finding, (fix, test)) in findings.iter_mut().zip(suggestions) {
            finding.fix = fix;
            finding.test = test;
        }
    }

    let context_drops = matrix
        .iter()
        .fold(ContextDrops::default(), |acc, screening| acc + screening.dropped);

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
            dropped_context_chars: context_drops.chars,
            dropped_context_items: context_drops.items,
        },
        usage: strategy.client().usage_summary(),
        redactions: strategy.client().redaction_summary(),
        skipped,
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

    /// `(file, result)` items for `tolerate`: `true` succeeds with the index.
    fn results(outcomes: &[bool]) -> Vec<(String, Result<usize>)> {
        outcomes
            .iter()
            .enumerate()
            .map(|(i, ok)| {
                let file = format!("f{i}.rs");
                let result = if *ok { Ok(i) } else { Err(anyhow!("400 max_tokens_exceeded")) };
                (file, result)
            })
            .collect()
    }

    #[tokio::test]
    async fn one_failure_is_skipped_not_fatal() {
        let mut skipped = Vec::new();
        let kept = tolerate(stream::iter(results(&[true, false, true])), ReviewStage::Screen, true, &|_| {}, &mut skipped)
            .await
            .unwrap();
        assert_eq!(kept, vec![0, 2]);
        assert_eq!(
            skipped,
            vec![SkippedFile {
                file: "f1.rs".into(),
                stage: ReviewStage::Screen,
                reason: "400 max_tokens_exceeded".into(),
            }]
        );
    }

    #[tokio::test]
    async fn a_failing_start_trips_the_breaker_without_draining_the_stream() {
        let pulled = std::sync::atomic::AtomicUsize::new(0);
        let items = stream::iter(results(&[false; 300])).inspect(|_| {
            pulled.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        let mut skipped = Vec::new();
        let err = tolerate(items, ReviewStage::Screen, true, &|_| {}, &mut skipped).await.unwrap_err();
        assert!(format!("{err:#}").contains("first 5 screen requests all failed"), "{err:#}");
        assert!(format!("{err:#}").contains("max_tokens_exceeded"), "{err:#}");
        assert_eq!(pulled.load(std::sync::atomic::Ordering::SeqCst), CIRCUIT_BREAKER, "stops at the breaker");
    }

    #[tokio::test]
    async fn failures_after_a_success_never_trip_the_breaker() {
        let mut outcomes = vec![true];
        outcomes.extend([false; 10]);
        let mut skipped = Vec::new();
        let kept = tolerate(stream::iter(results(&outcomes)), ReviewStage::Locate, true, &|_| {}, &mut skipped)
            .await
            .unwrap();
        assert_eq!(kept, vec![0]);
        assert_eq!(skipped.len(), 10);
        assert!(skipped.iter().all(|s| s.stage == ReviewStage::Locate));
    }

    /// `run_stage` over `outcomes` at `concurrency`: (kept, skipped count,
    /// requests started, error).
    async fn stage(outcomes: &[bool], concurrency: usize) -> (Vec<usize>, usize, usize, Option<String>) {
        let started = std::sync::atomic::AtomicUsize::new(0);
        let items: Vec<(usize, bool)> = outcomes.iter().copied().enumerate().collect();
        let mut skipped = Vec::new();
        let result = run_stage(items, concurrency, ReviewStage::Screen, &|_| {}, &mut skipped, |(i, ok)| {
            started.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async move { (format!("f{i}.rs"), if ok { Ok(i) } else { Err(anyhow!("boom")) }) }
        })
        .await;
        let started = started.load(std::sync::atomic::Ordering::SeqCst);
        match result {
            Ok(kept) => (kept, skipped.len(), started, None),
            Err(e) => (Vec::new(), skipped.len(), started, Some(format!("{e:#}"))),
        }
    }

    #[tokio::test]
    async fn a_systemic_failure_starts_only_the_probe_at_any_concurrency() {
        let (_, skipped, started, err) = stage(&[false; 300], 10).await;
        assert_eq!(started, CIRCUIT_BREAKER, "at MOMUS_CONCURRENCY=10, only the probe went out");
        assert_eq!(skipped, CIRCUIT_BREAKER, "every probe failure is recorded");
        assert!(err.unwrap().contains("first 5 screen requests all failed"));
    }

    #[tokio::test]
    async fn a_probe_success_opens_full_concurrency_for_the_rest() {
        let mut outcomes = vec![false; 300];
        outcomes[2] = true;
        let (kept, skipped, started, err) = stage(&outcomes, 10).await;
        assert!(err.is_none(), "{err:?}");
        assert_eq!((kept, skipped, started), (vec![2], 299, 300));
    }

    #[tokio::test]
    async fn fewer_items_than_the_probe_that_all_fail_are_skipped_not_fatal() {
        let (kept, skipped, _, err) = stage(&[false, false, false], 10).await;
        assert!(err.is_none() && kept.is_empty());
        assert_eq!(skipped, 3);
    }

    #[tokio::test]
    async fn long_reasons_are_shortened() {
        let items = stream::iter(vec![("f.rs".to_string(), Err::<usize, _>(anyhow!("x".repeat(1000))))]);
        let mut skipped = Vec::new();
        tolerate(items, ReviewStage::Profile, true, &|_| {}, &mut skipped).await.unwrap();
        assert_eq!(skipped[0].reason.chars().count(), MAX_SKIP_REASON_CHARS);
    }

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
    fn default_options_refine_like_the_cli() {
        let options = ReviewOptions::default();
        assert!(options.refine);
        assert_eq!(options.max_follow_ups, None);
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