//! CLI entry points: `review` (diff), `scan` (codebase), `dashboard`.
//! Mirrors the four `cli/*.ts` entry points consolidated under one clap binary.

use std::path::Path;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::adapters::exclude::Exclude;
use crate::adapters::report_store::{report_path, save_report};
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
        /// Scope directory (defaults to the current directory)
        #[arg(default_value = ".")]
        path: String,

        /// Exit non-zero when any finding requests changes
        #[arg(long)]
        fail_on_blocking: bool,

        /// Exclude paths matching a gitignore-style glob (repeatable),
        /// e.g. `--exclude 'frontend/src/assets/**'`
        #[arg(long = "exclude", value_name = "GLOB")]
        exclude: Vec<String>,
    },

    /// Scan every non-ignored source file under a scope
    Scan {
        /// Scope directory (defaults to the current directory)
        #[arg(default_value = ".")]
        path: String,

        /// Exit non-zero when any finding requests changes
        #[arg(long)]
        fail_on_blocking: bool,

        /// Exclude paths matching a gitignore-style glob (repeatable)
        #[arg(long = "exclude", value_name = "GLOB")]
        exclude: Vec<String>,
    },

    /// Serve the loopback dashboard
    Dashboard {
        /// Port (1–65535, default 4317)
        #[arg(long, default_value_t = crate::dashboard::DEFAULT_PORT)]
        port: u16,
    },
}

pub async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Review { path, fail_on_blocking, exclude } => {
            let strategy = ChangesStrategy::new(Exclude::new(&exclude)?)?;
            run_mode(path, fail_on_blocking, strategy).await
        }
        Command::Scan { path, fail_on_blocking, exclude } => {
            let strategy = CodebaseStrategy::new(Exclude::new(&exclude)?)?;
            run_mode(path, fail_on_blocking, strategy).await
        }
        Command::Dashboard { port } => crate::dashboard::serve(port).await,
    }
}

/// Runs a review, saves the report, prints JSON to stdout, and applies the
/// CI exit contract.
async fn run_mode<S: ReviewStrategy>(
    path: String,
    fail_on_blocking: bool,
    strategy: S,
) -> Result<()> {
    let scope = std::path::absolute(Path::new(&path))?;
    let log = |msg: &str| eprintln!("{msg}");

    let report = run_review(&scope, &log, strategy).await?;

    let out = report_path();
    save_report(&report, &out)?;
    eprintln!("saved {}", out.display());

    println!("{}", serde_json::to_string_pretty(&report)?);

    if fail_on_blocking && report.findings.iter().any(|f| f.action == Action::RequestChanges) {
        std::process::exit(1);
    }
    Ok(())
}