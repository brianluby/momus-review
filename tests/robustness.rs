//! Exercise the three opt-in features together through real CLI/HTTP/cache boundaries.

use axum::{Json, Router, routing::post};
use serde_json::{Value, json};
use std::path::Path;
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

type Requests = Arc<Mutex<Vec<Value>>>;

/// Commit a minimal changed behavior and a related test, keeping output outside discovery.
fn repository() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    git(repo, &["init", "-q"]);
    git(repo, &["config", "user.email", "tests@example.invalid"]);
    git(repo, &["config", "user.name", "Tests"]);
    std::fs::create_dir(repo.join("src")).unwrap();
    std::fs::create_dir(repo.join("tests")).unwrap();
    std::fs::write(repo.join(".gitignore"), "reviews/\nreport.json\n").unwrap();
    std::fs::write(repo.join("src/answer.rs"), "pub fn answer() -> u32 { 7 }\n").unwrap();
    git(repo, &["add", "."]);
    git(repo, &["commit", "-qm", "base"]);
    git(repo, &["tag", "base"]);
    std::fs::write(repo.join("src/answer.rs"), "pub fn answer() -> u32 { 9 }\n").unwrap();
    std::fs::write(
        repo.join("tests/answer_test.rs"),
        "#[test]\nfn answer_has_a_signature() { let _producer = answer; }\n",
    )
    .unwrap();
    git(repo, &["add", "."]);
    git(repo, &["commit", "-qm", "changed answer"]);
    dir
}

/// Git setup remains local to each test fixture.
fn git(repo: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Answer narrow judgments deterministically; capture the entire redacted wire request.
async fn server(
    two_signals: bool,
    spec_status: &'static str,
    planner_confidence: f64,
) -> (String, Requests) {
    let seen: Requests = Arc::default();
    let sink = seen.clone();
    let app = Router::new().route("/v1/systemone", post(move |Json(body): Json<Value>| {
        let sink = sink.clone();
        async move {
            sink.lock().unwrap().push(body.clone());
            let mut answers = serde_json::Map::new();
            for (id, question) in body["questions"].as_object().unwrap() {
                let answer = match question["type"].as_str().unwrap() {
                    "noul" => json!({"noul": match id.as_str() {
                        "testGap" | "supported" | "contradiction" => 0.96,
                        "security" if two_signals => 0.8,
                        _ => 0.1,
                    }}),
                    "score" => json!({"score":1.0,"confidence":0.96}),
                    "choice" => {
                        let criteria = question["criteria"].as_object().unwrap();
                        let label = match id.as_str() {
                            "assessment" => spec_status,
                            "requirement" => "S2",
                            "source" => if body["state"]["mode"] == "changes" { "hunk_1" } else { "R1" },
                            "testPlanStrategy" => "componentIntegration",
                            "mechanism" => if body["state"]["suspectedConcern"]["dimension"] == "security" { "sqlInjection" } else { "integration" },
                            "suggestedFix" => "addCoverage",
                            "suggestedTest" => "regression",
                            _ => criteria.keys().find(|k| !matches!(k.as_str(), "noMatch" | "noIssue" | "noSuggestion")).unwrap(),
                        };
                        json!({"choice":label,"confidence": if id == "testPlanStrategy" { planner_confidence } else {0.96}})
                    }
                    other => panic!("unexpected question type {other}"),
                };
                answers.insert(id.clone(), answer);
            }
            Json(json!({"model":"stub-robust-v1", "answers":answers, "usage":{"input_tokens":10,"output_tokens":2}}))
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, seen)
}

/// Run the binary with isolated configuration instead of mutating the test process.
fn run(repo: &Path, url: &str, flags: &[&str], scan: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_momus"));
    if scan {
        command.arg("scan");
    } else {
        command.args(["review", "--base", "base"]);
    }
    command
        .args(["--no-refine"])
        .args(flags)
        .current_dir(repo)
        .env("TYPESAFE_BASE_URL", url)
        .env("TYPESAFE_DEFAULT_MODEL", "stub-robust-v1")
        .env("MOMUS_REDACT", "on")
        .env("MOMUS_CONCURRENCY", "1")
        .env("MOMUS_REPORT", repo.join("report.json"))
        .env("MOMUS_CACHE_DIR", repo.join("reviews/cache"))
        .env("MOMUS_INDEX_DIR", repo.join("reviews/index"))
        .env("MOMUS_FEEDBACK", repo.join("reviews/feedback.json"))
        .env_remove("MOMUS_CONTEXT_BUDGET_CHARS")
        .env_remove("TYPESAFE_API_KEY");
    command.output().unwrap()
}

/// Parse stdout as the real user-facing report, asserting successful execution first.
fn report(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

/// All flags interact correctly, specs are redacted, and zero-budget cached replay completes.
#[tokio::test(flavor = "multi_thread")]
async fn combined_features_redact_cache_and_invalidate_changed_spec() {
    let dir = repository();
    let contract = dir.path().join("contract.md");
    std::fs::write(
        &contract,
        "# Contract\nThe answer must be seven. Credential AKIAIOSFODNN7EXAMPLE\n",
    )
    .unwrap();
    let (url, seen) = server(false, "drift", 0.96).await;
    let run_case = |budget: &'static str| {
        let repo = dir.path().to_path_buf();
        let url = url.clone();
        tokio::task::spawn_blocking(move || {
            report(run(
                &repo,
                &url,
                &[
                    "--test-plans",
                    "--spec",
                    "contract.md",
                    "--follow-up-strategy",
                    "voi",
                    "--budget",
                    budget,
                ],
                false,
            ))
        })
    };
    let cold = run_case("calls=9").await.unwrap();
    assert_eq!(cold["usage"]["calls"], 9);
    assert_eq!(cold["budget"]["reserved"], 9);
    assert_eq!(cold["partial"], false);
    assert_eq!(cold["profiles"], json!([]));
    assert_eq!(
        cold["findings"][0]["testPlan"]["strategy"],
        "componentIntegration"
    );
    assert_eq!(cold["findings"][0]["testPlan"]["incomplete"], true);
    assert!(
        cold["findings"][0]["testPlan"]["assertion"]
            .as_str()
            .unwrap()
            .contains("consumer")
    );
    assert_eq!(cold["specDrift"]["checks"][0]["status"], "drift");
    assert_eq!(cold["specDrift"]["checks"][0]["spec"]["startLine"], 2);
    assert!(
        cold["specDrift"]["checks"][0]["source"]["text"]
            .as_str()
            .unwrap()
            .contains("{ 9 }")
    );
    let warm = run_case("calls=0").await.unwrap();
    assert_eq!(warm["usage"]["calls"], 0);
    assert_eq!(warm["usage"]["cache"]["hits"], 9);
    assert_eq!(warm["partial"], false);
    assert_eq!(cold["findings"], warm["findings"]);
    assert_eq!(cold["specDrift"], warm["specDrift"]);
    let requests = seen.lock().unwrap().clone();
    assert_eq!(requests.len(), 9);
    let wire = serde_json::to_string(&requests).unwrap();
    assert!(!wire.contains("AKIAIOSFODNN7EXAMPLE"));
    assert!(wire.contains("redacted:aws-access-key"));
    let planner = requests
        .iter()
        .find(|b| b["questions"]["testPlanStrategy"].is_object())
        .unwrap();
    assert!(
        planner["state"]["context"]["relatedTests"]
            .to_string()
            .contains("answer_has_a_signature")
    );
    assert!(
        planner["state"]["context"]["fileContext"]
            .to_string()
            .contains("answer")
    );
    let old_hash = cold["specDrift"]["documents"][0]["contentHash"].clone();
    std::fs::write(contract, "# Contract\nThe answer must be eight.\n").unwrap();
    let changed = run_case("calls=2").await.unwrap();
    assert_eq!(changed["usage"]["calls"], 2);
    assert_ne!(
        changed["specDrift"]["documents"][0]["contentHash"],
        old_hash
    );
}

/// A finite budget completes the most consequential pipeline instead of profiling.
#[tokio::test(flavor = "multi_thread")]
async fn voi_prioritizes_security_and_marks_budget_deferred_spec() {
    let dir = repository();
    std::fs::write(
        dir.path().join("contract.md"),
        "# Contract\nReturn seven.\n",
    )
    .unwrap();
    let (url, seen) = server(true, "drift", 0.96).await;
    let repo = dir.path().to_path_buf();
    let result = tokio::task::spawn_blocking(move || {
        report(run(
            &repo,
            &url,
            &[
                "--follow-up-strategy",
                "voi",
                "--budget",
                "calls=5",
                "--spec",
                "contract.md",
            ],
            false,
        ))
    })
    .await
    .unwrap();
    assert_eq!(result["budget"]["reserved"], 5);
    assert_eq!(result["findings"].as_array().unwrap().len(), 1);
    assert_eq!(result["findings"][0]["dimension"], "security");
    assert_eq!(
        result["followUpPlan"]["candidates"][0]["dimension"],
        "security"
    );
    assert_eq!(result["specDrift"]["checks"][0]["status"], "deferred");
    assert_eq!(result["partial"], true);
    let requests = seen.lock().unwrap();
    assert_eq!(requests.len(), 5);
    assert!(
        requests
            .iter()
            .all(|b| b["questions"]["category"].is_null())
    );
}

/// Explicit thresholds affect eligibility, and planning abstention preserves findings.
#[tokio::test(flavor = "multi_thread")]
async fn threshold_override_and_low_confidence_planner_in_scan() {
    let dir = repository();
    std::fs::create_dir_all(dir.path().join("reviews")).unwrap();
    let feedback = json!({"entries": (0..5).map(|i| json!({
        "fingerprint":format!("down-{i}"), "file":"src/answer.rs", "dimension":"security",
        "probability":0.8, "vote":"down"
    })).collect::<Vec<_>>()});
    std::fs::write(
        dir.path().join("reviews/feedback.json"),
        serde_json::to_vec(&feedback).unwrap(),
    )
    .unwrap();
    let (url, _) = server(true, "matches", 0.4).await;
    let repo = dir.path().to_path_buf();
    let result = tokio::task::spawn_blocking(move || {
        report(run(
            &repo,
            &url,
            &[
                "--test-plans",
                "--follow-up-strategy",
                "voi",
                "--threshold",
                "security=0.9",
                "--follow-ups",
                "1",
            ],
            true,
        ))
    })
    .await
    .unwrap();
    assert_eq!(result["config"]["screenThresholds"]["security"], 0.9);
    assert_eq!(
        result["followUpPlan"]["candidates"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(result["findings"][0]["dimension"], "testGap");
    assert!(result["findings"][0]["testPlan"].is_null());
}

/// Default execution adds no new judgments; bad input fails before the first HTTP call.
#[tokio::test(flavor = "multi_thread")]
async fn defaults_and_invalid_inputs_have_no_extra_judgments() {
    let dir = repository();
    let (url, seen) = server(false, "drift", 0.96).await;
    let repo = dir.path().to_path_buf();
    let endpoint = url.clone();
    let result = tokio::task::spawn_blocking(move || report(run(&repo, &endpoint, &[], false)))
        .await
        .unwrap();
    assert!(result["specDrift"].is_null() && result["followUpPlan"].is_null());
    assert!(result["findings"][0]["testPlan"].is_null());
    let before = seen.lock().unwrap().len();
    std::fs::write(dir.path().join("contract.md"), "Return seven.\n").unwrap();
    for (flags, expected_reason) in [
        (vec!["--spec", "missing.md"], "missing.md"),
        (vec!["--threshold", "security=NaN"], "must be finite"),
        (
            vec!["--threshold", "security=0.2", "--threshold", "security=0.3"],
            "duplicate threshold override for security",
        ),
        (
            vec!["--shard", "1/2", "--test-plans"],
            "--shard cannot use --test-plans, --spec or VOI",
        ),
        (
            vec!["--shard", "1/2", "--follow-up-strategy", "voi"],
            "--shard cannot use --test-plans, --spec or VOI",
        ),
        (
            vec!["--shard", "1/2", "--spec", "contract.md"],
            "--shard cannot use --test-plans, --spec or VOI",
        ),
        (
            vec!["--tiered", "--spec", "contract.md"],
            "--spec cannot use --tiered",
        ),
    ] {
        let repo = dir.path().to_path_buf();
        let endpoint = url.clone();
        let out = tokio::task::spawn_blocking(move || run(&repo, &endpoint, &flags, true))
            .await
            .unwrap();
        assert!(!out.status.success());
        assert!(
            String::from_utf8_lossy(&out.stderr).contains(expected_reason),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    assert_eq!(seen.lock().unwrap().len(), before);
}

/// Requested contract evidence completes before optional scaffold/suggestion calls.
#[tokio::test(flavor = "multi_thread")]
async fn spec_checks_precede_optional_output_and_zero_followups_are_partial() {
    let dir = repository();
    std::fs::write(
        dir.path().join("contract.md"),
        "# Contract\nReturn seven.\n",
    )
    .unwrap();
    let (url, seen) = server(false, "drift", 0.96).await;
    let repo = dir.path().to_path_buf();
    let endpoint = url.clone();
    let result = tokio::task::spawn_blocking(move || {
        report(run(
            &repo,
            &endpoint,
            &[
                "--test-plans",
                "--spec",
                "contract.md",
                "--follow-up-strategy",
                "voi",
                "--budget",
                "calls=7",
            ],
            false,
        ))
    })
    .await
    .unwrap();
    assert_eq!(result["specDrift"]["checks"][0]["status"], "drift");
    assert!(result["findings"][0]["testPlan"].is_null());
    assert_eq!(result["partial"], true);
    assert_eq!(result["budget"]["reserved"], 7);
    assert!(
        result["skipped"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["stage"] == "testplan")
    );
    let requests = seen.lock().unwrap().clone();
    assert_eq!(requests[5]["questions"]["assessment"]["type"], "choice");
    assert_eq!(requests[6]["questions"]["contradiction"]["type"], "noul");
    let repo = dir.path().to_path_buf();
    let capped = tokio::task::spawn_blocking(move || {
        report(run(&repo, &url, &["--follow-ups", "0"], false))
    })
    .await
    .unwrap();
    assert_eq!(capped["partial"], true);
    assert_eq!(capped["workflow"]["thresholdSignals"], 1);
    assert_eq!(capped["followedSignals"], 0);
    assert!(capped["findings"].as_array().unwrap().is_empty());
}
