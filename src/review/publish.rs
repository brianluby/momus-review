//! `momus github-review`: publishes a saved report to its pull request as
//! one review of inline comments plus one sticky summary comment. Re-runs
//! are idempotent: findings already posted (by fingerprint marker) are
//! skipped and the summary is edited in place.

use anyhow::Result;

use crate::adapters::github::{GitHubApi, NewReview, PullRequest, is_unresolvable_anchor};
use crate::domain::github_review::{SUMMARY_MARKER, plan, posted_fingerprints, summary_body};
use crate::domain::report::{Action, ReviewReport};

/// Inline comments per run unless `--max-comments` says otherwise.
pub const DEFAULT_MAX_COMMENTS: usize = 10;

/// The review event to post. Blocking is normally the `--fail-on-blocking`
/// check status; `RequestChanges` also posts a changes-requested review when
/// a finding blocks, which stays on the PR until someone dismisses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReviewEvent {
    #[default]
    Comment,
    RequestChanges,
}

#[derive(Debug, Clone)]
pub struct PublishOptions {
    pub max_comments: usize,
    pub event: ReviewEvent,
    /// Read from GitHub but write nothing; the outcome carries what would be
    /// posted.
    pub dry_run: bool,
}

impl Default for PublishOptions {
    fn default() -> Self {
        Self { max_comments: DEFAULT_MAX_COMMENTS, event: ReviewEvent::Comment, dry_run: false }
    }
}

/// What happened to the sticky summary comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummaryAction {
    Created,
    Updated,
    Unchanged,
    DryRun,
}

/// The result of one publish.
#[derive(Debug)]
pub struct PublishOutcome {
    /// The review posted (or, in a dry run, the one that would be).
    pub review: Option<NewReview>,
    /// GitHub rejected the review's anchors; its findings went to the summary.
    pub review_rejected: bool,
    pub already_posted: usize,
    pub summary_only: usize,
    pub summary: String,
    pub summary_action: SummaryAction,
}

/// Publishes `report` to `pr`: new inline findings as one review, the rest
/// in the sticky summary comment.
pub async fn publish<A: GitHubApi>(
    api: &A,
    pr: &PullRequest,
    report: &ReviewReport,
    options: &PublishOptions,
) -> Result<PublishOutcome> {
    let files = api.pull_files(pr).await?;
    // Markers count only in bot comments: a PR author could otherwise
    // pre-post a finding's (deterministic) fingerprint to suppress it.
    let comments = api.review_comments(pr).await?;
    let posted =
        posted_fingerprints(comments.iter().filter(|c| c.by_bot()).map(|c| c.body.as_str()));
    let mut plan = plan(report, &files, &posted, options.max_comments);

    let mut review = None;
    let mut review_rejected = false;
    if !plan.inline.is_empty() {
        let blocking = plan.inline.iter().any(|(f, _)| f.action == Action::RequestChanges);
        let event = match options.event {
            ReviewEvent::RequestChanges if blocking => "REQUEST_CHANGES",
            _ => "COMMENT",
        };
        let count = plan.inline.len();
        let new_review = NewReview {
            commit_id: pr.head_sha.clone(),
            event,
            body: format!(
                "momus: {count} new finding{} on this diff; the momus summary comment has the rest.",
                if count == 1 { "" } else { "s" }
            ),
            comments: plan.inline.iter().map(|(_, c)| c.clone()).collect(),
        };
        if options.dry_run {
            review = Some(new_review);
        } else {
            match api.create_review(pr, &new_review).await {
                Ok(()) => review = Some(new_review),
                // An anchor GitHub cannot resolve fails the whole review: the
                // report was made from another commit. Keep the findings by
                // moving them to the summary.
                Err(e) if is_unresolvable_anchor(&e) => {
                    eprintln!("review rejected ({e}); listing its findings in the summary instead");
                    plan.demote_inline();
                    review_rejected = true;
                }
                Err(e) => return Err(e),
            }
        }
    }

    let summary = summary_body(report, &plan, &pr.head_sha);
    // Only a bot's comment is ours to edit: anyone can post the marker, and
    // editing their comment would fail on every run.
    let existing = api
        .issue_comments(pr)
        .await?
        .into_iter()
        .find(|c| c.by_bot() && c.body.starts_with(SUMMARY_MARKER));
    let summary_action = if options.dry_run {
        SummaryAction::DryRun
    } else {
        match existing {
            Some(c) if c.body == summary => SummaryAction::Unchanged,
            Some(c) => {
                api.update_issue_comment(pr, c.id, &summary).await?;
                SummaryAction::Updated
            }
            None => {
                api.create_issue_comment(pr, &summary).await?;
                SummaryAction::Created
            }
        }
    };

    Ok(PublishOutcome {
        review,
        review_rejected,
        already_posted: plan.already_posted,
        summary_only: plan.summary_only.len(),
        summary,
        summary_action,
    })
}
