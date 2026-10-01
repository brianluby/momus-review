//! CLI entry points: `review` (diff), `scan` (codebase), `github-review`
//! (publish to a pull request), `dashboard`, under one clap binary.

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use std::path::{Path, PathBuf};

use crate::adapters::exclude::Exclude;
use crate::adapters::feedback_store::{feedback_path, read_feedback};
use crate::adapters::git;
use crate::adapters::github::{GitHubClient, PullRequest};
use crate::adapters::report_store::{
    StoredReport, read_report, report_path, save_history, save_json, save_report,
};
use crate::adapters::sarif;
use crate::domain::redact::{Redactions, redact_value};
use crate::domain::report::{Action, ReviewReport};
use crate::review::changes::ChangesStrategy;
use crate::review::codebase::CodebaseStrategy;
use crate::review::publish::{PublishOptions, ReviewEvent, publish};
use crate::review::strategy::ReviewStrategy;
use crate::review::typesafe::TypeSafeClient;
use crate::review::workflow::{ReviewOptions, run_review};

#[derive(Parser)]
#[command(
    name = "momus",
    version,
    about = "Fast, staged code review with explicit evidence"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

/// Optional robustness judgments; ordinary reviews retain their existing funnel.
#[derive(Debug, Default, Args)]
pub struct RobustnessArgs {
    /// Emit unfinished test scaffolds for well-supported test-gap findings
    #[arg(long)]
    test_plans: bool,
    /// Compare reviewed source with a local UTF-8 requirement file (repeatable)
    #[arg(long, value_name = "PATH")]
    spec: Vec<PathBuf>,
    /// Rank follow-ups by probability or an auditable VOI heuristic
    #[arg(long, value_enum, default_value = "probability")]
    follow_up_strategy: crate::review::voi::FollowUpStrategy,
    /// Override one dimension's screening threshold, e.g. security=0.7 (repeatable)
    #[arg(long, value_name = "DIMENSION=P", value_parser = crate::review::voi::parse_threshold)]
    threshold: Vec<(crate::domain::policy::Dimension, f64)>,
}

impl RobustnessArgs {
    /// Validate global-option boundaries and load authoritative specs before API work.
    fn apply(self, mut options: ReviewOptions) -> Result<ReviewOptions> {
        if options.shard.is_some()
            && (self.test_plans
                || !self.spec.is_empty()
                || self.follow_up_strategy != crate::review::voi::FollowUpStrategy::Probability)
        {
            anyhow::bail!(
                "--shard cannot use --test-plans, --spec or VOI: these require a global review"
            );
        }
        if options.tiered && !self.spec.is_empty() {
            anyhow::bail!(
                "--spec cannot use --tiered: dismissed files would escape requirement checks"
            );
        }
        for (dimension, threshold) in self.threshold {
            if options
                .threshold_overrides
                .insert(dimension, threshold)
                .is_some()
            {
                anyhow::bail!("duplicate threshold override for {}", dimension.key());
            }
        }
        options.test_plans = self.test_plans;
        options.follow_up_strategy = self.follow_up_strategy;
        if !self.spec.is_empty() {
            options.specs = Some(crate::review::spec_drift::SpecInputs::load(&self.spec)?);
        }
        Ok(options)
    }
}

#[derive(Subcommand)]
pub enum Command {
    /// Evaluate outcome history and opt-in approval policy without publishing
    Confidence {
        /// Saved review report to assess; this command never publishes approval
        #[arg(long)]
        report: PathBuf,
        /// Trusted, provenance-backed observed history; absent history stays unknown
        #[arg(long)]
        history: Option<PathBuf>,
        /// Trusted opt-in approval policy for offline eligibility assessment
        #[arg(long)]
        policy: Option<PathBuf>,
        /// Explicit offline completeness/check assertions; live publication rechecks GitHub
        #[arg(long)]
        checks: Option<PathBuf>,
    },
    /// Generate a bounded, redacted repository tour without API requests
    Tour {
        #[arg(default_value = ".", value_name = "PATH")]
        paths: Vec<PathBuf>,
        #[arg(long)]
        exclude: Vec<String>,
        #[arg(long, default_value_t = 500, value_parser = clap::value_parser!(u32).range(1..=10000))]
        max_files: u32,
        #[arg(long, default_value_t = 1_000_000, value_parser = clap::value_parser!(u32).range(1..=10_000_000))]
        max_file_bytes: u32,
        #[arg(long, default_value_t = 8_000_000, value_parser = clap::value_parser!(u32).range(1..=100_000_000))]
        max_total_bytes: u32,
        /// Emit readable Markdown instead of JSON
        #[arg(long)]
        markdown: bool,
    },
    /// Review the current Git diff (tracked changes + untracked files)
    Review {
        /// Review a pinned committed tree; required for automatic approval
        #[arg(long, requires = "base")]
        committed_only: bool,
        /// Compare Cargo/npm dependency changes with available local release notes
        #[arg(long)]
        upgrade_triage: bool,
        /// Check supported documented public-interface examples against code
        #[arg(long)]
        docs_drift: bool,
        /// Scope directory/directories (defaults to the current directory);
        /// multiple paths are unioned into one run
        #[arg(default_value = ".", value_name = "PATH")]
        paths: Vec<String>,

        /// Exit non-zero when any finding requests changes
        #[arg(long)]
        fail_on_blocking: bool,

        /// Exclude paths matching a gitignore-style glob (repeatable),
        /// e.g. `--exclude 'frontend/src/assets/**'`
        #[arg(long = "exclude", value_name = "GLOB")]
        exclude: Vec<String>,

        /// No changed source files is an empty report and exit 0, not an
        /// error (CI: a docs- or config-only pull request)
        #[arg(long)]
        allow_empty: bool,

        /// Diff against the merge base of REV and HEAD instead of HEAD: the
        /// branch's commits plus uncommitted changes to tracked files;
        /// untracked files are ignored (a PR's diff in CI)
        #[arg(long = "base", value_name = "REV")]
        base: Option<String>,

        /// Cap follow-ups at N signals (default: follow every threshold signal)
        #[arg(long = "follow-ups", value_name = "N")]
        follow_ups: Option<usize>,

        /// Also write a SARIF 2.1.0 log to this path
        #[arg(long = "sarif", value_name = "PATH")]
        sarif: Option<String>,

        /// Skip refinement (dedupe, taint, counterfactual, ensemble, pairwise
        /// ranking): fewer API calls, noisier findings
        #[arg(long)]
        no_refine: bool,

        /// Send code to the API without redacting secrets (also
        /// `MOMUS_REDACT=off`); redaction is on by default
        #[arg(long)]
        no_redact: bool,

        /// Bypass the local result cache (reads and writes)
        #[arg(long)]
        no_cache: bool,

        /// Maximum HTTP attempts; cached units cost no budget (calls=N)
        #[arg(long, value_parser = crate::review::planner::parse_budget)]
        budget: Option<u64>,

        #[command(flatten)]
        robustness: RobustnessArgs,
    },

    /// Scan every non-ignored source file under a scope
    Scan {
        /// Scope directory/directories (defaults to the current directory);
        /// multiple paths are unioned into one run
        #[arg(default_value = ".", value_name = "PATH")]
        paths: Vec<String>,

        /// Exit non-zero when any finding requests changes
        #[arg(long)]
        fail_on_blocking: bool,

        /// Exclude paths matching a gitignore-style glob (repeatable)
        #[arg(long = "exclude", value_name = "GLOB")]
        exclude: Vec<String>,

        /// Cap follow-ups at N signals (default: follow every threshold signal)
        #[arg(long = "follow-ups", value_name = "N")]
        follow_ups: Option<usize>,

        /// Also write a SARIF 2.1.0 log to this path
        #[arg(long = "sarif", value_name = "PATH")]
        sarif: Option<String>,

        /// Skip refinement (dedupe, taint, counterfactual, ensemble, pairwise
        /// ranking): fewer API calls, noisier findings
        #[arg(long)]
        no_refine: bool,

        /// Send code to the API without redacting secrets (also
        /// `MOMUS_REDACT=off`); redaction is on by default
        #[arg(long)]
        no_redact: bool,

        /// Bypass the local result cache (reads and writes)
        #[arg(long)]
        no_cache: bool,

        /// Maximum HTTP attempts; cached units cost no budget (calls=N)
        #[arg(long, value_parser = crate::review::planner::parse_budget)]
        budget: Option<u64>,

        /// Experimental cheap pre-screen; disabled until the evaluation gate passes
        #[arg(long)]
        tiered: bool,

        /// Write a redacted copy for CI artifact exchange
        #[arg(long)]
        sanitized_report: Option<PathBuf>,

        /// Emit a partial scan for stable path partition i/N (1-based)
        #[arg(long)]
        shard: Option<crate::review::planner::Shard>,

        #[command(flatten)]
        robustness: RobustnessArgs,
    },

    /// Combine every partial scan, then refine/rank the combined findings
    Merge {
        #[arg(required = true, value_name = "REPORT")]
        reports: Vec<PathBuf>,
        /// Checkout scope used by the shards (repeatable)
        #[arg(long, default_value = ".")]
        scope: Vec<PathBuf>,
        #[arg(long)]
        sarif: Option<PathBuf>,
        #[arg(long)]
        fail_on_blocking: bool,
        #[arg(long)]
        no_cache: bool,
        #[arg(long)]
        no_redact: bool,
        #[arg(long)]
        exclude: Vec<String>,
    },

    /// Publish the saved report to its pull request (inside GitHub Actions):
    /// inline review comments on the diff plus one sticky summary comment
    GithubReview {
        /// Trusted opt-in approval policy; eligibility is recomputed from live GitHub evidence
        #[arg(long, requires = "history")]
        auto_approve_policy: Option<PathBuf>,
        /// Trusted observed history required with the opt-in approval policy
        #[arg(long, requires = "auto_approve_policy")]
        history: Option<PathBuf>,
        /// The report to publish (default: `MOMUS_REPORT` or reviews/latest.json)
        #[arg(long = "report", value_name = "PATH")]
        report: Option<String>,

        /// Post at most N new inline comments; the rest go in the summary
        #[arg(long = "max-comments", value_name = "N", default_value_t = crate::review::publish::DEFAULT_MAX_COMMENTS)]
        max_comments: usize,

        /// Review event: `comment`, or `request-changes` to request changes
        /// when an inline finding blocks
        #[arg(long = "event", value_enum, default_value_t = EventArg::Comment)]
        event: EventArg,

        /// Exit non-zero when any finding requests changes (after publishing)
        #[arg(long)]
        fail_on_blocking: bool,

        /// Read the pull request but post nothing; print what would be posted
        #[arg(long)]
        dry_run: bool,

        /// Write the report with secret values replaced by typed
        /// placeholders to PATH (the same rules as outgoing requests); the
        /// raw report keeps verbatim evidence and stays local
        #[arg(long = "sanitized-report", value_name = "PATH")]
        sanitized_report: Option<String>,
    },

    /// Serve the loopback dashboard
    Dashboard {
        /// Port (1–65535, default 4317)
        #[arg(long, default_value_t = crate::dashboard::DEFAULT_PORT, value_parser = clap::value_parser!(u16).range(1..))]
        port: u16,
    },
}

/// Dispatch the parsed CLI command and preserve each command's exit contract.
pub async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Confidence {
            report,
            history,
            policy,
            checks,
        } => {
            let report: ReviewReport = load_json(&report)?;
            let history = history.as_deref().map(load_json).transpose()?;
            let policy = policy.as_deref().map(load_json).transpose()?;
            let checks = checks.as_deref().map(load_json).transpose()?;
            let summary = crate::review::merge_confidence::assess(
                &report,
                history.as_ref(),
                policy.as_ref(),
                checks.as_ref(),
            )?;
            let mut value = serde_json::to_value(summary)?;
            redact_value(&mut value, &mut Redactions::default());
            println!("{}", serde_json::to_string_pretty(&value)?);
            Ok(())
        }
        Command::Tour {
            paths,
            exclude,
            max_files,
            max_file_bytes,
            max_total_bytes,
            markdown,
        } => {
            let initial_head = paths.first().and_then(|path| git::head_sha(path).ok());
            let clean_before = initial_head.is_some()
                && paths
                    .iter()
                    .all(|path| git::tracked_checkout_clean(path).unwrap_or(false));
            let mut evidence = git::repository_evidence(
                &paths,
                &Exclude::new(&exclude)?,
                None,
                max_files as usize,
                max_file_bytes as usize,
                max_total_bytes as usize,
            )?;
            if !clean_before
                || paths.iter().any(|path| {
                    !git::tracked_checkout_clean(path).unwrap_or(false)
                        || git::head_sha(path).ok() != initial_head
                })
            {
                evidence.unknowns.push("Tour describes uncommitted or untracked working-tree evidence; head identifies the checkout baseline, not an immutable tour snapshot".into());
            }
            let tour = crate::review::tour::build(
                evidence,
                &crate::adapters::index_store::IndexStore::from_env(),
            );
            let mut value = serde_json::to_value(tour)?;
            redact_value(&mut value, &mut Redactions::default());
            if markdown {
                let tour = serde_json::from_value(value)?;
                print!("{}", crate::review::tour::markdown(&tour));
            } else {
                println!("{}", serde_json::to_string_pretty(&value)?);
            }
            Ok(())
        }
        Command::Review {
            committed_only,
            upgrade_triage,
            docs_drift,
            paths,
            fail_on_blocking,
            exclude,
            allow_empty,
            base,
            follow_ups,
            sarif,
            no_refine,
            no_redact,
            no_cache,
            budget,
            robustness,
        } => {
            if committed_only && (upgrade_triage || docs_drift) {
                anyhow::bail!(
                    "--committed-only cannot combine --upgrade-triage or --docs-drift; run worktree analyses separately from the committed-only approval review"
                );
            }
            let mut options = robustness.apply(options(
                follow_ups,
                no_refine,
                allow_empty || upgrade_triage || docs_drift,
                false,
            ))?;
            if upgrade_triage || docs_drift {
                let scopes: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
                options.auxiliary = Some(crate::review::auxiliary::AuxiliaryInputs {
                    evidence: git::repository_evidence(
                        &scopes,
                        &Exclude::new(&exclude)?,
                        base.as_deref(),
                        500,
                        1_000_000,
                        8_000_000,
                    )?,
                    upgrades: upgrade_triage,
                    docs_drift,
                });
            }
            let mut strategy = ChangesStrategy::new(
                client(no_redact, no_cache)?.with_budget(budget),
                Exclude::new(&exclude)?,
                base.clone(),
            );
            let committed_head = if committed_only {
                let first = paths
                    .first()
                    .ok_or_else(|| anyhow::anyhow!("committed review requires a scope"))?;
                let root = git::repository_root(Path::new(first))?.canonicalize()?;
                for path in &paths {
                    anyhow::ensure!(
                        git::repository_root(Path::new(path))?.canonicalize()? == root,
                        "committed-only scopes must share one canonical checkout"
                    );
                }
                Some(git::head_sha(Path::new(first))?)
            } else {
                None
            };
            if let Some(head) = &committed_head {
                strategy = strategy.with_committed_head(head.clone());
            }
            let sarif = sarif.map(PathBuf::from);
            let reviewed_base = paths
                .first()
                .zip(base.as_deref())
                .map(|(p, b)| git::review_base_sha(Path::new(p), b))
                .transpose()?;
            run_mode(
                paths,
                fail_on_blocking,
                options,
                sarif,
                strategy,
                None,
                ReviewIdentity {
                    base: reviewed_base,
                    committed_head,
                },
            )
            .await
        }
        Command::Scan {
            paths,
            fail_on_blocking,
            exclude,
            follow_ups,
            sarif,
            no_refine,
            no_redact,
            no_cache,
            budget,
            tiered,
            shard,
            sanitized_report,
            robustness,
        } => {
            if shard.is_some() && follow_ups.is_some() {
                anyhow::bail!(
                    "--shard cannot use --follow-ups: a per-shard cap changes global selection"
                );
            }
            let options = robustness.apply(ReviewOptions {
                shard,
                ..options(follow_ups, no_refine, false, tiered)
            })?;
            let strategy = CodebaseStrategy::new(
                client(no_redact, no_cache)?.with_budget(budget),
                Exclude::new(&exclude)?,
            );
            let sarif = sarif.map(PathBuf::from);
            run_mode(
                paths,
                fail_on_blocking,
                options,
                sarif,
                strategy,
                sanitized_report,
                ReviewIdentity::default(),
            )
            .await
        }
        Command::Merge {
            reports,
            scope,
            sarif,
            fail_on_blocking,
            no_cache,
            no_redact,
            exclude,
        } => {
            let started = std::time::Instant::now();
            let parts = reports
                .iter()
                .map(|path| {
                    let text = std::fs::read_to_string(path)?;
                    Ok(serde_json::from_str::<ReviewReport>(&text)?)
                })
                .collect::<Result<Vec<_>>>()?;
            let scope = scope
                .iter()
                .map(std::path::absolute)
                .collect::<std::io::Result<Vec<_>>>()?;
            let strategy =
                CodebaseStrategy::new(client(no_redact, no_cache)?, Exclude::new(&exclude)?);
            let mut report =
                crate::review::shard_merge::merge_reports(parts, &scope, strategy, &|s| {
                    eprintln!("{s}")
                })
                .await?;
            attach_default_merge_outcomes(&mut report);
            report.wall_time_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
            eprintln!("{}", metrics_summary(&report));
            persist_report(&report, &scope, sarif.as_ref(), fail_on_blocking)
        }
        Command::GithubReview {
            auto_approve_policy,
            history,
            report,
            max_comments,
            event,
            fail_on_blocking,
            dry_run,
            sanitized_report,
        } => {
            let options = PublishOptions {
                max_comments,
                event: event.into(),
                dry_run,
            };
            github_review(
                report.map(PathBuf::from),
                sanitized_report.map(PathBuf::from),
                options,
                fail_on_blocking,
                auto_approve_policy,
                history,
            )
            .await
        }
        Command::Dashboard { port } => crate::dashboard::serve(port).await,
    }
}

/// `--event` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum EventArg {
    Comment,
    RequestChanges,
}

impl From<EventArg> for ReviewEvent {
    /// Translate the CLI review event into the publisher's event type.
    fn from(event: EventArg) -> Self {
        match event {
            EventArg::Comment => ReviewEvent::Comment,
            EventArg::RequestChanges => ReviewEvent::RequestChanges,
        }
    }
}

/// Publishes the saved report to the pull request named by the Actions
/// environment, then applies the CI exit contract.
async fn github_review(
    report: Option<PathBuf>,
    sanitized_report: Option<PathBuf>,
    options: PublishOptions,
    fail_on_blocking: bool,
    auto_approve_policy: Option<PathBuf>,
    history: Option<PathBuf>,
) -> Result<()> {
    let path = report.unwrap_or_else(report_path);
    let mut report = match read_report(&path) {
        StoredReport::Ok { report, .. } => report,
        StoredReport::Empty => {
            anyhow::bail!("no report at {} (run `momus review` first)", path.display())
        }
        StoredReport::Error(e) => anyhow::bail!("{}: {e}", path.display()),
    };
    if report.shard.is_some() {
        anyhow::bail!("partial shard reports must be merged before publication");
    }
    // Before publishing, so a posting failure still leaves the artifact.
    if let Some(path) = &sanitized_report {
        write_sanitized_report(&report, path)?;
    }
    let pr = PullRequest::from_env()?;
    let client = GitHubClient::from_env()?;

    let approval = if let Some(path) = auto_approve_policy {
        let policy = load_json(&path)?;
        let history = history.as_deref().map(load_json).transpose()?;
        let checks = client.approval_evidence(&pr, &report).await?;
        let summary = crate::review::merge_confidence::assess(
            &report,
            history.as_ref(),
            Some(&policy),
            Some(&checks),
        )?;
        let decision = summary.approval.clone();
        report.merge_confidence = Some(summary);
        Some((decision, policy, history))
    } else {
        None
    };

    if approval.is_some()
        && let Some(path) = &sanitized_report
    {
        write_sanitized_report(&report, path)?;
    }
    let outcome = publish(&client, &pr, &report, &options).await?;
    let mut approval_error = None;
    if let Some((decision, policy, history)) = approval {
        if decision.eligible && !outcome.review_rejected {
            if options.dry_run {
                eprintln!("automatic approval eligible (dry run; no approval posted)");
            } else {
                match client
                    .approve_current_head(&pr, &report, &policy, history.as_ref())
                    .await
                {
                    Ok(()) => eprintln!("automatic approval posted for {}", pr.head_sha),
                    Err(error) => {
                        if let Some(summary) = &mut report.merge_confidence {
                            summary.approval.eligible = false;
                            summary.approval.reasons.push(format!(
                                "Final approval publication was not confirmed: {error}"
                            ));
                        }
                        eprintln!("automatic approval publication was not confirmed: {error}");
                        approval_error = Some(error);
                    }
                }
            }
        } else if outcome.review_rejected {
            if let Some(summary) = &mut report.merge_confidence {
                summary.approval.eligible = false;
                summary.approval.reasons.push(
                    "GitHub rejected the review submission; automatic approval was withheld."
                        .into(),
                );
            }
            eprintln!("automatic approval withheld: GitHub rejected the review submission");
        } else {
            eprintln!(
                "automatic approval rejected: {}",
                decision.reasons.join("; ")
            );
        }
        if let Some(path) = &sanitized_report {
            write_sanitized_report(&report, path)?;
        }
    }
    let inline = outcome.review.as_ref().map_or(0, |r| r.comments.len());
    if options.dry_run {
        let preview = serde_json::json!({ "review": outcome.review, "summary": outcome.summary });
        println!("{}", serde_json::to_string_pretty(&preview)?);
    }
    eprintln!(
        "{}#{}: {inline} inline, {} already posted, {} in summary{} (summary {:?})",
        pr.repository,
        pr.number,
        outcome.already_posted,
        outcome.summary_only,
        if outcome.review_rejected {
            ", review rejected"
        } else {
            ""
        },
        outcome.summary_action,
    );
    if let Some(error) = approval_error {
        return Err(
            error.context("review publication completed; automatic approval was not confirmed")
        );
    }

    if fail_on_blocking
        && report
            .findings
            .iter()
            .any(|f| f.action == Action::RequestChanges)
    {
        std::process::exit(1);
    }
    Ok(())
}

/// The report with secret values replaced by typed placeholders — the same
/// rules as outgoing requests (`domain::redact`) — for CI artifact upload.
/// The report at `--report` keeps verbatim evidence and stays local
/// (docs/security.md).
fn write_sanitized_report(report: &ReviewReport, path: &Path) -> Result<()> {
    let mut value = serde_json::to_value(report)?;
    redact_value(&mut value, &mut Redactions::default());
    std::fs::write(path, serde_json::to_string_pretty(&value)?)?;
    Ok(())
}

/// The System One client from the environment; `--no-redact` overrides
/// `MOMUS_REDACT`.
fn client(no_redact: bool, no_cache: bool) -> Result<TypeSafeClient> {
    let client = TypeSafeClient::from_env()?;
    let client = if no_redact {
        client.without_redaction()
    } else {
        client
    };
    Ok(if no_cache {
        client.without_cache()
    } else {
        client
    })
}

/// Review options from CLI flags plus the saved feedback log. Feedback is
/// best-effort: an unreadable log is reported and ignored, not fatal.
fn options(
    max_follow_ups: Option<usize>,
    no_refine: bool,
    allow_empty: bool,
    tiered: bool,
) -> ReviewOptions {
    let path = feedback_path();
    let feedback = read_feedback(&path).unwrap_or_else(|e| {
        eprintln!("feedback ignored: {e:#}");
        Default::default()
    });
    ReviewOptions {
        max_follow_ups,
        refine: !no_refine,
        feedback,
        allow_empty,
        tiered,
        shard: None,
        ..Default::default()
    }
}

/// A stable, grep-friendly line from the report's own counters (#47).
fn metrics_summary(r: &ReviewReport) -> String {
    format!(
        "momus_metrics wall_ms={} files={} signals={} findings={} blocking={} calls={} tokens_in={} tokens_out={} cache_hits={} cache_misses={} dropped_chars={} dropped_items={} skipped={} index_computed={} index_reused={}",
        r.wall_time_ms,
        r.screened_files,
        r.workflow.threshold_signals,
        r.findings.len(),
        r.findings
            .iter()
            .filter(|f| f.action == Action::RequestChanges)
            .count(),
        r.usage.calls,
        r.usage.input_tokens,
        r.usage.output_tokens,
        r.usage.cache.hits,
        r.usage.cache.misses,
        r.workflow.dropped_context_chars,
        r.workflow.dropped_context_items,
        r.skipped.len(),
        r.index.computed,
        r.index.reused
    )
}

/// Source identity; a pinned committed head enables immutable review inputs.
#[derive(Default)]
struct ReviewIdentity {
    base: Option<String>,
    committed_head: Option<String>,
}

/// Run and persist the completed review, with optional history and SARIF.
async fn run_mode<S: ReviewStrategy>(
    paths: Vec<String>,
    fail_on_blocking: bool,
    mut options: ReviewOptions,
    sarif: Option<PathBuf>,
    strategy: S,
    sanitized_report: Option<PathBuf>,
    identity: ReviewIdentity,
) -> Result<()> {
    let scopes: Vec<std::path::PathBuf> = paths
        .iter()
        .map(|p| std::path::absolute(std::path::Path::new(p)))
        .collect::<std::io::Result<_>>()?;
    let log = |msg: &str| eprintln!("{msg}");

    let started = std::time::Instant::now();
    let auxiliary = options.auxiliary.take();
    let feedback = options.feedback.clone();
    let actual_head = scopes.first().and_then(|s| git::head_sha(s).ok());
    let head = identity.committed_head.clone().or(actual_head.clone());
    let roots: Vec<_> = scopes
        .iter()
        .map(|s| git::repository_root(s).ok())
        .collect();
    let same_checkout = roots
        .first()
        .is_some_and(|first| first.is_some() && roots.iter().all(|r| r == first));
    let clean_before = head.is_some()
        && actual_head == head
        && same_checkout
        && scopes
            .iter()
            .all(|s| git::tracked_checkout_clean(s).unwrap_or(false));
    let mut report = run_review(&scopes, &log, options, strategy).await?;
    report.reviewed_head = head;
    report.reviewed_base = identity.base;
    report.reviewed_committed = identity.committed_head.is_some();
    report.reviewed_clean = clean_before
        && scopes.iter().all(|s| {
            git::tracked_checkout_clean(s).unwrap_or(false)
                && git::head_sha(s).ok() == report.reviewed_head
        });
    if !report.reviewed_clean && clean_before {
        report.partial = true;
    }
    if let Some(inputs) = auxiliary {
        crate::review::auxiliary::attach(&mut report, inputs, &feedback);
    }
    attach_default_merge_outcomes(&mut report);
    report.wall_time_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
    eprintln!("{}", metrics_summary(&report));

    if !report.redactions.is_empty() {
        let rules: Vec<String> = report
            .redactions
            .iter()
            .map(|(rule, n)| format!("{rule} ×{n}"))
            .collect();
        eprintln!("redacted secrets before sending: {}", rules.join(", "));
    }

    if !report.skipped.is_empty() {
        eprintln!(
            "skipped {} failed request(s); see `skipped` in the report",
            report.skipped.len()
        );
    }

    if let Some(path) = sanitized_report {
        write_sanitized_report(&report, &path)?;
    }
    persist_report(&report, &scopes, sarif.as_ref(), fail_on_blocking)
}

/// Save JSON and final artifacts; partial shards defer history, SARIF and blocking gates.
fn persist_report(
    report: &ReviewReport,
    scopes: &[PathBuf],
    sarif: Option<&PathBuf>,
    fail_on_blocking: bool,
) -> Result<()> {
    if report.partial {
        eprintln!(
            "partial review: coverage is incomplete; see budget, tier, skipped and shard fields"
        );
    }
    if report.shard.is_some() {
        let out = report_path();
        save_report(report, &out)?;
        eprintln!(
            "saved partial shard {} (global stages deferred to merge)",
            out.display()
        );
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(());
    }
    let out = report_path();
    save_report(report, &out)?;
    eprintln!("saved {}", out.display());

    if let Some(path) = sarif {
        save_json(&sarif::to_sarif(report), path)?;
        eprintln!("saved {}", path.display());
    }

    // History is best-effort: an unborn repo or an I/O failure must not
    // discard an already-produced report.
    match git::head_sha(&scopes[0]).and_then(|sha| save_history(report, &sha)) {
        Ok(path) => eprintln!("saved {}", path.display()),
        Err(e) => eprintln!("history not saved: {e:#}"),
    }

    println!("{}", serde_json::to_string_pretty(&report)?);

    if fail_on_blocking
        && report
            .findings
            .iter()
            .any(|f| f.action == Action::RequestChanges)
    {
        std::process::exit(1);
    }
    Ok(())
}

fn attach_default_merge_outcomes(report: &mut ReviewReport) {
    match crate::review::merge_confidence::assess(report, None, None, None) {
        Ok(summary) => report.merge_confidence = Some(summary),
        Err(error) => {
            report.partial = true;
            report.merge_confidence = None;
            eprintln!(
                "merge outcome assessment unavailable: {error}; completed review preserved, automatic approval unavailable"
            );
        }
    }
}

fn load_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    use anyhow::Context;
    use std::io::Read;
    let file = std::fs::File::open(path)
        .with_context(|| format!("open JSON evidence {}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(20_000_001)
        .read_to_end(&mut bytes)
        .with_context(|| format!("read JSON evidence {}", path.display()))?;
    anyhow::ensure!(
        bytes.len() <= 20_000_000,
        "JSON evidence {} exceeds 20 MB limit",
        path.display()
    );
    serde_json::from_slice(&bytes)
        .with_context(|| format!("parse JSON evidence {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::report::Finding;

    #[test]
    fn metrics_are_derived_from_report_counters() {
        let mut r = ReviewReport {
            wall_time_ms: 42,
            screened_files: 3,
            ..Default::default()
        };
        r.usage.calls = 7;
        r.usage.cache.hits = 5;
        r.findings.push(Finding {
            action: Action::RequestChanges,
            ..Default::default()
        });
        let line = metrics_summary(&r);
        for value in [
            "wall_ms=42",
            "files=3",
            "calls=7",
            "cache_hits=5",
            "findings=1",
            "blocking=1",
        ] {
            assert!(line.contains(value), "{line}");
        }
    }

    #[test]
    fn sanitized_report_replaces_secret_values_not_evidence() {
        let report = ReviewReport {
            screened_files: 2,
            findings: vec![Finding {
                file: "src/config.rs".into(),
                evidence: "let key = \"AKIAIOSFODNN7EXAMPLE\";".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let dir = std::env::temp_dir().join("momus-sanitized-report-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("report.sanitized.json");
        write_sanitized_report(&report, &path).unwrap();

        let body = std::fs::read_to_string(&path).unwrap();
        assert!(!body.contains("AKIAIOSFODNN7EXAMPLE"), "{body}");
        assert!(body.contains("<redacted:aws-access-key>"), "{body}");
        assert!(body.contains("src/config.rs"), "{body}");
    }
}
