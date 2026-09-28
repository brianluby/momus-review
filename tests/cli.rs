//! End-to-end tests of the `momus` binary against a temporary repository.
//! The API points at an unreachable loopback address (no key needed there),
//! so any test that reached the network would fail instead of calling out.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git").arg("-C").arg(repo).args(args).status().expect("git runs");
    assert!(status.success(), "git {args:?} failed");
}

/// A repo whose `feature` branch changes only a YAML file on top of `base`.
fn config_only_branch() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    git(repo, &["init", "-q"]);
    git(repo, &["config", "user.email", "t@t.co"]);
    git(repo, &["config", "user.name", "t"]);
    git(repo, &["config", "commit.gpgsign", "false"]);
    fs::write(repo.join("lib.rs"), "pub fn f() {}\n").unwrap();
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-q", "-m", "seed"]);
    git(repo, &["branch", "base"]);
    git(repo, &["checkout", "-q", "-b", "feature"]);
    fs::create_dir_all(repo.join(".github/workflows")).unwrap();
    fs::write(repo.join(".github/workflows/ci.yml"), "name: ci\non: push\n").unwrap();
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-q", "-m", "config only"]);
    dir
}

fn momus(repo: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_momus"))
        .args(args)
        .current_dir(repo)
        .env_remove("TYPESAFE_API_KEY")
        .env("TYPESAFE_BASE_URL", "http://127.0.0.1:9")
        .env("MOMUS_REPORT", repo.join("report.json"))
        .env_remove("MOMUS_CONCURRENCY")
        .output()
        .expect("momus runs")
}

#[test]
fn allow_empty_turns_nothing_to_review_into_an_empty_report() {
    let dir = config_only_branch();
    let repo = dir.path();

    let out = momus(repo, &["review", "--base", "base", "--allow-empty"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "exit {:?}: {stderr}", out.status.code());
    assert!(stderr.contains("No changed source files to review"), "{stderr}");

    let report: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(repo.join("report.json")).unwrap()).unwrap();
    assert_eq!(report["screenedFiles"], 0);
    assert_eq!(report["findings"], serde_json::json!([]));
    assert_eq!(report["usage"]["calls"], 0, "no API call was made");
}

#[test]
fn without_allow_empty_nothing_to_review_is_still_an_error() {
    let dir = config_only_branch();
    let out = momus(dir.path(), &["review", "--base", "base"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(stderr.contains("No changed source files found"), "{stderr}");
}
