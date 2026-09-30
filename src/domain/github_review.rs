//! Pure planning for `momus github-review`: which findings become inline PR
//! review comments, which go to the sticky summary comment, and the text of
//! both. No I/O; `review/publish.rs` drives it over the GitHub adapter.
//!
//! A GitHub review POST fails as a whole (422) when any comment targets a
//! line outside the PR's diff, so every anchor is checked against the
//! commentable RIGHT-side lines of the PR's own patches first. Every
//! published string goes through `domain::redact` (the saved report keeps
//! unredacted evidence); the hidden markers are appended after redaction so
//! a fingerprint is never mistaken for a secret.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::domain::patch::parse_hunks;
use crate::domain::policy::{Dimension, dimension_metadata};
use crate::domain::redact::{Redactions, redact};
use crate::domain::report::{Action, Finding, ReviewMode, ReviewReport};

/// Marks the one sticky summary comment, edited in place on every run.
pub const SUMMARY_MARKER: &str = "<!-- momus:summary -->";

/// At most this many summary-only findings are listed individually.
const SUMMARY_LIST_MAX: usize = 50;

static FINGERPRINT_MARKER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<!-- momus:fp=([0-9a-f]+) -->").expect("valid marker regex"));

/// The hidden marker that makes a posted comment's finding recognizable on
/// a re-run.
pub fn fingerprint_marker(fingerprint: &str) -> String {
    format!("<!-- momus:fp={fingerprint} -->")
}

/// Fingerprints already posted, read from existing review comment bodies.
pub fn posted_fingerprints<'a>(bodies: impl IntoIterator<Item = &'a str>) -> HashSet<String> {
    bodies
        .into_iter()
        .flat_map(|body| FINGERPRINT_MARKER.captures_iter(body))
        .map(|caps| caps[1].to_string())
        .collect()
}

static TOPIC_MARKER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"<!-- momus:topic=([A-Za-z]+/[A-Za-z0-9_]+) -->").expect("valid marker regex")
});

/// A bot comment within this many lines, on the same file with the same
/// dimension and mechanism, means the finding is probably already posted even
/// when its fingerprint changed (the fingerprint hashes the evidence text, so
/// an edit nearby changes it). It could also be a second, distinct defect, so
/// such a finding is not posted inline again but is listed in the summary:
/// nothing is dropped on a guess.
const REPOST_LINE_WINDOW: usize = 20;

/// The topic a finding is about, `dimension/mechanism`.
fn topic(finding: &Finding) -> String {
    format!("{}/{}", finding.dimension.key(), finding.mechanism)
}

/// The hidden marker that records a posted comment's topic, so a re-run can
/// recognize the same concern after the code around it changed.
pub fn topic_marker(finding: &Finding) -> String {
    format!("<!-- momus:topic={} -->", topic(finding))
}

/// A posted comment's topic and where it sits.
#[derive(Debug, Clone, PartialEq)]
pub struct PostedTopic {
    pub path: String,
    pub line: usize,
    pub topic: String,
}

/// Topics already posted, from `(path, line, body)` of existing review
/// comments (a comment GitHub no longer places on a line has none).
pub fn posted_topics<'a>(
    comments: impl IntoIterator<Item = (&'a str, Option<usize>, &'a str)>,
) -> Vec<PostedTopic> {
    comments
        .into_iter()
        .filter_map(|(path, line, body)| {
            let caps = TOPIC_MARKER.captures(body)?;
            Some(PostedTopic {
                path: path.to_string(),
                line: line?,
                topic: caps[1].to_string(),
            })
        })
        .collect()
}

/// What earlier runs already posted on the pull request.
#[derive(Debug, Default)]
pub struct Posted {
    pub fingerprints: HashSet<String>,
    pub topics: Vec<PostedTopic>,
}

impl Posted {
    /// This exact finding (its fingerprint) is already posted.
    fn has(&self, finding: &Finding) -> bool {
        !finding.fingerprint.is_empty() && self.fingerprints.contains(&finding.fingerprint)
    }

    /// The same concern is posted near the finding's line: probably the same
    /// finding after an edit, possibly a distinct one.
    fn has_near(&self, finding: &Finding) -> bool {
        self.topics.iter().any(|t| {
            t.path == finding.file
                && t.line.abs_diff(finding.line) <= REPOST_LINE_WINDOW
                && t.topic == topic(finding)
        })
    }
}

/// A file of the pull request as `GET /pulls/{n}/files` lists it. `patch`
/// is absent for binary files and diffs GitHub considers too large.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PrFile {
    pub filename: String,
    #[serde(default)]
    pub patch: Option<String>,
}

/// The new-file (RIGHT side) lines a review comment can target: added and
/// context lines inside the patch's hunks.
pub fn commentable_lines(patch: &str) -> BTreeSet<usize> {
    let mut lines = BTreeSet::new();
    for hunk in parse_hunks(patch) {
        let mut line = hunk.start_line;
        for text in hunk.patch.split('\n').skip(1) {
            match text.as_bytes().first() {
                Some(b'+') | Some(b' ') => {
                    lines.insert(line);
                    line += 1;
                }
                // `-` removals and `\ No newline at end of file` take no new line.
                _ => {}
            }
        }
    }
    lines
}

/// One inline comment of a review, in the shape `POST /pulls/{n}/reviews`
/// takes.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InlineComment {
    pub path: String,
    pub line: usize,
    pub side: &'static str,
    pub body: String,
}

/// Why a finding is listed in the summary instead of inline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummaryReason {
    /// A `momus scan` report: its findings are not tied to the PR's diff.
    NotADiffReview,
    /// The finding's line is not a commentable line of the PR's diff.
    OutsideDiff,
    /// More findings than the inline comment cap.
    OverCap,
    /// GitHub rejected the review's anchors (the report was made from a
    /// different commit than the PR head).
    Rejected,
    /// A bot comment with the same concern sits within a few lines: likely
    /// this finding before an edit, so it is not posted inline again.
    NearPosted,
}

impl SummaryReason {
    /// Render a concise finding description for inline or summary publication.
    fn describe(self) -> &'static str {
        match self {
            SummaryReason::NotADiffReview => "codebase scan",
            SummaryReason::OutsideDiff => "outside the diff",
            SummaryReason::OverCap => "over the inline cap",
            SummaryReason::Rejected => "GitHub rejected the anchor",
            SummaryReason::NearPosted => "same concern already posted nearby",
        }
    }
}

/// How a report's findings split between inline comments and the summary.
#[derive(Debug, Default)]
pub struct Plan<'a> {
    /// New inline comments, best first.
    pub inline: Vec<(&'a Finding, InlineComment)>,
    pub summary_only: Vec<(&'a Finding, SummaryReason)>,
    /// Findings whose fingerprint an earlier run already posted inline.
    pub already_posted: usize,
}

impl Plan<'_> {
    /// Moves every inline comment to the summary (the review was rejected).
    pub fn demote_inline(&mut self) {
        for (finding, _) in self.inline.drain(..) {
            self.summary_only.push((finding, SummaryReason::Rejected));
        }
    }
}

/// Fix-first order: ranked findings by rank, then the rest by severity.
fn fix_first(a: &Finding, b: &Finding) -> std::cmp::Ordering {
    let rank = |f: &Finding| f.rank.unwrap_or(usize::MAX);
    rank(a)
        .cmp(&rank(b))
        .then(b.severity.total_cmp(&a.severity))
}

/// Splits `report`'s findings into at most `max_inline` new inline comments
/// and summary-only findings. A finding whose fingerprint `posted` already
/// has is counted but not posted again; one whose concern is posted nearby
/// goes to the summary rather than inline (`SummaryReason::NearPosted`).
pub fn plan<'a>(
    report: &'a ReviewReport,
    files: &[PrFile],
    posted: &Posted,
    max_inline: usize,
) -> Plan<'a> {
    let commentable: HashMap<&str, BTreeSet<usize>> = files
        .iter()
        .filter_map(|f| {
            f.patch
                .as_deref()
                .map(|p| (f.filename.as_str(), commentable_lines(p)))
        })
        .collect();

    let mut findings: Vec<&Finding> = report.findings.iter().collect();
    findings.sort_by(|a, b| fix_first(a, b));

    let mut out = Plan::default();
    let mut seen: HashSet<&str> = HashSet::new();
    for finding in findings {
        let fp = finding.fingerprint.as_str();
        if posted.has(finding) || (!fp.is_empty() && !seen.insert(fp)) {
            out.already_posted += 1;
            continue;
        }
        let reason = if report.mode == ReviewMode::Codebase {
            Some(SummaryReason::NotADiffReview)
        } else if posted.has_near(finding) {
            Some(SummaryReason::NearPosted)
        } else if !commentable
            .get(finding.file.as_str())
            .is_some_and(|lines| lines.contains(&finding.line))
        {
            Some(SummaryReason::OutsideDiff)
        } else if out.inline.len() >= max_inline {
            Some(SummaryReason::OverCap)
        } else {
            None
        };
        match reason {
            Some(reason) => out.summary_only.push((finding, reason)),
            None => {
                let comment = InlineComment {
                    path: finding.file.clone(),
                    line: finding.line,
                    side: "RIGHT",
                    body: comment_body(finding),
                };
                out.inline.push((finding, comment));
            }
        }
    }
    out
}

/// Return the human-readable label for a review dimension.
fn dimension_label(dimension: Dimension) -> String {
    dimension_metadata()
        .into_iter()
        .find(|meta| meta.key == dimension)
        .map(|meta| meta.label)
        .unwrap_or_else(|| dimension.key().to_string())
}

/// `**title** (dimension, severity 2.3)`: the heading line of the dashboard's
/// `formatPrComment`.
fn heading(finding: &Finding) -> String {
    let dimension = dimension_label(finding.dimension);
    let title = finding.title.as_deref().unwrap_or(&dimension);
    format!(
        "**{title}** ({dimension}, severity {:.1})",
        finding.severity
    )
}

/// Replace secrets in outbound GitHub comment text before publication.
fn redacted(text: &str) -> String {
    redact(text, &mut Redactions::default())
}

/// An inline comment body: a port of the dashboard's `formatPrComment`
/// (without the `file:line` line, which the anchor already shows), redacted,
/// plus the fingerprint marker.
pub fn comment_body(finding: &Finding) -> String {
    let mut lines = vec![heading(finding)];
    if let Some(why) = &finding.why {
        lines.push(format!("> {why}"));
    }
    if let Some(fix) = &finding.fix {
        lines.push(format!("- Fix: {fix}"));
    }
    if let Some(test) = &finding.test {
        lines.push(format!("- Test: {test}"));
    }
    let mut body = redacted(&lines.join("\n"));
    let mut markers = Vec::new();
    if !finding.fingerprint.is_empty() {
        markers.push(fingerprint_marker(&finding.fingerprint));
    }
    if !finding.mechanism.is_empty() {
        markers.push(topic_marker(finding));
    }
    if !markers.is_empty() {
        body.push_str("\n\n");
        body.push_str(&markers.join("\n"));
    }
    body
}

/// The sticky summary comment: counts, P(revert), usage, redaction totals,
/// and the findings not posted inline. Starts with `SUMMARY_MARKER`.
pub fn summary_body(report: &ReviewReport, plan: &Plan, head_sha: &str) -> String {
    let total = report.findings.len();
    let blocking = report
        .findings
        .iter()
        .filter(|f| f.action == Action::RequestChanges)
        .count();
    let mut out = vec!["### momus review".to_string(), String::new()];

    if report.partial {
        out.push(format!("**Partial coverage**: {} deferred requests, {} Tier-0 dismissals, {} skipped requests. Rerun to complete uncached work.",
            report.budget.deferred, report.tier.dismissed.len(), report.skipped.len()));
        out.push(String::new());
    }
    if total == 0 && report.screened_files == 0 && report.skipped.is_empty() && !report.partial {
        out.push("No source files to review in this pull request.".to_string());
    } else if total == 0 {
        out.push("No findings.".to_string());
    } else {
        out.push(format!(
            "**{total} finding{}** · {blocking} blocking · P(revert) {:.2} (uncalibrated)",
            if total == 1 { "" } else { "s" },
            report.p_revert
        ));
        out.push(String::new());
        // Totals, not per-run counts, so an identical re-run leaves the
        // summary unchanged.
        out.push(format!(
            "{} posted inline · {} in this summary",
            plan.inline.len() + plan.already_posted,
            plan.summary_only.len()
        ));
        out.push(format!(
            "findings on {} of {} screened file{}",
            report
                .findings
                .iter()
                .map(|f| f.file.as_str())
                .collect::<BTreeSet<_>>()
                .len(),
            report.screened_files,
            if report.screened_files == 1 { "" } else { "s" }
        ));
    }

    if !plan.summary_only.is_empty() {
        out.push(String::new());
        out.push("<details><summary>Findings not posted inline</summary>".to_string());
        out.push(String::new());
        for (finding, reason) in plan.summary_only.iter().take(SUMMARY_LIST_MAX) {
            out.push(format!(
                "- {} `{}:{}` · {}",
                heading(finding),
                finding.file,
                finding.line,
                reason.describe()
            ));
        }
        if plan.summary_only.len() > SUMMARY_LIST_MAX {
            out.push(format!(
                "- … and {} more",
                plan.summary_only.len() - SUMMARY_LIST_MAX
            ));
        }
        out.push(String::new());
        out.push("</details>".to_string());
    }

    if !report.skipped.is_empty() {
        out.push(String::new());
        out.push(format!(
            "<details><summary>{} request{} failed and {} skipped (not reviewed)</summary>",
            report.skipped.len(),
            if report.skipped.len() == 1 { "" } else { "s" },
            if report.skipped.len() == 1 {
                "was"
            } else {
                "were"
            }
        ));
        out.push(String::new());
        for skip in report.skipped.iter().take(SUMMARY_LIST_MAX) {
            out.push(format!(
                "- `{}` · {}: {}",
                skip.file,
                skip.stage.key(),
                skip.reason
            ));
        }
        if report.skipped.len() > SUMMARY_LIST_MAX {
            out.push(format!(
                "- … and {} more",
                report.skipped.len() - SUMMARY_LIST_MAX
            ));
        }
        out.push(String::new());
        out.push("</details>".to_string());
    }

    let mut footer = vec![format!("Jev calls: {}", report.usage.calls)];
    if report.usage.input_tokens + report.usage.output_tokens > 0 {
        footer.push(format!(
            "tokens: {} in / {} out",
            report.usage.input_tokens, report.usage.output_tokens
        ));
    }
    if !report.redactions.is_empty() {
        let rules: Vec<String> = report
            .redactions
            .iter()
            .map(|(rule, n)| format!("{rule} ×{n}"))
            .collect();
        footer.push(format!("redacted before sending: {}", rules.join(", ")));
    }
    let short_sha: String = head_sha.chars().take(7).collect();
    if !short_sha.is_empty() {
        footer.push(format!("head {short_sha}"));
    }
    out.push(String::new());
    out.push(format!("<sub>{}</sub>", footer.join(" · ")));

    format!("{SUMMARY_MARKER}\n{}", redacted(&out.join("\n")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Incomplete reviews must disclose deferred, dismissed, and failed work.
    #[test]
    fn summary_marks_partial_coverage_with_actual_counters() {
        let mut report = ReviewReport {
            partial: true,
            ..Default::default()
        };
        report.budget.deferred = 3;
        report.tier.dismissed = vec!["a.rs".into(), "b.rs".into()];
        report.skipped.push(crate::domain::report::SkippedFile {
            file: "c.rs".into(),
            stage: crate::domain::report::ReviewStage::Screen,
            reason: "budget exhausted".into(),
        });
        let plan = plan(&report, &[], &Posted::default(), 10);
        let body = summary_body(&report, &plan, "abcdef");
        assert!(body.contains("**Partial coverage**"));
        assert!(body.contains("3 deferred requests, 2 Tier-0 dismissals, 1 skipped requests"));
        drop(plan);
        report.skipped.clear();
        let dismissed_plan = super::plan(&report, &[], &Posted::default(), 10);
        let dismissed_body = summary_body(&report, &dismissed_plan, "abcdef");
        assert!(dismissed_body.contains("No findings."));
        assert!(!dismissed_body.contains("No source files"));
        drop(dismissed_plan);
        report.partial = false;
        let plan = super::plan(&report, &[], &Posted::default(), 10);
        assert!(!summary_body(&report, &plan, "abcdef").contains("Partial coverage"));
    }

    fn finding(file: &str, line: usize, severity: f64, fingerprint: &str) -> Finding {
        Finding {
            file: file.into(),
            line,
            dimension: Dimension::Security,
            severity,
            fingerprint: fingerprint.into(),
            title: Some(format!("Issue at {line}")),
            ..Default::default()
        }
    }

    fn pr_file(filename: &str, patch: &str) -> PrFile {
        PrFile {
            filename: filename.into(),
            patch: Some(patch.into()),
        }
    }

    #[test]
    fn commentable_lines_are_added_and_context_lines() {
        // new lines 10 (ctx), 11 (added), 12 (ctx); the removal takes none.
        let patch = "@@ -10,3 +10,3 @@ fn f()\n a\n-b\n+c\n d\n@@ -40,1 +40,2 @@\n x\n+y";
        let lines: Vec<usize> = commentable_lines(patch).into_iter().collect();
        assert_eq!(lines, vec![10, 11, 12, 40, 41]);
    }

    #[test]
    fn deleted_file_and_no_newline_marker_have_no_extra_lines() {
        assert!(commentable_lines("@@ -1,2 +0,0 @@\n-a\n-b").is_empty());
        let lines: Vec<usize> =
            commentable_lines("@@ -1 +1 @@\n-a\n+b\n\\ No newline at end of file")
                .into_iter()
                .collect();
        assert_eq!(lines, vec![1]);
    }

    #[test]
    fn plan_anchors_inside_the_diff_and_summarizes_the_rest() {
        let report = ReviewReport {
            findings: vec![
                finding("src/a.rs", 11, 2.0, "aaaa"),
                finding("src/a.rs", 90, 2.5, "bbbb"),
                finding("src/other.rs", 1, 1.0, "cccc"),
            ],
            ..Default::default()
        };
        let files = [pr_file("src/a.rs", "@@ -10,2 +10,3 @@\n a\n+b\n c")];

        let plan = plan(&report, &files, &Posted::default(), 10);
        assert_eq!(plan.inline.len(), 1);
        assert_eq!(plan.inline[0].1.path, "src/a.rs");
        assert_eq!(plan.inline[0].1.line, 11);
        assert_eq!(plan.inline[0].1.side, "RIGHT");
        let reasons: Vec<_> = plan
            .summary_only
            .iter()
            .map(|(f, r)| (f.line, *r))
            .collect();
        assert_eq!(
            reasons,
            vec![
                (90, SummaryReason::OutsideDiff),
                (1, SummaryReason::OutsideDiff)
            ]
        );
    }

    #[test]
    fn plan_caps_inline_comments_in_rank_then_severity_order() {
        let mut ranked = finding("src/a.rs", 1, 0.5, "r1");
        ranked.rank = Some(1);
        let report = ReviewReport {
            findings: vec![
                finding("src/a.rs", 2, 1.0, "low"),
                finding("src/a.rs", 3, 2.9, "high"),
                ranked,
            ],
            ..Default::default()
        };
        let files = [pr_file("src/a.rs", "@@ -0,0 +1,3 @@\n+a\n+b\n+c")];

        let plan = plan(&report, &files, &Posted::default(), 2);
        let inline: Vec<usize> = plan.inline.iter().map(|(f, _)| f.line).collect();
        assert_eq!(inline, vec![1, 3], "rank 1 first, then the most severe");
        assert_eq!(plan.summary_only.len(), 1);
        assert_eq!(plan.summary_only[0].0.line, 2);
        assert_eq!(plan.summary_only[0].1, SummaryReason::OverCap);
    }

    #[test]
    fn a_posted_topic_nearby_moves_a_changed_fingerprint_to_the_summary() {
        let at = |line, mechanism: &str, fp: &str| Finding {
            mechanism: mechanism.into(),
            ..finding("src/a.rs", line, 2.0, fp)
        };
        let report = ReviewReport {
            findings: vec![
                at(12, "boundary", "new1"),
                at(40, "boundary", "new2"),
                at(12, "nullDeref", "new3"),
            ],
            ..Default::default()
        };
        let files = [pr_file("src/a.rs", "@@ -0,0 +1,50 @@\n+x")];
        let body = comment_body(&at(10, "boundary", "old"));
        let posted = Posted {
            topics: posted_topics([
                ("src/a.rs", Some(10), body.as_str()),
                ("src/b.rs", Some(12), body.as_str()),
            ]),
            ..Default::default()
        };
        assert_eq!(posted.topics.len(), 2);
        let plan = plan(&report, &files, &posted, 10);
        // 12 is within the window of the posted boundary at 10: not inline
        // again, but listed, since it could be a second defect. 40 is too far,
        // and nullDeref is another concern: both are new (outside this
        // one-line diff, so summary too, but not as NearPosted).
        assert_eq!(
            plan.already_posted, 0,
            "a topic match never silently drops a finding"
        );
        let near: Vec<(usize, &str)> = plan
            .summary_only
            .iter()
            .filter(|(_, reason)| *reason == SummaryReason::NearPosted)
            .map(|(f, _)| (f.line, f.mechanism.as_str()))
            .collect();
        assert_eq!(near, [(12, "boundary")]);
        assert_eq!(
            plan.inline.len() + plan.summary_only.len(),
            3,
            "all three findings are kept"
        );
    }

    #[test]
    fn a_comment_without_a_line_or_marker_records_no_topic() {
        let body = comment_body(&Finding {
            mechanism: "boundary".into(),
            ..finding("src/a.rs", 1, 2.0, "f")
        });
        assert!(
            posted_topics([
                ("src/a.rs", None, body.as_str()),
                ("src/a.rs", Some(3), "plain text")
            ])
            .is_empty()
        );
    }

    #[test]
    fn already_posted_fingerprints_are_skipped() {
        let report = ReviewReport {
            findings: vec![
                finding("src/a.rs", 1, 2.0, "0123abcd"),
                finding("src/a.rs", 2, 2.0, "ffff"),
            ],
            ..Default::default()
        };
        let files = [pr_file("src/a.rs", "@@ -0,0 +1,2 @@\n+a\n+b")];
        let bodies = ["**x**\n\n<!-- momus:fp=0123abcd -->", "a human comment"];
        let posted = posted_fingerprints(bodies);
        assert_eq!(posted, HashSet::from(["0123abcd".to_string()]));

        let posted = Posted {
            fingerprints: posted,
            ..Default::default()
        };
        let plan = plan(&report, &files, &posted, 10);
        assert_eq!(plan.already_posted, 1);
        assert_eq!(plan.inline.len(), 1);
        assert_eq!(plan.inline[0].0.fingerprint, "ffff");
    }

    #[test]
    fn scan_reports_go_to_the_summary() {
        let report = ReviewReport {
            mode: ReviewMode::Codebase,
            findings: vec![finding("src/a.rs", 1, 2.0, "aaaa")],
            ..Default::default()
        };
        let files = [pr_file("src/a.rs", "@@ -0,0 +1 @@\n+a")];
        let plan = plan(&report, &files, &Posted::default(), 10);
        assert!(plan.inline.is_empty());
        assert_eq!(plan.summary_only[0].1, SummaryReason::NotADiffReview);
    }

    #[test]
    fn comment_body_ports_format_pr_comment_with_a_marker() {
        let finding = Finding {
            why: Some("User input reaches a SQL string.".into()),
            fix: Some("Use a parameterized query.".into()),
            test: Some("Add a test with a quote in the name.".into()),
            mechanism: "sqlInjection".into(),
            ..finding("src/a.rs", 1, 2.24, "0123456789abcdef")
        };
        assert_eq!(
            comment_body(&finding),
            "**Issue at 1** (Security, severity 2.2)\n\
             > User input reaches a SQL string.\n\
             - Fix: Use a parameterized query.\n\
             - Test: Add a test with a quote in the name.\n\
             \n\
             <!-- momus:fp=0123456789abcdef -->\n\
             <!-- momus:topic=security/sqlInjection -->"
        );

        // No title: the dimension label heads the comment; no fingerprint or
        // mechanism, no markers.
        let bare = Finding {
            dimension: Dimension::TestGap,
            severity: 1.0,
            ..Default::default()
        };
        assert_eq!(comment_body(&bare), "**Test gap** (Test gap, severity 1.0)");
    }

    #[test]
    fn published_text_is_redacted() {
        let finding = Finding {
            fix: Some("Rotate AKIAIOSFODNN7EXAMPLE and load it from the environment.".into()),
            ..finding("src/a.rs", 1, 2.0, "aaaa")
        };
        let body = comment_body(&finding);
        assert!(!body.contains("AKIAIOSFODNN7EXAMPLE"), "{body}");
        assert!(body.contains("<redacted:aws-access-key>"), "{body}");
        assert!(body.ends_with("<!-- momus:fp=aaaa -->"));

        let mut leaky = finding;
        leaky.title = Some("Hardcoded AKIAIOSFODNN7EXAMPLE".into());
        let report = ReviewReport {
            findings: vec![leaky],
            ..Default::default()
        };
        let mut plan = Plan::default();
        plan.summary_only
            .push((&report.findings[0], SummaryReason::OutsideDiff));
        let summary = summary_body(&report, &plan, "");
        assert!(!summary.contains("AKIAIOSFODNN7EXAMPLE"), "{summary}");
    }

    #[test]
    fn summary_reports_counts_usage_and_redactions() {
        let mut blocking = finding("src/a.rs", 3, 2.5, "aaaa");
        blocking.action = Action::RequestChanges;
        let report = ReviewReport {
            findings: vec![blocking, finding("src/b.rs", 7, 1.0, "bbbb")],
            p_revert: 0.123,
            screened_files: 5,
            usage: crate::domain::report::UsageSummary {
                calls: 12,
                input_tokens: 0,
                output_tokens: 0,
                ..Default::default()
            },
            redactions: [("aws-access-key".to_string(), 1)].into(),
            ..Default::default()
        };
        let files = [pr_file("src/a.rs", "@@ -0,0 +1,3 @@\n+a\n+b\n+c")];
        let plan = plan(&report, &files, &Posted::default(), 10);
        let summary = summary_body(&report, &plan, "0123456789abcdef");

        assert!(summary.starts_with(SUMMARY_MARKER), "{summary}");
        assert!(
            summary.contains("**2 findings** · 1 blocking · P(revert) 0.12"),
            "{summary}"
        );
        assert!(
            summary.contains("1 posted inline · 1 in this summary"),
            "{summary}"
        );
        assert!(
            summary.contains("findings on 2 of 5 screened files"),
            "{summary}"
        );
        assert!(
            summary.contains(
                "- **Issue at 7** (Security, severity 1.0) `src/b.rs:7` · outside the diff"
            )
        );
        assert!(
            summary.contains(
                "Jev calls: 12 · redacted before sending: aws-access-key ×1 · head 0123456"
            )
        );
        assert!(
            !summary.contains("tokens:"),
            "no token totals when the server reports none"
        );
    }

    #[test]
    fn nothing_to_review_says_so() {
        let summary = summary_body(&ReviewReport::default(), &Plan::default(), "abc");
        assert!(
            summary.contains("No source files to review in this pull request."),
            "{summary}"
        );
        assert!(!summary.contains("No findings."));
    }

    #[test]
    fn summary_lists_skipped_requests() {
        use crate::domain::report::{ReviewStage, SkippedFile};
        let report = ReviewReport {
            skipped: vec![SkippedFile {
                file: "app.ts".into(),
                stage: ReviewStage::Screen,
                reason: "system_one failed (400 Bad Request): max_tokens_exceeded".into(),
            }],
            screened_files: 2,
            ..Default::default()
        };
        let summary = summary_body(&report, &Plan::default(), "");
        assert!(
            summary.contains("1 request failed and was skipped (not reviewed)"),
            "{summary}"
        );
        assert!(
            summary.contains("- `app.ts` · screen: system_one failed (400 Bad Request)"),
            "{summary}"
        );
    }

    #[test]
    fn empty_report_summary() {
        let report = ReviewReport {
            screened_files: 3,
            ..Default::default()
        };
        let summary = summary_body(&report, &Plan::default(), "abc");
        assert!(summary.contains("No findings."));
        assert!(!summary.contains("<details>"));
    }
}
