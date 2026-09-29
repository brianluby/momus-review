//! End-to-end tests of the `momus` binary against a temporary repository.
//! The API is an unreachable loopback address (no key needed there), so a
//! test that reached the network fails instead of calling out, or a local
//! stub server where a test needs answers.

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

/// A stub System One server: answers every `noul` question with a low
/// probability, and fails (400, not retried) any request about `b.rs` or
/// with a non-`noul` question (so every profile request fails).
async fn stub_server() -> String {
    use axum::http::StatusCode;
    use axum::routing::post;
    use axum::{Json, Router};
    use serde_json::{Value, json};

    let app = Router::new().route(
        "/v1/systemone",
        post(|Json(body): Json<Value>| async move {
            let questions = body["questions"].as_object().cloned().unwrap_or_default();
            let about_b = body["state"]["file"]["path"] == "b.rs";
            let all_noul = questions.values().all(|q| q["type"] == "noul");
            if about_b || !all_noul {
                return (StatusCode::BAD_REQUEST, Json(json!({ "detail": { "error_type": "max_tokens_exceeded" } })));
            }
            let answers: serde_json::Map<String, Value> =
                questions.keys().map(|k| (k.clone(), json!({ "noul": 0.1 }))).collect();
            (StatusCode::OK, Json(json!({ "model": "stub", "answers": answers })))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

/// A repo whose `feature` branch adds two source files, `a.rs` and `b.rs`.
fn two_file_branch() -> tempfile::TempDir {
    let dir = config_only_branch();
    let repo = dir.path();
    fs::write(repo.join("a.rs"), "pub fn a() {}\n").unwrap();
    fs::write(repo.join("b.rs"), "pub fn b() {}\n").unwrap();
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-q", "-m", "two sources"]);
    dir
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_requests_are_skipped_reported_and_recorded() {
    let url = stub_server().await;
    let dir = two_file_branch();
    let repo = dir.path().to_path_buf();
    let out = tokio::task::spawn_blocking(move || {
        Command::new(env!("CARGO_BIN_EXE_momus"))
            .args(["review", "--base", "base"])
            .current_dir(&repo)
            .env_remove("TYPESAFE_API_KEY")
            .env("TYPESAFE_BASE_URL", &url)
            .env("MOMUS_REPORT", repo.join("report.json"))
            .env("MOMUS_CONCURRENCY", "1")
            .output()
            .expect("momus runs")
    })
    .await
    .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "exit {:?}: {stderr}", out.status.code());
    assert!(stderr.contains("screen b.rs failed, skipped"), "{stderr}");
    assert!(stderr.contains("skipped 2 failed request(s)"), "{stderr}");

    let report: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dir.path().join("report.json")).unwrap()).unwrap();
    assert_eq!(report["screenedFiles"], 1, "a.rs was screened");
    let skipped: Vec<(String, String)> = report["skipped"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["file"].as_str().unwrap().to_string(), s["stage"].as_str().unwrap().to_string()))
        .collect();
    assert_eq!(
        skipped,
        [("b.rs".to_string(), "screen".to_string()), ("a.rs".to_string(), "profile".to_string())],
        "the failed screen and the failed (non-gating) profile are recorded"
    );
    assert!(report["skipped"][0]["reason"].as_str().unwrap().contains("max_tokens_exceeded"));
}
