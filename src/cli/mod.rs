//! CLI entry points: `review` (diff), `scan` (codebase), `dashboard`.
//! Mirrors the four `cli/*.ts` entry points consolidated under one clap binary.

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use crate::adapters::exclude::Exclude;
use crate::adapters::git;
use crate::adapters::report_store::{report_path, save_history, save_json, save_report};
use crate::adapters::sarif;
use crate::domain::report::Action;
use crate::review::changes::ChangesStrategy;
use crate::review::codebase::CodebaseStrategy;
use crate::review::strategy::ReviewStrategy;
use crate::review::workflow::run_review;

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

        /// Cap follow-ups at N signals (default: follow every threshold signal)
        #[arg(long = "follow-ups", value_name = "N")]
        follow_ups: Option<usize>,

        /// Also write a SARIF 2.1.0 log to this path
        #[arg(long = "sarif", value_name = "PATH")]
        sarif: Option<String>,
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
        Command::Review { paths, fail_on_blocking, exclude, follow_ups, sarif } => {
            let strategy = ChangesStrategy::new(Exclude::new(&exclude)?)?;
            let sarif = sarif.map(PathBuf::from);
            run_mode(paths, fail_on_blocking, follow_ups, sarif, strategy).await
        }
        Command::Scan { paths, fail_on_blocking, exclude, follow_ups, sarif } => {
            let strategy = CodebaseStrategy::new(Exclude::new(&exclude)?)?;
            let sarif = sarif.map(PathBuf::from);
            run_mode(paths, fail_on_blocking, follow_ups, sarif, strategy).await
        }
        Command::Dashboard { port } => crate::dashboard::serve(port).await,
    }
}

/// Runs a review, saves the report, prints JSON to stdout, and applies the
/// CI exit contract. Also writes history (always) and SARIF (when requested).
async fn run_mode<S: ReviewStrategy>(
    paths: Vec<String>,
    fail_on_blocking: bool,
    max_follow_ups: Option<usize>,
    sarif: Option<PathBuf>,
    strategy: S,
) -> Result<()> {
    let scopes: Vec<std::path::PathBuf> = paths
        .iter()
        .map(|p| std::path::absolute(std::path::Path::new(p)))
        .collect::<std::io::Result<_>>()?;
    let log = |msg: &str| eprintln!("{msg}");

    let report = run_review(&scopes, &log, max_follow_ups, strategy).await?;

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