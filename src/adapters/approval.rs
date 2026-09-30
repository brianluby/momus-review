//! Automatic approval uses live GitHub head, full file inventory and checks.
use super::github::{GitHubApi, GitHubClient, NewReview, PullRequest};
use crate::{
    domain::report::ReviewReport,
    review::merge_confidence::{CheckEvidence, NamedCheck},
};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::collections::BTreeSet;

// GitHub REST timestamps are UTC RFC3339 seconds. Anything else is unknown.
fn timestamp(value: &Value) -> i64 {
    let Some(text) = value.as_str() else { return 0 };
    if !text.is_ascii()
        || text.len() != 20
        || &text[4..5] != "-"
        || &text[7..8] != "-"
        || &text[10..11] != "T"
        || &text[13..14] != ":"
        || &text[16..17] != ":"
        || &text[19..20] != "Z"
    {
        return 0;
    }
    let part = |a, b| text[a..b].parse::<i64>().ok();
    let (Some(mut y), Some(m), Some(d), Some(h), Some(min), Some(sec)) = (
        part(0, 4),
        part(5, 7),
        part(8, 10),
        part(11, 13),
        part(14, 16),
        part(17, 19),
    ) else {
        return 0;
    };
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if !text
        .chars()
        .enumerate()
        .all(|(i, c)| matches!(i, 4 | 7 | 10 | 13 | 16 | 19) || c.is_ascii_digit())
        || !(1970..=9999).contains(&y)
        || !(1..=12).contains(&m)
        || d < 1
        || d > days[(m - 1) as usize]
        || h > 23
        || min > 59
        || sec > 59
    {
        return 0;
    }
    y -= i64::from(m <= 2);
    let era = y / 400;
    let year = y - era * 400;
    let mp = m + if m > 2 { -3 } else { 9 };
    let day = (153 * mp + 2) / 5 + d - 1;
    let epoch_days = era * 146097 + year * 365 + year / 4 - year / 100 + day - 719468;
    epoch_days * 86400 + h * 3600 + min * 60 + sec
}

impl GitHubClient {
    async fn current_pull(&self, pr: &PullRequest) -> Result<Value> {
        let url = self.repo_url(pr, &format!("pulls/{}", pr.number));
        let current: Value = self.send(self.http.get(url)).await?.json().await?;
        ensure!(
            current["head"]["sha"].as_str() == Some(pr.head_sha.as_str()),
            "PR head changed; approval refused"
        );
        ensure!(
            current["state"] == "open" && current["draft"] == false,
            "automatic approval requires an open, non-draft PR"
        );
        Ok(current)
    }
    pub async fn approval_evidence(
        &self,
        pr: &PullRequest,
        report: &ReviewReport,
    ) -> Result<CheckEvidence> {
        let current = self.current_pull(pr).await?;
        let base = current["base"]["sha"]
            .as_str()
            .context("PR base is missing")?;
        let compare = self.repo_url(pr, &format!("compare/{base}...{}", pr.head_sha));
        let comparison: Value = self.send(self.http.get(compare)).await?.json().await?;
        let merge_base = comparison["merge_base_commit"]["sha"]
            .as_str()
            .context("merge base unavailable")?;
        let files = self.pull_files(pr).await?;
        let mut unknowns = Vec::new();
        if current["changed_files"].as_u64() != Some(files.len() as u64) {
            unknowns.push("PR file inventory is missing or truncated".into());
        }
        if report.reviewed_base.as_deref() != Some(merge_base) {
            unknowns.push("Report merge base does not match current PR merge base".into());
        }
        let covered: BTreeSet<_> = report.matrix.iter().map(|r| r.file.as_str()).collect();
        // Complete ordinary source screening is required. Auxiliary reports do
        // not imply coverage of arbitrary configuration/deletions/test-only files.
        for file in &files {
            if file
                .patch
                .as_ref()
                .is_none_or(|patch| !patch.contains("@@ "))
            {
                unknowns.push(format!(
                    "{}: binary, absent or unsupported patch evidence",
                    file.filename
                ));
            }
            if !covered.contains(file.filename.as_str()) {
                unknowns.push(format!(
                    "{}: changed input has no complete source screen",
                    file.filename
                ));
            }
        }
        if files.len() != covered.len() {
            unknowns.push("PR inventory and reviewed inventory differ".into());
        }
        let mut checks = Vec::new();
        for (endpoint, array, total) in [
            ("check-runs", "check_runs", "total_count"),
            ("status", "statuses", "total_count"),
        ] {
            let url = self.repo_url(pr, &format!("commits/{}/{endpoint}", pr.head_sha));
            let batch: Value = self
                .send(self.http.get(&url).query(&[("per_page", 100)]))
                .await?
                .json()
                .await?;
            let values = batch[array].as_array().context("missing check inventory")?;
            ensure!(
                batch[total].as_u64() == Some(values.len() as u64),
                "check inventory truncated; approval refused"
            );
            if endpoint == "status" {
                ensure!(
                    batch["sha"].as_str() == Some(pr.head_sha.as_str()),
                    "status head mismatched"
                );
            }
            for item in values {
                let (name, head, passed, completed, evidence) = if endpoint == "check-runs" {
                    (
                        item["name"].as_str(),
                        item["head_sha"].as_str(),
                        item["status"] == "completed" && item["conclusion"] == "success",
                        timestamp(&item["completed_at"]),
                        item["html_url"].as_str(),
                    )
                } else {
                    (
                        item["context"].as_str(),
                        Some(pr.head_sha.as_str()),
                        item["state"] == "success",
                        timestamp(&item["updated_at"]),
                        item["url"].as_str(),
                    )
                };
                checks.push(NamedCheck {
                    name: name.unwrap_or_default().into(),
                    head: head.unwrap_or_default().into(),
                    status: if passed { "passed" } else { "unknown" }.into(),
                    completed_at: completed,
                    evidence: evidence.unwrap_or_default().into(),
                });
            }
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64;
        Ok(CheckEvidence {
            repository: pr.repository.clone(),
            head: pr.head_sha.clone(),
            reviewed_head: report.reviewed_head.clone().unwrap_or_default(),
            assessed_at: now,
            review_complete: unknowns.is_empty(),
            evidence_complete: unknowns.is_empty(),
            supported_inputs: unknowns.is_empty(),
            auxiliary_unknowns: unknowns,
            checks,
        })
    }
    /// Bind approval to the exact reviewed commit after checking the head again.
    pub async fn approve_current_head(
        &self,
        pr: &PullRequest,
        report: &ReviewReport,
        policy: &crate::review::merge_confidence::ApprovalPolicy,
        history: Option<&crate::review::merge_confidence::OutcomeHistory>,
    ) -> Result<()> {
        let before = self.current_pull(pr).await?;
        let checks = self.approval_evidence(pr, report).await?;
        let assessment =
            crate::review::merge_confidence::assess(report, history, Some(policy), Some(&checks))?;
        ensure!(
            assessment.approval.eligible,
            "automatic approval rejected on fresh evidence: {}",
            assessment.approval.reasons.join("; ")
        );
        let after = self.current_pull(pr).await?;
        ensure!(
            before["base"]["sha"] == after["base"]["sha"],
            "PR base changed during approval assessment"
        );
        self.create_review(pr,&NewReview {commit_id:pr.head_sha.clone(),event:"APPROVE",body:"momus: explicit opt-in policy passed complete current-head review, required checks and observed-outcome evaluation.".into(),comments:Vec::new()}).await
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct Fixture {
        head: String,
        base: String,
        count: u64,
        patch: Option<String>,
        check: String,
        posts: usize,
    }
    async fn fixture() -> (
        GitHubClient,
        PullRequest,
        std::sync::Arc<std::sync::Mutex<Fixture>>,
    ) {
        use axum::{
            Json, Router,
            extract::State,
            routing::{get, post},
        };
        use std::sync::{Arc, Mutex};
        let state = Arc::new(Mutex::new(Fixture {
            head: "candidate".into(),
            base: "base".into(),
            count: 1,
            patch: Some("@@ -1 +1 @@\n-a\n+b".into()),
            check: "success".into(),
            posts: 0,
        }));
        let app=Router::new()
            .route("/repos/test/repo/pulls/1",get(|State(s):State<Arc<Mutex<Fixture>>>|async move {let s=s.lock().unwrap();Json(serde_json::json!({"head":{"sha":s.head},"base":{"sha":s.base},"state":"open","draft":false,"changed_files":s.count}))}))
            .route("/repos/test/repo/compare/base...candidate",get(||async {Json(serde_json::json!({"merge_base_commit":{"sha":"base"}}))}))
            .route("/repos/test/repo/pulls/1/files",get(|State(s):State<Arc<Mutex<Fixture>>>|async move {let s=s.lock().unwrap();Json(serde_json::json!([{"filename":"src/main.rs","patch":s.patch}]))}))
            .route("/repos/test/repo/commits/candidate/check-runs",get(|State(s):State<Arc<Mutex<Fixture>>>|async move {let s=s.lock().unwrap();Json(serde_json::json!({"total_count":1,"check_runs":[{"name":"ci","head_sha":"candidate","status":"completed","conclusion":s.check,"completed_at":"2026-09-30T00:00:00Z","html_url":"https://example.test/ci"}]}))}))
            .route("/repos/test/repo/commits/candidate/status",get(||async {Json(serde_json::json!({"sha":"candidate","total_count":0,"statuses":[]}))}))
            .route("/repos/test/repo/pulls/1/reviews",post(|State(s):State<Arc<Mutex<Fixture>>>,Json(body):Json<Value>|async move {assert_eq!(body["event"],"APPROVE");assert_eq!(body["commit_id"],"candidate");s.lock().unwrap().posts+=1;Json(serde_json::json!({}))}))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (
            GitHubClient::new(&url, "fixture-token").unwrap(),
            PullRequest {
                repository: "test/repo".into(),
                number: 1,
                head_sha: "candidate".into(),
            },
            state,
        )
    }
    fn inputs() -> (
        ReviewReport,
        crate::review::merge_confidence::OutcomeHistory,
        crate::review::merge_confidence::ApprovalPolicy,
    ) {
        let mut report: ReviewReport = serde_json::from_str(include_str!(
            "../../examples/merge-confidence/routine-report.json"
        ))
        .unwrap();
        report.reviewed_head = Some("candidate".into());
        report.reviewed_base = Some("base".into());
        report.reviewed_clean = true;
        let mut history: crate::review::merge_confidence::OutcomeHistory = serde_json::from_str(
            include_str!("../../examples/merge-confidence/synthetic-history.json"),
        )
        .unwrap();
        // Fabricated observed-source contract for a stub API test only; this
        // is never real-world calibration or a deployable history export.
        history.synthetic = false;
        history.repository = "test/repo".into();
        let policy = crate::review::merge_confidence::ApprovalPolicy {
            enabled: true,
            required_checks: vec!["ci".into()],
            max_revert_probability: 0.3,
            max_incident_probability: 0.3,
            max_flake_probability: 0.3,
            max_check_age_seconds: 10_000_000_000,
            max_history_age_seconds: 10_000_000_000,
            ..Default::default()
        };
        (report, history, policy)
    }
    #[tokio::test]
    async fn live_evidence_and_approval_fail_closed_on_changes() {
        let (client, pr, state) = fixture().await;
        let (report, history, policy) = inputs();
        let evidence = client.approval_evidence(&pr, &report).await.unwrap();
        assert!(evidence.review_complete);
        client
            .approve_current_head(&pr, &report, &policy, Some(&history))
            .await
            .unwrap();
        assert_eq!(state.lock().unwrap().posts, 1);
        state.lock().unwrap().check = "failure".into();
        assert!(
            client
                .approve_current_head(&pr, &report, &policy, Some(&history))
                .await
                .is_err()
        );
        assert_eq!(state.lock().unwrap().posts, 1);
        state.lock().unwrap().check = "success".into();
        state.lock().unwrap().count = 3001;
        assert!(
            !client
                .approval_evidence(&pr, &report)
                .await
                .unwrap()
                .review_complete
        );
        assert!(
            client
                .approve_current_head(&pr, &report, &policy, Some(&history))
                .await
                .is_err()
        );
        state.lock().unwrap().count = 1;
        state.lock().unwrap().patch = None;
        assert!(
            !client
                .approval_evidence(&pr, &report)
                .await
                .unwrap()
                .supported_inputs
        );
        state.lock().unwrap().head = "new-head".into();
        assert!(client.approval_evidence(&pr, &report).await.is_err());
        assert_eq!(state.lock().unwrap().posts, 1);
    }
    #[test]
    fn utc_timestamp_validation() {
        assert_eq!(timestamp(&serde_json::json!("1970-01-01T00:00:00Z")), 0);
        assert_eq!(
            timestamp(&serde_json::json!("2024-02-29T00:00:00Z")),
            1709164800
        );
        assert_eq!(timestamp(&serde_json::json!("2023-02-29T00:00:00Z")), 0);
        for text in [
            "2024-02-29T25:00:00Z",
            "2024-02-29T00:60:00Z",
            "not a timestamp",
            "2024-02-29T-1:00:00Z",
            "2024-02-29T00:-1:00Z",
            "2024-02-29T00:00:-1Z",
            "2024-02-29T00:00:00+00:00",
        ] {
            assert_eq!(timestamp(&serde_json::json!(text)), 0);
        }
    }
}
