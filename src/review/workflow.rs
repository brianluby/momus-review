//! Shared staged orchestration. Each review mode owns discovery and judgments;
//! this module owns concurrency, thresholds, ranking, and report assembly.

use std::cmp::Ordering;
use std::path::PathBuf;

use anyhow::{Result, anyhow};
use futures::{Stream, StreamExt, stream};
use std::future::Future;

use std::collections::{BTreeMap, HashMap};

use serde_json::json;

use crate::domain::feedback::{FeedbackLog, fingerprint};
use crate::domain::policy::{
    CONCURRENCY, DIMENSIONS, Dimension, MAX_PROFILES, SCREEN_THRESHOLD, SEVERITY_MAX,
    dimension_metadata,
};
use crate::domain::report::{
    ConfigSnapshot, FileProfile, Finding, MatrixRow, ReviewReport, ReviewStage, SkippedFile,
    WorkflowCounts,
};
use crate::review::context::ContextDrops;
use crate::review::explain::{self, MAX_ENRICH};
use crate::review::merge_confidence;
use crate::review::refine::{self, RefineCounts, Refiner};
use crate::review::strategy::{Discovery, FileEntry, ReviewStrategy, Screening, Signal};
use crate::review::typesafe::parse_positive;
use crate::review::voi::{self, FollowUpStrategy};

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
                let deferred = error.is::<crate::review::budget::BudgetExhausted>();
                if !deferred {
                    failures += 1;
                }
                let reason: String = format!("{error:#}")
                    .chars()
                    .take(MAX_SKIP_REASON_CHARS)
                    .collect();
                skipped.push(SkippedFile {
                    file: file.clone(),
                    stage,
                    reason: reason.clone(),
                });
                if breaker && kept.is_empty() && failures >= CIRCUIT_BREAKER {
                    return Err(error.context(format!(
                        "the first {CIRCUIT_BREAKER} {} requests all failed; stopping instead of sending the rest",
                        stage.key()
                    )));
                }
                log(&format!(
                    "  {} {file} failed, skipped: {reason}",
                    stage.key()
                ));
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
    let probing = stream::iter(probe)
        .map(&request)
        .buffered(concurrency.min(CIRCUIT_BREAKER));
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
    /// Opt-in information-value ordering; default probability policy stays unchanged.
    pub follow_up_strategy: FollowUpStrategy,
    /// Explicit eligibility thresholds applied after saved reviewer feedback.
    pub threshold_overrides: BTreeMap<Dimension, f64>,
    /// Attach unfinished scaffolds for supported test-gap findings; never execute them.
    pub test_plans: bool,
    /// Explicit bounded requirement inputs for the opt-in spec check.
    pub specs: Option<crate::review::spec_drift::SpecInputs>,
    /// Run the post-locate refinement stages (`review::refine`).
    pub refine: bool,
    /// Reviewer feedback: tunes per-dimension thresholds and suppresses
    /// findings (`domain::feedback`).
    pub feedback: FeedbackLog,
    /// No files to review is an empty report, not an error (`--allow-empty`):
    /// in CI, a docs- or config-only pull request has nothing to screen.
    pub allow_empty: bool,
    pub tiered: bool,
    pub shard: Option<crate::review::planner::Shard>,
}

impl Default for ReviewOptions {
    /// Enable ordinary refinement and unlimited follow-ups, without tiering or sharding.
    fn default() -> Self {
        Self {
            max_follow_ups: None,
            follow_up_strategy: FollowUpStrategy::default(),
            threshold_overrides: BTreeMap::new(),
            test_plans: false,
            specs: None,
            refine: true,
            feedback: FeedbackLog::default(),
            allow_empty: false,
            tiered: false,
            shard: None,
        }
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
    let ReviewOptions {
        max_follow_ups,
        follow_up_strategy,
        threshold_overrides,
        test_plans,
        specs,
        refine,
        feedback,
        allow_empty,
        tiered,
        shard,
    } = options;
    if shard.is_some()
        && (test_plans || specs.is_some() || follow_up_strategy == FollowUpStrategy::Voi)
    {
        return Err(anyhow!(
            "test plans, spec checks and VOI require a global, unsharded review"
        ));
    }
    if tiered && specs.is_some() {
        return Err(anyhow!("spec checks cannot omit Tier-0 dismissed files"));
    }
    let mut thresholds = feedback.thresholds();
    for (dimension, threshold) in threshold_overrides {
        if !threshold.is_finite() || !(0.0..=1.0).contains(&threshold) {
            return Err(anyhow!(
                "{} threshold must be finite and from 0 to 1",
                dimension.key()
            ));
        }
        thresholds.insert(dimension, threshold);
    }
    let suppressed = feedback.suppressed();
    for (dimension, threshold) in &thresholds {
        if (*threshold - SCREEN_THRESHOLD).abs() > f64::EPSILON {
            log(&format!(
                "Using the {} threshold {threshold:.2}",
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
    let discovery = strategy.discover(scopes)?;
    strategy.prepass(scopes, &discovery)?;
    let Discovery {
        mut files,
        context_files,
    } = discovery;
    let shard_metadata = shard.map(|shard| crate::domain::report::ShardMetadata {
        index: shard.index,
        count: shard.count,
        inventory_key: crate::review::planner::inventory_key(&files, &context_files),
        expected_paths: files.iter().map(|f| f.path().to_string()).collect(),
        refine,
        model: strategy.client().model_identity(),
    });
    if let Some(shard) = shard {
        files.retain(|f| shard.contains(f.path()));
    }
    if strategy.client().budget_summary().limit.is_some() || tiered {
        crate::review::planner::prioritize(&mut files, scopes);
    }
    // Retain the requested spec inventory independently of successful risk screens.
    let spec_files = specs.as_ref().map(|_| files.clone());
    if files.is_empty() && (allow_empty || shard.is_some()) {
        log(&format!(
            "No {} files to review under {scope_label}",
            strategy.subject()
        ));
        return Ok(ReviewReport {
            mode: strategy.mode(),
            scope: scope_label,
            dimensions: dimension_metadata(),
            config: ConfigSnapshot {
                screen_threshold: SCREEN_THRESHOLD,
                screen_thresholds: thresholds,
                severity_max: SEVERITY_MAX,
                max_follow_ups,
                follow_up_strategy,
                max_profiles: MAX_PROFILES,
            },
            shard: shard_metadata,
            partial: shard.is_some(),
            spec_drift: specs
                .as_ref()
                .map(|s| crate::review::spec_drift::SpecSummary {
                    documents: s.documents(),
                    checks: Vec::new(),
                }),
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

    let mut tier = crate::domain::report::TierSummary::default();
    if tiered {
        let question = json!({ "concern": crate::review::typesafe::noul(
            json!("Does this file contain any correctness, security, reliability, compatibility, or test-coverage concern worth a detailed review? Inspect all behavior; favor review when uncertain."),
            json!({"true": "Any plausible concern needing detailed review", "false": "No concern warrants review"})) });
        let answers: Vec<_> = stream::iter(files)
            .map(|file| {
                let strategy = &strategy;
                let question = question.clone();
                async move {
                    let Some(state) = file.tier_state() else {
                        return (file, None);
                    };
                    let answer = strategy
                        .client()
                        .system_one(state, question)
                        .await
                        .and_then(|a| a.noul("concern"));
                    (file, Some(answer))
                }
            })
            .buffered(concurrency)
            .collect()
            .await;
        let mut retained = Vec::new();
        for (file, answer) in answers {
            match answer {
                None => retained.push(file),
                Some(answer) => {
                    tier.screened += 1;
                    match answer {
                        Ok(p) if p < 0.05 => tier.dismissed.push(file.path().to_string()),
                        _ => retained.push(file), // uncertainty/errors always retain full screening
                    }
                }
            }
        }
        files = retained;
    }
    if files.is_empty() {
        return Ok(ReviewReport {
            mode: strategy.mode(),
            scope: scope_label,
            tier,
            shard: shard_metadata.map(|mut meta| {
                meta.model = strategy.client().model_identity();
                meta
            }),
            usage: strategy.client().usage_summary(),
            budget: strategy.client().budget_summary(),
            dimensions: dimension_metadata(),
            config: ConfigSnapshot {
                screen_threshold: SCREEN_THRESHOLD,
                screen_thresholds: thresholds,
                severity_max: SEVERITY_MAX,
                max_follow_ups,
                follow_up_strategy,
                max_profiles: MAX_PROFILES,
            },
            index: strategy.index_stats(),
            redactions: strategy.client().redaction_summary(),
            context_files: context_files.iter().map(|f| f.path().to_string()).collect(),
            partial: true,
            ..Default::default()
        });
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
    let matrix: Vec<Screening<S::File>> = run_stage(
        files,
        concurrency,
        ReviewStage::Screen,
        log,
        &mut skipped,
        |file| {
            let strategy = &strategy;
            let context = &context_files;
            async move {
                log(&format!("  screen {}", file.path()));
                (
                    file.path().to_string(),
                    strategy.screen(&file, context).await,
                )
            }
        },
    )
    .await
    .map_err(|e| anyhow!("Screening failed: {e:#}"))?;
    if matrix.is_empty() && strategy.client().budget_summary().deferred == 0 {
        let first = skipped.first().map(|s| s.reason.as_str()).unwrap_or("");
        return Err(anyhow!("Screening failed for every file: {first}"));
    }
    let screened_files = matrix.len();

    // 2. Rank: flatten to signals, keep those at/above the (feedback-tuned)
    //    per-dimension threshold, sort desc.
    let mut signals: Vec<Signal<S::File>> = Vec::new();
    for screening in &matrix {
        for (dimension, probability) in &screening.probabilities {
            let threshold = thresholds
                .get(dimension)
                .copied()
                .unwrap_or(SCREEN_THRESHOLD);
            if *probability >= threshold {
                signals.push(Signal {
                    file: screening.file.clone(),
                    dimension: *dimension,
                    probability: *probability,
                });
            }
        }
    }
    signals.sort_by(|a, b| {
        b.probability
            .partial_cmp(&a.probability)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.file.path().cmp(b.file.path()))
            .then_with(|| a.dimension.key().cmp(b.dimension.key()))
    });
    let threshold_signals = signals.len();

    // 3. Profiles are optional metadata. Under a finite HTTP ceiling, VOI
    //    spends available attempts on concrete evidence first.
    let skip_optional_profiles = follow_up_strategy == FollowUpStrategy::Voi
        && strategy.client().budget_summary().limit.is_some();
    //    Otherwise retain the existing top MAX_PROFILES by max probability.
    let mut profile_candidates: Vec<&Screening<S::File>> = matrix.iter().collect();
    profile_candidates.sort_by(|a, b| {
        max_probability(b)
            .partial_cmp(&max_probability(a))
            .unwrap_or(Ordering::Equal)
    });
    profile_candidates.truncate(MAX_PROFILES);
    if skip_optional_profiles {
        profile_candidates.clear();
        log("VOI: skipping optional profiles to prioritize evidence under the call budget");
    }

    log(&format!("Profiling {} files...", profile_candidates.len()));
    // Profiles are a triage aid and never gate findings: even the breaker
    // tripping here only costs the profiles, not the review.
    let profiles: Vec<FileProfile> = match run_stage(
        profile_candidates,
        concurrency,
        ReviewStage::Profile,
        log,
        &mut skipped,
        |screening| {
            let strategy = &strategy;
            async move {
                log(&format!("  profile {}", screening.file.path()));
                let path = screening.file.path().to_string();
                (
                    path,
                    strategy
                        .profile(&screening.file, &screening.probabilities)
                        .await,
                )
            }
        },
    )
    .await
    {
        Ok(profiles) => profiles,
        Err(error) => {
            log(&format!(
                "Profiling stopped, continuing without profiles: {error:#}"
            ));
            Vec::new()
        }
    };
    let profiled_files = profiles.len();

    // 4. Locate: follow up every threshold signal (unlimited), or cap with a
    //    per-dimension budget when `max_follow_ups` is set.
    let (follow_ups, follow_up_plan) = match follow_up_strategy {
        FollowUpStrategy::Probability => (select_follow_ups(&signals, max_follow_ups), None),
        FollowUpStrategy::Voi => {
            let (selected, plan) = voi::select(
                &signals,
                max_follow_ups,
                &thresholds,
                skip_optional_profiles,
            );
            (selected, Some(plan))
        }
    };
    let followed_signals = follow_ups.len();
    log(&format!(
        "Following {} of {} threshold signals...",
        followed_signals, threshold_signals
    ));
    // Sequential evidence pipelines prevent concurrent partially completed
    // signals from spending the final finite attempts ahead of the top plan.
    // Cached pipelines are still tried after exhaustion, including calls=0.
    let locate_concurrency = if skip_optional_profiles {
        1
    } else {
        concurrency
    };
    let located: Vec<Option<Finding>> = run_stage(
        follow_ups,
        locate_concurrency,
        ReviewStage::Locate,
        log,
        &mut skipped,
        |signal| {
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
        },
    )
    .await?;

    let mut findings: Vec<Finding> = located.into_iter().flatten().collect();
    let located_findings = findings.len();

    // 5. Suppress: drop findings a reviewer asked to hide (by fingerprint).
    for finding in &mut findings {
        finding.fingerprint = fingerprint(finding);
    }
    findings.retain(|f| !suppressed.contains(&f.fingerprint));
    let suppressed_findings = located_findings - findings.len();

    // Explicit requirement checks precede optional refinement/enrichment/scaffolds.
    let spec_drift = match (specs.as_ref(), spec_files) {
        (Some(specs), Some(files)) => {
            let mut checks = Vec::with_capacity(files.len());
            for file in files {
                log(&format!("  spec {}", file.path()));
                let check = match crate::review::spec_drift::assess_file(
                    strategy.client(),
                    specs,
                    serde_json::to_value(&file)?,
                )
                .await
                {
                    Ok(check) => check,
                    Err(error) => crate::review::spec_drift::SpecCheck::deferred(
                        file.path(),
                        format!("{error:#}")
                            .chars()
                            .take(MAX_SKIP_REASON_CHARS)
                            .collect::<String>(),
                    ),
                };
                checks.push(check);
            }
            Some(crate::review::spec_drift::SpecSummary {
                documents: specs.documents(),
                checks,
            })
        }
        _ => None,
    };
    let spec_incomplete = spec_drift.as_ref().is_some_and(|summary| {
        summary.checks.iter().any(|check| {
            matches!(
                check.status,
                crate::review::spec_drift::SpecCheckStatus::Deferred
                    | crate::review::spec_drift::SpecCheckStatus::Uncertain
            )
        })
    });

    let files: HashMap<&str, &S::File> = matrix.iter().map(|s| (s.file.path(), &s.file)).collect();
    let (mut findings, refine_counts) = if shard.is_none() {
        finish_findings(&strategy, &files, findings, refine, log, concurrency).await
    } else {
        (findings, RefineCounts::default())
    };
    if test_plans {
        attach_test_plans(
            &strategy,
            &files,
            &context_files,
            &mut findings,
            log,
            &mut skipped,
        )
        .await;
    }
    let routed_findings = findings.iter().filter(|f| f.owner.is_some()).count();

    let context_drops = matrix
        .iter()
        .fold(ContextDrops::default(), |acc, screening| {
            acc + screening.dropped
        })
        + findings
            .iter()
            .filter_map(|f| f.test_plan.as_ref())
            .fold(ContextDrops::default(), |acc, plan| {
                acc + plan.context_drops
            });

    let matrix_rows: Vec<MatrixRow> = matrix
        .iter()
        .map(|s| MatrixRow {
            file: s.file.path().to_string(),
            probabilities: s.probabilities.clone(),
        })
        .collect();
    let context_paths = context_files.iter().map(|f| f.path().to_string()).collect();

    let p_revert = if shard.is_some() {
        0.0
    } else {
        merge_confidence::p_revert(&findings, &matrix_rows)
    };

    Ok(ReviewReport {
        mode: strategy.mode(),
        scope: scope_label,
        dimensions: dimension_metadata(),
        config: ConfigSnapshot {
            screen_threshold: SCREEN_THRESHOLD,
            screen_thresholds: thresholds,
            severity_max: SEVERITY_MAX,
            max_follow_ups,
            follow_up_strategy,
            max_profiles: MAX_PROFILES,
        },
        budget: strategy.client().budget_summary(),
        follow_up_plan,
        spec_drift,
        shard: shard_metadata.map(|mut meta| {
            meta.model = strategy.client().model_identity();
            meta
        }),
        partial: shard.is_some()
            || spec_incomplete
            || followed_signals < threshold_signals
            || strategy.client().budget_summary().deferred > 0
            || !skipped.is_empty()
            || !tier.dismissed.is_empty(),
        tier,
        wall_time_ms: 0,
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
        index: strategy.index_stats(),
        redactions: strategy.client().redaction_summary(),
        skipped,
        findings,
        p_revert,
    })
}

/// Attach at most the global planning cap, preserving findings when a plan fails.
async fn attach_test_plans<S: ReviewStrategy>(
    strategy: &S,
    files: &HashMap<&str, &S::File>,
    tests: &[S::File],
    findings: &mut [Finding],
    log: &dyn Fn(&str),
    skipped: &mut Vec<SkippedFile>,
) {
    for finding in findings
        .iter_mut()
        .filter(|f| crate::review::test_planner::should_plan(f))
        .take(crate::review::test_planner::MAX_TEST_PLANS)
    {
        let file = files.get(finding.file.as_str());
        let context = json!({
            "fileContext": file.map(|file| file.context_around(finding.line)),
            "relatedTests": file.map(|file| file.test_context(tests)),
            "neighbors": strategy.neighbor_context(&finding.file),
        });
        match crate::review::test_planner::plan(strategy.client(), finding, context).await {
            Ok(plan) => finding.test_plan = plan,
            Err(error) => {
                log(&format!("  test plan {} deferred: {error:#}", finding.file));
                skipped.push(SkippedFile {
                    file: finding.file.clone(),
                    stage: ReviewStage::TestPlan,
                    reason: format!("{error:#}")
                        .chars()
                        .take(MAX_SKIP_REASON_CHARS)
                        .collect(),
                });
            }
        }
    }
}

/// Whole-review stages are deferred to merge, retaining global top-K semantics.
pub async fn finish_findings<S: ReviewStrategy>(
    strategy: &S,
    files: &HashMap<&str, &S::File>,
    mut findings: Vec<Finding>,
    refine: bool,
    log: &dyn Fn(&str),
    concurrency: usize,
) -> (Vec<Finding>, RefineCounts) {
    // Stable ties make the global caps independent of discovery/shard order.
    findings.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then(a.line.cmp(&b.line))
            .then(a.dimension.key().cmp(b.dimension.key()))
            .then(a.mechanism.cmp(&b.mechanism))
    });
    // 6. Refine: dedupe, taint, counterfactual, ensemble, pairwise rank.
    let mut refine_counts = RefineCounts::default();
    if refine && !findings.is_empty() {
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
        findings.sort_by(|a, b| {
            b.severity
                .partial_cmp(&a.severity)
                .unwrap_or(Ordering::Equal)
        });
    }

    // 7. Enrich: title/why for every finding (deterministic); fix/test via one
    //    narrow Jev call each, for the first MAX_ENRICH findings in report
    //    order: by severity, with refinement's pairwise-ranked top-K (K <=
    //    MAX_ENRICH) re-ordered within it, so the same findings are chosen.
    for finding in &mut findings {
        explain::apply_context(finding);
    }
    let enrich_cap = MAX_ENRICH.min(findings.len());
    if enrich_cap > 0 {
        let suggestions: Vec<(Option<String>, Option<String>)> =
            stream::iter(findings.iter().take(enrich_cap))
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

    (findings, refine_counts)
}

/// Return the highest dimension probability for profile candidate ranking.
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
                let result = if *ok {
                    Ok(i)
                } else {
                    Err(anyhow!("400 max_tokens_exceeded"))
                };
                (file, result)
            })
            .collect()
    }

    #[tokio::test]
    async fn one_failure_is_skipped_not_fatal() {
        let mut skipped = Vec::new();
        let kept = tolerate(
            stream::iter(results(&[true, false, true])),
            ReviewStage::Screen,
            true,
            &|_| {},
            &mut skipped,
        )
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
        let err = tolerate(items, ReviewStage::Screen, true, &|_| {}, &mut skipped)
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("first 5 screen requests all failed"),
            "{err:#}"
        );
        assert!(
            format!("{err:#}").contains("max_tokens_exceeded"),
            "{err:#}"
        );
        assert_eq!(
            pulled.load(std::sync::atomic::Ordering::SeqCst),
            CIRCUIT_BREAKER,
            "stops at the breaker"
        );
    }

    #[tokio::test]
    async fn failures_after_a_success_never_trip_the_breaker() {
        let mut outcomes = vec![true];
        outcomes.extend([false; 10]);
        let mut skipped = Vec::new();
        let kept = tolerate(
            stream::iter(results(&outcomes)),
            ReviewStage::Locate,
            true,
            &|_| {},
            &mut skipped,
        )
        .await
        .unwrap();
        assert_eq!(kept, vec![0]);
        assert_eq!(skipped.len(), 10);
        assert!(skipped.iter().all(|s| s.stage == ReviewStage::Locate));
    }

    /// `run_stage` over `outcomes` at `concurrency`: (kept, skipped count,
    /// requests started, error).
    async fn stage(
        outcomes: &[bool],
        concurrency: usize,
    ) -> (Vec<usize>, usize, usize, Option<String>) {
        let started = std::sync::atomic::AtomicUsize::new(0);
        let items: Vec<(usize, bool)> = outcomes.iter().copied().enumerate().collect();
        let mut skipped = Vec::new();
        let result = run_stage(
            items,
            concurrency,
            ReviewStage::Screen,
            &|_| {},
            &mut skipped,
            |(i, ok)| {
                started.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                async move {
                    (
                        format!("f{i}.rs"),
                        if ok { Ok(i) } else { Err(anyhow!("boom")) },
                    )
                }
            },
        )
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
        assert_eq!(
            started, CIRCUIT_BREAKER,
            "at MOMUS_CONCURRENCY=10, only the probe went out"
        );
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
        let items = stream::iter(vec![(
            "f.rs".to_string(),
            Err::<usize, _>(anyhow!("x".repeat(1000))),
        )]);
        let mut skipped = Vec::new();
        tolerate(items, ReviewStage::Profile, true, &|_| {}, &mut skipped)
            .await
            .unwrap();
        assert_eq!(skipped[0].reason.chars().count(), MAX_SKIP_REASON_CHARS);
    }

    fn sig(dim: Dimension, p: f64) -> Signal<ChangedFile> {
        Signal {
            file: ChangedFile {
                path: format!("{dim:?}"),
                patch: String::new(),
                base: String::new(),
            },
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
        assert_eq!(options.follow_up_strategy, FollowUpStrategy::Probability);
        assert!(options.threshold_overrides.is_empty());
        assert!(!options.test_plans);
        assert!(options.specs.is_none());
    }

    #[tokio::test]
    async fn sequential_evidence_stage_completes_priority_pipeline_before_next() {
        use std::sync::{Arc, Mutex};
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut skipped = Vec::new();
        let outcomes = run_stage(
            vec!["security", "testGap"],
            1,
            ReviewStage::Locate,
            &|_| {},
            &mut skipped,
            |dimension| {
                let events = events.clone();
                async move {
                    events.lock().unwrap().push(format!("{dimension}:evidence"));
                    tokio::task::yield_now().await;
                    events
                        .lock()
                        .unwrap()
                        .push(format!("{dimension}:mechanism"));
                    (dimension.to_string(), Ok(dimension))
                }
            },
        )
        .await
        .unwrap();
        assert_eq!(outcomes, ["security", "testGap"]);
        assert!(skipped.is_empty());
        assert_eq!(
            *events.lock().unwrap(),
            [
                "security:evidence",
                "security:mechanism",
                "testGap:evidence",
                "testGap:mechanism"
            ]
        );
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
