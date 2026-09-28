//! GitHub REST adapter for `momus github-review`: the pull request context
//! from the Actions environment, and the handful of calls the publisher
//! makes, behind `GitHubApi` so tests can use a fake.

use std::future::Future;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::domain::github_review::{InlineComment, PrFile};

/// The pull request being reviewed: `owner/repo`, number, and head commit.
#[derive(Debug, Clone, PartialEq)]
pub struct PullRequest {
    pub repository: String,
    pub number: u64,
    pub head_sha: String,
}

impl PullRequest {
    /// From `GITHUB_REPOSITORY` and the event payload at `GITHUB_EVENT_PATH`.
    pub fn from_env() -> Result<Self> {
        let repository = std::env::var("GITHUB_REPOSITORY")
            .context("GITHUB_REPOSITORY is not set (run inside GitHub Actions)")?;
        let event_path = std::env::var("GITHUB_EVENT_PATH")
            .context("GITHUB_EVENT_PATH is not set (run inside GitHub Actions)")?;
        let event = std::fs::read_to_string(&event_path)
            .with_context(|| format!("read event payload {event_path}"))?;
        Self::from_event(&repository, &event)
    }

    /// Parses a `pull_request` (or `pull_request_target`) event payload.
    pub fn from_event(repository: &str, event: &str) -> Result<Self> {
        let valid_part = |part: &str| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        };
        match repository.split_once('/') {
            Some((owner, repo)) if valid_part(owner) && valid_part(repo) => {}
            _ => bail!("GITHUB_REPOSITORY must be owner/repo, got '{repository}'"),
        }
        let event: Value = serde_json::from_str(event).context("event payload is not JSON")?;
        let pr = &event["pull_request"];
        let number = pr["number"]
            .as_u64()
            .context("the event has no pull_request (run github-review on pull_request events)")?;
        let head_sha = pr["head"]["sha"]
            .as_str()
            .filter(|sha| sha.len() >= 7 && sha.chars().all(|c| c.is_ascii_hexdigit()))
            .context("the event's pull_request.head.sha is missing or not a commit sha")?
            .to_string();
        Ok(Self { repository: repository.to_string(), number, head_sha })
    }
}

/// A comment on the pull request: an issue (conversation) comment or a
/// review (inline) comment; both list with the same `id`/`body`/`user`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Comment {
    pub id: u64,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub user: Option<CommentUser>,
}

impl Comment {
    /// Written by a bot account (`github-actions[bot]` with `GITHUB_TOKEN`),
    /// so not a human's comment that merely quotes a marker.
    pub fn by_bot(&self) -> bool {
        self.user.as_ref().is_some_and(|u| u.kind == "Bot")
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CommentUser {
    #[serde(rename = "type", default)]
    pub kind: String,
}

/// A review to create: the head commit it applies to, the event
/// (`COMMENT` / `REQUEST_CHANGES`), a body, and its inline comments.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NewReview {
    pub commit_id: String,
    pub event: &'static str,
    pub body: String,
    pub comments: Vec<InlineComment>,
}

/// A non-success response from the GitHub API. Returned inside `anyhow` so
/// callers can downcast it (e.g. to recover from a 422).
#[derive(Debug)]
pub struct ApiStatusError {
    pub status: u16,
    pub message: String,
}

impl std::fmt::Display for ApiStatusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "GitHub API returned {}: {}", self.status, self.message)
    }
}

impl std::error::Error for ApiStatusError {}

/// True when `error` is a GitHub `422 Unprocessable Entity`.
pub fn is_unprocessable(error: &anyhow::Error) -> bool {
    error.downcast_ref::<ApiStatusError>().is_some_and(|e| e.status == 422)
}

/// The GitHub calls the publisher needs, for one pull request.
pub trait GitHubApi {
    /// Every file of the pull request, with its patch when GitHub has one.
    fn pull_files(&self, pr: &PullRequest) -> impl Future<Output = Result<Vec<PrFile>>> + Send;
    /// Every review (inline) comment on the pull request.
    fn review_comments(&self, pr: &PullRequest) -> impl Future<Output = Result<Vec<Comment>>> + Send;
    /// Every issue (conversation) comment on the pull request.
    fn issue_comments(&self, pr: &PullRequest) -> impl Future<Output = Result<Vec<Comment>>> + Send;
    fn create_review(&self, pr: &PullRequest, review: &NewReview) -> impl Future<Output = Result<()>> + Send;
    fn create_issue_comment(&self, pr: &PullRequest, body: &str) -> impl Future<Output = Result<()>> + Send;
    fn update_issue_comment(&self, pr: &PullRequest, id: u64, body: &str) -> impl Future<Output = Result<()>> + Send;
}

/// Page size for list calls (the API maximum).
const PER_PAGE: usize = 100;
/// List calls stop after this many pages (GitHub itself stops listing a
/// pull request's files at 3000).
const MAX_PAGES: usize = 30;

/// A `reqwest` client for the GitHub REST API.
pub struct GitHubClient {
    http: reqwest::Client,
    api_url: String,
    token: String,
}

impl GitHubClient {
    /// Reads `GITHUB_TOKEN` and `GITHUB_API_URL` (default
    /// `https://api.github.com`; set by Actions on GitHub Enterprise Server).
    pub fn from_env() -> Result<Self> {
        let token = std::env::var("GITHUB_TOKEN")
            .ok()
            .filter(|t| !t.trim().is_empty())
            .context("GITHUB_TOKEN is not set (pass secrets.GITHUB_TOKEN to the step)")?;
        let api_url = std::env::var("GITHUB_API_URL")
            .ok()
            .filter(|u| !u.trim().is_empty())
            .unwrap_or_else(|| "https://api.github.com".to_string());
        Self::new(&api_url, &token)
    }

    pub fn new(api_url: &str, token: &str) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(concat!("momus-review/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            http,
            api_url: api_url.trim_end_matches('/').to_string(),
            token: token.to_string(),
        })
    }

    fn repo_url(&self, pr: &PullRequest, path: &str) -> String {
        format!("{}/repos/{}/{path}", self.api_url, pr.repository)
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> Result<reqwest::Response> {
        let resp = request
            .bearer_auth(&self.token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await?;
        let status = resp.status();
        if status.is_success() {
            return Ok(resp);
        }
        let text = resp.text().await.unwrap_or_default();
        let message = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| v["message"].as_str().map(String::from))
            .unwrap_or(text);
        Err(ApiStatusError { status: status.as_u16(), message }.into())
    }

    /// GETs every page of a list endpoint.
    async fn get_all<T: DeserializeOwned>(&self, url: &str) -> Result<Vec<T>> {
        let mut items = Vec::new();
        for page in 1..=MAX_PAGES {
            let request = self.http.get(url).query(&[("per_page", PER_PAGE), ("page", page)]);
            let batch: Vec<T> = self.send(request).await?.json().await?;
            let last = batch.len() < PER_PAGE;
            items.extend(batch);
            if last {
                break;
            }
        }
        Ok(items)
    }
}

impl GitHubApi for GitHubClient {
    async fn pull_files(&self, pr: &PullRequest) -> Result<Vec<PrFile>> {
        self.get_all(&self.repo_url(pr, &format!("pulls/{}/files", pr.number))).await
    }

    async fn review_comments(&self, pr: &PullRequest) -> Result<Vec<Comment>> {
        self.get_all(&self.repo_url(pr, &format!("pulls/{}/comments", pr.number))).await
    }

    async fn issue_comments(&self, pr: &PullRequest) -> Result<Vec<Comment>> {
        self.get_all(&self.repo_url(pr, &format!("issues/{}/comments", pr.number))).await
    }

    async fn create_review(&self, pr: &PullRequest, review: &NewReview) -> Result<()> {
        let url = self.repo_url(pr, &format!("pulls/{}/reviews", pr.number));
        self.send(self.http.post(url).json(review)).await?;
        Ok(())
    }

    async fn create_issue_comment(&self, pr: &PullRequest, body: &str) -> Result<()> {
        let url = self.repo_url(pr, &format!("issues/{}/comments", pr.number));
        self.send(self.http.post(url).json(&json!({ "body": body }))).await?;
        Ok(())
    }

    async fn update_issue_comment(&self, pr: &PullRequest, id: u64, body: &str) -> Result<()> {
        let url = self.repo_url(pr, &format!("issues/comments/{id}"));
        self.send(self.http.patch(url).json(&json!({ "body": body }))).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn pull_request_from_event() {
        let event = format!(r#"{{ "pull_request": {{ "number": 15, "head": {{ "sha": "{SHA}" }} }} }}"#);
        let pr = PullRequest::from_event("brianluby/momus-review", &event).unwrap();
        assert_eq!(pr, PullRequest { repository: "brianluby/momus-review".into(), number: 15, head_sha: SHA.into() });
    }

    #[test]
    fn non_pull_request_events_and_bad_repositories_are_errors() {
        let push = r#"{ "ref": "refs/heads/main" }"#;
        let err = PullRequest::from_event("o/r", push).unwrap_err();
        assert!(err.to_string().contains("pull_request events"), "{err:#}");

        let event = format!(r#"{{ "pull_request": {{ "number": 1, "head": {{ "sha": "{SHA}" }} }} }}"#);
        for bad in ["", "owner", "o/r/x", "../r", "o/..", "o/", "o/r?x=1"] {
            assert!(PullRequest::from_event(bad, &event).is_err(), "accepted '{bad}'");
        }

        let bad_sha = r#"{ "pull_request": { "number": 1, "head": { "sha": "main" } } }"#;
        assert!(PullRequest::from_event("o/r", bad_sha).is_err());
    }

    #[test]
    fn review_serializes_to_the_api_shape() {
        let review = NewReview {
            commit_id: SHA.into(),
            event: "COMMENT",
            body: "b".into(),
            comments: vec![InlineComment { path: "src/a.rs".into(), line: 3, side: "RIGHT", body: "c".into() }],
        };
        assert_eq!(
            serde_json::to_value(&review).unwrap(),
            json!({
                "commit_id": SHA,
                "event": "COMMENT",
                "body": "b",
                "comments": [{ "path": "src/a.rs", "line": 3, "side": "RIGHT", "body": "c" }],
            })
        );
    }

    /// A stub GitHub: 101 files over two pages, a 422 on review creation,
    /// and a record of the auth header each request carried.
    async fn stub_github() -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        use axum::extract::Query;
        use axum::http::{HeaderMap, StatusCode};
        use axum::routing::{get, post};
        use axum::{Json, Router};
        use std::collections::HashMap;
        use std::sync::{Arc, Mutex};

        let auth = Arc::new(Mutex::new(Vec::new()));
        let seen = auth.clone();
        let app = Router::new()
            .route(
                "/repos/o/r/pulls/7/files",
                get(move |headers: HeaderMap, Query(q): Query<HashMap<String, String>>| {
                    let seen = seen.clone();
                    async move {
                        let header = |name: &str| {
                            headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or_default().to_string()
                        };
                        seen.lock().unwrap().push(format!("{} {}", header("authorization"), header("accept")));
                        let (start, count) = match q.get("page").map(String::as_str) {
                            Some("1") => (0, 100),
                            _ => (100, 1),
                        };
                        let files: Vec<Value> = (start..start + count)
                            .map(|i| json!({ "filename": format!("f{i}.rs"), "patch": "@@ -0,0 +1 @@\n+x" }))
                            .collect();
                        Json(Value::Array(files))
                    }
                }),
            )
            .route(
                "/repos/o/r/pulls/7/reviews",
                post(|| async {
                    (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({ "message": "Line could not be resolved" })))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (url, auth)
    }

    #[tokio::test]
    async fn client_pages_lists_and_surfaces_api_errors() {
        let (url, auth) = stub_github().await;
        let client = GitHubClient::new(&url, "t0ken").unwrap();
        let pr = PullRequest { repository: "o/r".into(), number: 7, head_sha: SHA.into() };

        let files = client.pull_files(&pr).await.unwrap();
        assert_eq!(files.len(), 101);
        assert_eq!(files[100].filename, "f100.rs");
        assert_eq!(files[0].patch.as_deref(), Some("@@ -0,0 +1 @@\n+x"));
        let auth = auth.lock().unwrap().clone();
        assert_eq!(auth, vec!["Bearer t0ken application/vnd.github+json"; 2]);

        let review = NewReview { commit_id: SHA.into(), event: "COMMENT", body: "b".into(), comments: vec![] };
        let err = client.create_review(&pr, &review).await.unwrap_err();
        assert!(is_unprocessable(&err), "{err:#}");
        assert_eq!(err.to_string(), "GitHub API returned 422: Line could not be resolved");
    }

    #[test]
    fn only_a_422_is_unprocessable() {
        let err: anyhow::Error = ApiStatusError { status: 422, message: "Line could not be resolved".into() }.into();
        assert!(is_unprocessable(&err));
        let err: anyhow::Error = ApiStatusError { status: 403, message: "forbidden".into() }.into();
        assert!(!is_unprocessable(&err));
        assert!(!is_unprocessable(&anyhow::anyhow!("other")));
    }
}
