//! `momus github-review` publishing against a fake GitHub: inline anchoring,
//! idempotent re-runs, the sticky summary, and recovery from a rejected
//! review.

use std::sync::Mutex;

use anyhow::Result;
use momus_review::adapters::github::{
    ApiStatusError, CommentUser, GitHubApi, Comment, NewReview, PullRequest,
};
use momus_review::domain::github_review::{PrFile, SUMMARY_MARKER};
use momus_review::domain::policy::Dimension;
use momus_review::domain::report::{Action, Finding, ReviewReport};
use momus_review::review::publish::{PublishOptions, ReviewEvent, SummaryAction, publish};

#[derive(Default)]
struct State {
    review_comments: Vec<Comment>,
    issue_comments: Vec<Comment>,
    reviews: Vec<NewReview>,
    next_id: u64,
}

/// An in-memory pull request: posted reviews become review comments, so a
/// second publish sees the first one's markers.
#[derive(Default)]
struct FakeGitHub {
    files: Vec<PrFile>,
    reject_reviews: bool,
    /// The 422 `errors` detail a rejected review carries.
    reject_with: &'static str,
    state: Mutex<State>,
}

impl FakeGitHub {
    fn with_files(files: Vec<PrFile>) -> Self {
        Self { files, ..Default::default() }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }
}

impl GitHubApi for FakeGitHub {
    async fn pull_files(&self, _: &PullRequest) -> Result<Vec<PrFile>> {
        Ok(self.files.clone())
    }

    async fn review_comments(&self, _: &PullRequest) -> Result<Vec<Comment>> {
        Ok(self.state().review_comments.clone())
    }

    async fn issue_comments(&self, _: &PullRequest) -> Result<Vec<Comment>> {
        Ok(self.state().issue_comments.clone())
    }

    async fn create_review(&self, _: &PullRequest, review: &NewReview) -> Result<()> {
        if self.reject_reviews {
            let errors = vec![self.reject_with.to_string()];
            return Err(ApiStatusError { status: 422, message: "Unprocessable Entity".into(), errors }.into());
        }
        let mut state = self.state();
        let posted: Vec<Comment> = review.comments.iter().map(|c| bot_comment(0, &c.body)).collect();
        state.review_comments.extend(posted);
        state.reviews.push(review.clone());
        Ok(())
    }

    async fn create_issue_comment(&self, _: &PullRequest, body: &str) -> Result<()> {
        let mut state = self.state();
        state.next_id += 1;
        let id = state.next_id;
        state.issue_comments.push(bot_comment(id, body));
        Ok(())
    }

    async fn update_issue_comment(&self, _: &PullRequest, id: u64, body: &str) -> Result<()> {
        let mut state = self.state();
        let comment = state.issue_comments.iter_mut().find(|c| c.id == id).expect("known comment");
        comment.body = body.to_string();
        Ok(())
    }
}

fn comment(id: u64, body: &str, kind: &str) -> Comment {
    Comment { id, body: body.to_string(), user: Some(CommentUser { kind: kind.into() }) }
}

/// A comment as `github-actions[bot]` posts it.
fn bot_comment(id: u64, body: &str) -> Comment {
    comment(id, body, "Bot")
}

fn pr() -> PullRequest {
    PullRequest {
        repository: "o/r".into(),
        number: 7,
        head_sha: "0123456789abcdef0123456789abcdef01234567".into(),
    }
}

fn finding(file: &str, line: usize, fingerprint: &str, action: Action) -> Finding {
    Finding {
        file: file.into(),
        line,
        dimension: Dimension::Correctness,
        severity: if action == Action::RequestChanges { 2.5 } else { 1.0 },
        action,
        fingerprint: fingerprint.into(),
        title: Some(format!("Finding {fingerprint}")),
        ..Default::default()
    }
}

/// `src/a.rs` lines 1–3 are in the diff (all added).
fn files() -> Vec<PrFile> {
    vec![PrFile { filename: "src/a.rs".into(), patch: Some("@@ -0,0 +1,3 @@\n+a\n+b\n+c".into()) }]
}

fn report() -> ReviewReport {
    ReviewReport {
        findings: vec![
            finding("src/a.rs", 2, "aaaa", Action::RequestChanges),
            finding("src/a.rs", 40, "bbbb", Action::Comment),
        ],
        ..Default::default()
    }
}

#[tokio::test]
async fn first_run_posts_inline_comments_and_a_summary() {
    let github = FakeGitHub::with_files(files());
    let outcome = publish(&github, &pr(), &report(), &PublishOptions::default()).await.unwrap();

    let state = github.state();
    assert_eq!(state.reviews.len(), 1);
    let review = &state.reviews[0];
    assert_eq!(review.event, "COMMENT", "blocking is the check status by default");
    assert_eq!(review.commit_id, pr().head_sha);
    assert_eq!(review.comments.len(), 1);
    assert_eq!((review.comments[0].path.as_str(), review.comments[0].line), ("src/a.rs", 2));
    assert!(review.comments[0].body.ends_with("<!-- momus:fp=aaaa -->"));

    assert_eq!(outcome.summary_action, SummaryAction::Created);
    assert_eq!(outcome.summary_only, 1);
    assert_eq!(state.issue_comments.len(), 1);
    let summary = &state.issue_comments[0].body;
    assert!(summary.starts_with(SUMMARY_MARKER));
    assert!(summary.contains("`src/a.rs:40` · outside the diff"), "{summary}");
}

#[tokio::test]
async fn rerun_skips_posted_findings_and_edits_the_summary_in_place() {
    let github = FakeGitHub::with_files(files());
    let options = PublishOptions::default();
    publish(&github, &pr(), &report(), &options).await.unwrap();

    // Same report: nothing new to post, and the summary is unchanged.
    let outcome = publish(&github, &pr(), &report(), &options).await.unwrap();
    assert!(outcome.review.is_none());
    assert_eq!(outcome.already_posted, 1);
    assert_eq!(outcome.summary_action, SummaryAction::Unchanged);

    // A new finding on a later push: one new comment, the summary is edited.
    let mut next = report();
    next.findings.push(finding("src/a.rs", 3, "cccc", Action::Comment));
    let outcome = publish(&github, &pr(), &next, &options).await.unwrap();
    assert_eq!(outcome.summary_action, SummaryAction::Updated);

    let state = github.state();
    assert_eq!(state.reviews.len(), 2);
    assert_eq!(state.reviews[1].comments.len(), 1);
    assert!(state.reviews[1].comments[0].body.contains("momus:fp=cccc"));
    assert_eq!(state.issue_comments.len(), 1, "one sticky summary");
    assert!(state.issue_comments[0].body.contains("2 posted inline · 1 in this summary"));
}

#[tokio::test]
async fn a_human_cannot_suppress_a_finding_with_its_marker() {
    let github = FakeGitHub::with_files(files());
    github.state().review_comments.push(comment(5, "<!-- momus:fp=aaaa -->", "User"));

    let outcome = publish(&github, &pr(), &report(), &PublishOptions::default()).await.unwrap();
    assert_eq!(outcome.already_posted, 0);
    let comments = outcome.review.unwrap().comments;
    assert_eq!(comments.len(), 1);
    assert!(comments[0].body.contains("momus:fp=aaaa"));
}

#[tokio::test]
async fn a_human_comment_with_the_marker_is_not_the_summary() {
    let github = FakeGitHub::with_files(files());
    github.state().issue_comments.push(comment(99, &format!("{SUMMARY_MARKER}\nplanted"), "User"));
    github.state().next_id = 99;

    let outcome = publish(&github, &pr(), &report(), &PublishOptions::default()).await.unwrap();
    assert_eq!(outcome.summary_action, SummaryAction::Created);
    let state = github.state();
    assert_eq!(state.issue_comments.len(), 2);
    assert!(state.issue_comments[0].body.ends_with("planted"), "the human's comment is untouched");
}

#[tokio::test]
async fn request_changes_event_is_opt_in_and_needs_a_blocking_finding() {
    let options = PublishOptions { event: ReviewEvent::RequestChanges, ..Default::default() };

    let github = FakeGitHub::with_files(files());
    publish(&github, &pr(), &report(), &options).await.unwrap();
    assert_eq!(github.state().reviews[0].event, "REQUEST_CHANGES");

    let mut minor = report();
    minor.findings[0].action = Action::Comment;
    let github = FakeGitHub::with_files(files());
    publish(&github, &pr(), &minor, &options).await.unwrap();
    assert_eq!(github.state().reviews[0].event, "COMMENT");
}

#[tokio::test]
async fn a_rejected_review_moves_its_findings_to_the_summary() {
    let github = FakeGitHub {
        files: files(),
        reject_reviews: true,
        reject_with: "Line could not be resolved",
        ..Default::default()
    };
    let outcome = publish(&github, &pr(), &report(), &PublishOptions::default()).await.unwrap();

    assert!(outcome.review_rejected);
    assert!(outcome.review.is_none());
    assert_eq!(outcome.summary_only, 2);
    let state = github.state();
    assert!(state.reviews.is_empty());
    assert!(state.issue_comments[0].body.contains("`src/a.rs:2` · GitHub rejected the anchor"));
}

#[tokio::test]
async fn another_422_is_an_error_not_a_demotion() {
    let github = FakeGitHub {
        files: files(),
        reject_reviews: true,
        reject_with: "Can not request changes on your own pull request",
        ..Default::default()
    };
    let err = publish(&github, &pr(), &report(), &PublishOptions::default()).await.unwrap_err();
    assert!(err.to_string().contains("own pull request"), "{err:#}");
    assert!(github.state().issue_comments.is_empty(), "no summary claims the review was posted");
}

#[tokio::test]
async fn inline_comments_are_capped() {
    let report = ReviewReport {
        findings: (1..=3).map(|line| finding("src/a.rs", line, &format!("f{line}"), Action::Comment)).collect(),
        ..Default::default()
    };
    let github = FakeGitHub::with_files(files());
    let options = PublishOptions { max_comments: 2, ..Default::default() };
    let outcome = publish(&github, &pr(), &report, &options).await.unwrap();
    assert_eq!(outcome.review.unwrap().comments.len(), 2);
    assert_eq!(outcome.summary_only, 1);
    assert!(outcome.summary.contains("over the inline cap"));
}

#[tokio::test]
async fn dry_run_writes_nothing() {
    let github = FakeGitHub::with_files(files());
    let options = PublishOptions { dry_run: true, ..Default::default() };
    let outcome = publish(&github, &pr(), &report(), &options).await.unwrap();

    assert_eq!(outcome.review.unwrap().comments.len(), 1);
    assert_eq!(outcome.summary_action, SummaryAction::DryRun);
    let state = github.state();
    assert!(state.reviews.is_empty());
    assert!(state.issue_comments.is_empty());
}
