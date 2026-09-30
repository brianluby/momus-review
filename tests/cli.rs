//! End-to-end tests of the `momus` binary against a temporary repository.
//! The API is an unreachable loopback address (no key needed there), so a
//! test that reached the network fails instead of calling out, or a local
//! stub server where a test needs answers.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .status()
        .expect("git runs");
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
    fs::write(
        repo.join(".github/workflows/ci.yml"),
        "name: ci\non: push\n",
    )
    .unwrap();
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
    assert!(
        out.status.success(),
        "exit {:?}: {stderr}",
        out.status.code()
    );
    assert!(
        stderr.contains("No changed source files to review"),
        "{stderr}"
    );

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
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": { "error_type": "max_tokens_exceeded" } })),
                );
            }
            let answers: serde_json::Map<String, Value> = questions
                .keys()
                .map(|k| (k.clone(), json!({ "noul": 0.1 })))
                .collect();
            (
                StatusCode::OK,
                Json(json!({ "model": "stub", "answers": answers })),
            )
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
    assert!(
        out.status.success(),
        "exit {:?}: {stderr}",
        out.status.code()
    );
    assert!(stderr.contains("screen b.rs failed, skipped"), "{stderr}");
    assert!(stderr.contains("skipped 2 failed request(s)"), "{stderr}");

    let report: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dir.path().join("report.json")).unwrap()).unwrap();
    assert_eq!(report["screenedFiles"], 1, "a.rs was screened");
    let skipped: Vec<(String, String)> = report["skipped"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["file"].as_str().unwrap().to_string(),
                s["stage"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        skipped,
        [
            ("b.rs".to_string(), "screen".to_string()),
            ("a.rs".to_string(), "profile".to_string())
        ],
        "the failed screen and the failed (non-gating) profile are recorded"
    );
    assert!(
        report["skipped"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("max_tokens_exceeded")
    );
}
/// A stub System One server that records every request body: answers `noul`
/// questions with a low probability, fails non-`noul` ones (profiles).
async fn capturing_stub_server() -> (
    String,
    std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
) {
    use axum::http::StatusCode;
    use axum::routing::post;
    use axum::{Json, Router};
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};

    let bodies: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let seen = bodies.clone();
    let app = Router::new().route(
        "/v1/systemone",
        post(move |Json(body): Json<Value>| {
            let seen = seen.clone();
            async move {
                let questions = body["questions"].as_object().cloned().unwrap_or_default();
                let all_noul = questions.values().all(|q| q["type"] == "noul");
                seen.lock().unwrap().push(body);
                if !all_noul {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({ "detail": { "error_type": "max_tokens_exceeded" } })),
                    );
                }
                let answers: serde_json::Map<String, Value> = questions
                    .keys()
                    .map(|k| (k.clone(), json!({ "noul": 0.1 })))
                    .collect();
                (
                    StatusCode::OK,
                    Json(json!({ "model": "stub", "answers": answers })),
                )
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, bodies)
}

/// A repo whose `feature` branch adds `src/widget.rs` plus `count` changed
/// test files, each `test_lines` assertions wide.
fn widget_branch(count: usize, source_lines: usize, test_lines: usize) -> tempfile::TempDir {
    let dir = config_only_branch();
    let repo = dir.path();
    fs::create_dir_all(repo.join("src")).unwrap();
    fs::create_dir_all(repo.join("tests")).unwrap();
    let source: String = (0..source_lines)
        .map(|i| format!("pub fn widget_{i}() -> u32 {{ {i} }}\n"))
        .collect();
    fs::write(repo.join("src/widget.rs"), source).unwrap();
    for i in 0..count {
        let test: String = (0..test_lines)
            .map(|j| format!("it('widget {j}', () => {{ assert.ok(widget_{j}); }});\n"))
            .collect();
        fs::write(repo.join(format!("tests/widget_{i}.test.ts")), test).unwrap();
    }
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-q", "-m", "widget plus tests"]);
    dir
}

fn run_review(repo: &std::path::Path, url: &str, budget: Option<&str>) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_momus"));
    command
        .args(["review", "--base", "base"])
        .current_dir(repo)
        .env_remove("TYPESAFE_API_KEY")
        .env("TYPESAFE_BASE_URL", url)
        .env("MOMUS_REPORT", repo.join("report.json"))
        .env("MOMUS_CONCURRENCY", "1")
        .env_remove("MOMUS_CONTEXT_BUDGET_CHARS");
    if let Some(budget) = budget {
        command.env("MOMUS_CONTEXT_BUDGET_CHARS", budget);
    }
    command.output().expect("momus runs")
}

/// The screen requests sent for the changed source files (those whose state
/// carries `changedTests`).
fn screen_states(bodies: &[serde_json::Value]) -> Vec<serde_json::Value> {
    bodies
        .iter()
        .filter(|b| !b["state"]["changedTests"].is_null())
        .cloned()
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn hundreds_of_changed_test_files_stay_under_the_context_budget() {
    let (url, bodies) = capturing_stub_server().await;
    // 300 changed test files, ~2.5k characters each: ~750k characters of
    // test context that must never reach a single request (the Juice Shop
    // PR shape). A tight budget proves the bound, not just the selection.
    let dir = widget_branch(300, 10, 50);
    let repo = dir.path().to_path_buf();
    let out = tokio::task::spawn_blocking(move || run_review(&repo, &url, Some("8000")))
        .await
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "exit {:?}: {stderr}",
        out.status.code()
    );

    let bodies = bodies.lock().unwrap().clone();
    let screens = screen_states(&bodies);
    assert!(!screens.is_empty(), "at least one screen request was sent");
    for body in &screens {
        let tests = body["state"]["changedTests"].as_array().unwrap();
        assert!(tests.len() <= 4, "{} changed tests sent", tests.len());
        // The budget bounds context characters; JSON keys, paths, and
        // escaping add a little structure around them.
        let state = serde_json::to_string(&body["state"]).unwrap();
        assert!(
            state.len() <= 8000 + 1000,
            "state was {} chars",
            state.len()
        );
    }

    let report: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dir.path().join("report.json")).unwrap()).unwrap();
    assert_eq!(
        report["screenedFiles"], 1,
        "the one source file was screened"
    );
    assert_eq!(
        report["workflow"]["droppedContextChars"], 0,
        "nothing needed dropping under the tight budget"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn context_over_the_budget_is_trimmed_and_counted() {
    let (url, bodies) = capturing_stub_server().await;
    // One ~130k-character source file against a 5k budget: the patch itself
    // must be trimmed and the drop recorded in the report.
    let dir = widget_branch(0, 5000, 0);
    let repo = dir.path().to_path_buf();
    let out = tokio::task::spawn_blocking(move || run_review(&repo, &url, Some("5000")))
        .await
        .unwrap();
    assert!(out.status.success(), "exit {:?}", out.status.code());

    let bodies = bodies.lock().unwrap().clone();
    for body in &screen_states(&bodies) {
        let state = serde_json::to_string(&body["state"]).unwrap();
        assert!(
            state.len() <= 5000 + 1000,
            "state was {} chars",
            state.len()
        );
    }
    let report: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dir.path().join("report.json")).unwrap()).unwrap();
    assert!(
        report["workflow"]["droppedContextChars"]
            .as_u64()
            .unwrap_or(0)
            > 100_000,
        "the trimmed patch is counted: {}",
        report["workflow"]["droppedContextChars"]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_small_diff_sends_equivalent_context_with_nothing_dropped() {
    let (url, bodies) = capturing_stub_server().await;
    let dir = widget_branch(1, 3, 2);
    let repo = dir.path().to_path_buf();
    let out = tokio::task::spawn_blocking(move || run_review(&repo, &url, None))
        .await
        .unwrap();
    assert!(out.status.success(), "exit {:?}", out.status.code());

    let bodies = bodies.lock().unwrap().clone();
    let screens = screen_states(&bodies);
    assert_eq!(screens.len(), 1, "one source file, one screen request");
    let tests = screens[0]["state"]["changedTests"].as_array().unwrap();
    assert_eq!(tests.len(), 1, "the one related changed test travels");
    assert_eq!(tests[0]["path"], "tests/widget_0.test.ts");
    // Equivalence with the pre-budget behavior: every assertion line of the
    // changed test is still present, in full, untrimmed.
    let patch = tests[0]["patch"].as_str().unwrap();
    for j in 0..2 {
        let line = format!("assert.ok(widget_{j});");
        assert!(patch.contains(&line), "missing {line} in {patch}");
    }
    assert!(!patch.contains("\n...\n"), "the snippet was not truncated");

    let report: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dir.path().join("report.json")).unwrap()).unwrap();
    assert_eq!(report["workflow"]["droppedContextChars"], 0);
    assert_eq!(report["workflow"]["droppedContextItems"], 0);
}

/// Actual CLI reruns prove both request identity and cache accounting across processes.
#[tokio::test(flavor = "multi_thread")]
async fn cold_and_warm_indexes_send_identical_requests_and_cached_reruns_make_no_calls() {
    use axum::{Json, Router, routing::post};
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};
    let bodies: Arc<Mutex<Vec<Value>>> = Arc::default();
    let seen = bodies.clone();
    let app = Router::new().route("/v1/systemone", post(move |Json(body): Json<Value>| {
        let seen = seen.clone();
        async move {
            let answers: serde_json::Map<String, Value> = body["questions"].as_object().unwrap().iter().map(|(id, q)| {
                let answer = match q["type"].as_str().unwrap() {
                    "noul" => json!({ "noul": 0.1 }),
                    "score" => json!({ "score": 1.0, "confidence": 0.9 }),
                    "choice" => json!({ "choice": q["criteria"].as_object().unwrap().keys().next().unwrap(), "confidence": 0.9 }),
                    _ => panic!("unexpected question"),
                };
                (id.clone(), answer)
            }).collect();
            seen.lock().unwrap().push(body);
            Json(json!({ "model": "stub-v1", "answers": answers, "usage": { "input_tokens": 10, "output_tokens": 2 } }))
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    for mode in ["scan", "review"] {
        let dir = widget_branch(2, 3, 2);
        let repo = dir.path().to_path_buf();
        // Include an import neighbor to exercise graph-derived context.
        fs::write(
            repo.join("src/neighbor.rs"),
            "use crate::widget;\npub fn neighbor() {}\n",
        )
        .unwrap();
        git(&repo, &["add", "src/neighbor.rs"]);
        git(&repo, &["commit", "-q", "-m", "neighbor"]);
        let run = |no_cache: bool| {
            let repo = repo.clone();
            let url = url.clone();
            tokio::task::spawn_blocking(move || {
                let mut cmd = Command::new(env!("CARGO_BIN_EXE_momus"));
                cmd.arg(mode).arg("--no-refine");
                if mode == "review" {
                    cmd.args(["--base", "base"]);
                }
                if no_cache {
                    cmd.arg("--no-cache");
                }
                cmd.current_dir(&repo)
                    .env_remove("TYPESAFE_API_KEY")
                    .env("TYPESAFE_BASE_URL", url)
                    .env("TYPESAFE_DEFAULT_MODEL", "stub-v1")
                    .env("MOMUS_CONCURRENCY", "1")
                    .env("MOMUS_REPORT", repo.join("report.json"))
                    .env("MOMUS_CACHE_DIR", repo.join("reviews/cache"))
                    .env("MOMUS_INDEX_DIR", repo.join("reviews/index"))
                    .env_remove("MOMUS_CONTEXT_BUDGET_CHARS");
                let out = cmd.output().unwrap();
                assert!(
                    out.status.success(),
                    "{}",
                    String::from_utf8_lossy(&out.stderr)
                );
                serde_json::from_slice::<Value>(&fs::read(repo.join("report.json")).unwrap())
                    .unwrap()
            })
        };
        bodies.lock().unwrap().clear();
        let cold = run(true).await.unwrap();
        let cold_requests = bodies.lock().unwrap().clone();
        assert!(cold["index"]["computed"].as_u64().unwrap() > 0);
        bodies.lock().unwrap().clear();
        let warm = run(true).await.unwrap();
        assert!(warm["index"]["reused"].as_u64().unwrap() > 0);
        assert_eq!(warm["index"]["computed"], 0);
        assert_eq!(
            cold_requests,
            *bodies.lock().unwrap(),
            "{mode}: index changed the requests"
        );
        // Populate all request units, then restart the CLI with the pinned model.
        run(false).await.unwrap();
        bodies.lock().unwrap().clear();
        let cached = run(false).await.unwrap();
        assert!(
            bodies.lock().unwrap().is_empty(),
            "{mode}: warm cache made HTTP calls"
        );
        assert_eq!(cached["usage"]["calls"], 0);
        assert_eq!(cached["usage"]["inputTokens"], 0);
        assert!(cached["usage"]["cache"]["hits"].as_u64().unwrap() > 0);
        assert_eq!(cached["usage"]["cache"]["misses"], 0);
        assert_eq!(cold["matrix"], cached["matrix"]);
        assert_eq!(cold["profiles"], cached["profiles"]);
        assert_eq!(cold["findings"], cached["findings"]);
        // One changed source should invalidate only its blob metadata, and
        // requests depending on it, while retaining other work units.
        fs::write(repo.join("src/widget.rs"), "pub fn widget_changed() {}\n").unwrap();
        let changed = run(false).await.unwrap();
        assert_eq!(changed["index"]["computed"], 1);
        assert!(changed["usage"]["cache"]["misses"].as_u64().unwrap() > 0);
        assert!(changed["usage"]["cache"]["hits"].as_u64().unwrap() > 0);
    }
}
