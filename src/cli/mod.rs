//! CLI entry points: `review` (diff), `scan` (codebase), `github-review`
//! (publish to a pull request), `dashboard`, under one clap binary.

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

use crate::adapters::exclude::Exclude;
use crate::adapters::feedback_store::{feedback_path, read_feedback};
use crate::adapters::git;
use crate::adapters::github::{GitHubClient, PullRequest};
use crate::adapters::report_store::{StoredReport, read_report, report_path, save_history, save_json, save_report};
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
#[command(name = "momus", version, about = "Fast, calibrated, staged code review")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Review the current Git diff (tracked changes + untracked files)
    Review {
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
    },

    /// Publish the saved report to its pull request (inside GitHub Actions):
    /// inline review comments on the diff plus one sticky summary comment
    GithubReview {
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

pub async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Review { paths, fail_on_blocking, exclude, allow_empty, base, follow_ups, sarif, no_refine, no_redact } => {
            let strategy = ChangesStrategy::new(client(no_redact)?, Exclude::new(&exclude)?, base);
            let sarif = sarif.map(PathBuf::from);
            run_mode(paths, fail_on_blocking, options(follow_ups, no_refine, allow_empty), sarif, strategy).await
        }
        Command::Scan { paths, fail_on_blocking, exclude, follow_ups, sarif, no_refine, no_redact } => {
            let strategy = CodebaseStrategy::new(client(no_redact)?, Exclude::new(&exclude)?);
            let sarif = sarif.map(PathBuf::from);
            run_mode(paths, fail_on_blocking, options(follow_ups, no_refine, false), sarif, strategy).await
        }
        Command::GithubReview { report, max_comments, event, fail_on_blocking, dry_run, sanitized_report } => {
            let options = PublishOptions { max_comments, event: event.into(), dry_run };
            github_review(report.map(PathBuf::from), sanitized_report.map(PathBuf::from), options, fail_on_blocking).await
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
) -> Result<()> {
    let path = report.unwrap_or_else(report_path);
    let report = match read_report(&path) {
        StoredReport::Ok { report, .. } => report,
        StoredReport::Empty => anyhow::bail!("no report at {} (run `momus review` first)", path.display()),
        StoredReport::Error(e) => anyhow::bail!("{}: {e}", path.display()),
    };
    // Before publishing, so a posting failure still leaves the artifact.
    if let Some(path) = &sanitized_report {
        write_sanitized_report(&report, path)?;
    }
    let pr = PullRequest::from_env()?;
    let client = GitHubClient::from_env()?;

    let outcome = publish(&client, &pr, &report, &options).await?;
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
        if outcome.review_rejected { ", review rejected" } else { "" },
        outcome.summary_action,
    );

    if fail_on_blocking && report.findings.iter().any(|f| f.action == Action::RequestChanges) {
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
fn client(no_redact: bool) -> Result<TypeSafeClient> {
    let client = TypeSafeClient::from_env()?;
    Ok(if no_redact { client.without_redaction() } else { client })
}

/// Review options from CLI flags plus the saved feedback log. Feedback is
/// best-effort: an unreadable log is reported and ignored, not fatal.
fn options(max_follow_ups: Option<usize>, no_refine: bool, allow_empty: bool) -> ReviewOptions {
    let path = feedback_path();
    let feedback = read_feedback(&path).unwrap_or_else(|e| {
        eprintln!("feedback ignored: {e:#}");
        Default::default()
    });
    ReviewOptions { max_follow_ups, refine: !no_refine, feedback, allow_empty }
}

/// Runs a review, saves the report, prints JSON to stdout, and applies the
/// CI exit contract. Also writes history (always) and SARIF (when requested).
async fn run_mode<S: ReviewStrategy>(
    paths: Vec<String>,
    fail_on_blocking: bool,
    options: ReviewOptions,
    sarif: Option<PathBuf>,
    strategy: S,
) -> Result<()> {
    let scopes: Vec<std::path::PathBuf> = paths
        .iter()
        .map(|p| std::path::absolute(std::path::Path::new(p)))
        .collect::<std::io::Result<_>>()?;
    let log = |msg: &str| eprintln!("{msg}");

    let report = run_review(&scopes, &log, options, strategy).await?;

    if !report.redactions.is_empty() {
        let rules: Vec<String> =
            report.redactions.iter().map(|(rule, n)| format!("{rule} ×{n}")).collect();
        eprintln!("redacted secrets before sending: {}", rules.join(", "));
    }

    if !report.skipped.is_empty() {
        eprintln!(
            "skipped {} failed request(s); see `skipped` in the report",
            report.skipped.len()
        );
    }

    let out = report_path();
    save_report(&report, &out)?;
    eprintln!("saved {}", out.display());

    if let Some(path) = &sarif {
        save_json(&sarif::to_sarif(&report), path)?;
        eprintln!("saved {}", path.display());
    }

    // History is best-effort: an unborn repo or an I/O failure must not
    // discard an already-produced report.
    match git::head_sha(&scopes[0]).and_then(|sha| save_history(&report, &sha)) {
        Ok(path) => eprintln!("saved {}", path.display()),
        Err(e) => eprintln!("history not saved: {e:#}"),
    }

    println!("{}", serde_json::to_string_pretty(&report)?);

    if fail_on_blocking && report.findings.iter().any(|f| f.action == Action::RequestChanges) {
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::report::Finding;

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