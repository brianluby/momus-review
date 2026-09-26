//! Local-only dashboard server: three allowlisted static assets plus the saved
//! review report as JSON. Binds to loopback only. Mirrors `dashboard/server.ts`
//! with the same Host/CSP/nosniff hardening.

use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, Request, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router, routing::get};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::adapters::feedback_store::{append_feedback, feedback_path, read_feedback};
use crate::adapters::report_store::{
    HistoryEntry, StoredReport, read_history, read_report, report_path,
};
use crate::domain::feedback::{FeedbackEntry, FeedbackLog, Vote};
use crate::domain::report::{Action, ReviewReport};

pub const DEFAULT_PORT: u16 = 4317;

const INDEX_HTML: &str = include_str!("public/index.html");
const STYLE_CSS: &str = include_str!("public/style.css");
const APP_JS: &str = include_str!("public/app.js");

/// Serves the dashboard on `127.0.0.1:<port>` until the process is killed.
pub async fn serve(port: u16) -> anyhow::Result<()> {
    let router = Router::new()
        .route("/api/review", get(api_review))
        .route("/api/history", get(api_history))
        .route("/api/feedback", get(api_feedback).post(post_feedback))
        .route("/", get(index).head(index))
        .route("/style.css", get(style_css).head(style_css))
        .route("/app.js", get(app_js).head(app_js))
        .fallback(not_found)
        .with_state(port)
        .layer(middleware::from_fn_with_state(port, host_check))
        .layer(middleware::from_fn(security_headers));

    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    println!("Jev review dashboard: http://{addr}");
    let report = report_path();
    println!("report: {}", report.display());

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router).await?;
    Ok(())
}

/// Host allowlist: only `127.0.0.1:<port>` / `localhost:<port>` (trimmed and
/// lowercased) are served; anything else is 403.
async fn host_check(
    State(port): State<u16>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .trim()
        .to_lowercase();
    let expected = [format!("127.0.0.1:{port}"), format!("localhost:{port}")];
    if !expected.contains(&host) {
        return (StatusCode::FORBIDDEN, "Forbidden").into_response();
    }
    next.run(req).await
}

/// Adds the must-preserve security headers to every response.
async fn security_headers(req: Request<axum::body::Body>, next: Next) -> Response {
    let mut resp = next.run(req).await;
    let h = resp.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; img-src 'self' data:; frame-ancestors 'none'",
        ),
    );
    resp
}

async fn api_review() -> Json<Value> {
    let path = report_path();
    let source = path.display().to_string();
    let body = match read_report(&path) {
        StoredReport::Ok { saved_at, report } => json!({
            "source": source,
            "status": "ok",
            "savedAt": saved_at,
            "report": report,
        }),
        StoredReport::Empty => json!({ "source": source, "status": "empty" }),
        StoredReport::Error(message) => json!({
            "source": source,
            "status": "error",
            "message": message,
        }),
    };
    Json(body)
}

/// Per-sha history trend: chronological entries plus top file hotspots
/// aggregated across every saved history report. A missing history dir is
/// empty history (`read_history` returns `Ok(vec![])`); a genuine read
/// failure (e.g. permission denied) surfaces as a 500 rather than a silent
/// empty list.
async fn api_history() -> Result<Json<Value>, StatusCode> {
    let mut entries = read_history().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Chronological at full mtime precision: whole-second `saved_at` would
    // leave same-second saves (e.g. a scan then a review) in arbitrary
    // directory order, and `hotspots` relies on this order.
    entries.sort_by_key(|e| e.saved_nanos);

    let entry_values: Vec<Value> = entries
        .iter()
        .map(|e| {
            let findings = e.report.findings.len();
            let blocking = e
                .report
                .findings
                .iter()
                .filter(|f| f.action == Action::RequestChanges)
                .count();
            let max_severity = e
                .report
                .findings
                .iter()
                .map(|f| f.severity)
                .fold(0.0, f64::max);
            json!({
                "sha": e.sha,
                "mode": e.report.mode,
                "savedAt": e.saved_at,
                "findings": findings,
                "blocking": blocking,
                "maxSeverity": max_severity,
            })
        })
        .collect();

    let hotspot_values = hotspots(&entries);

    Ok(Json(json!({ "entries": entry_values, "hotspots": hotspot_values })))
}

/// Top file hotspots aggregated across `entries` (chronological). A file's
/// open/resolved state comes from the most recent entry that actually
/// screened it (its matrix covers the file), not simply the newest entry: a
/// diff review touching three files says nothing about the rest of the repo,
/// so it must not mark their earlier findings resolved.
fn hotspots(entries: &[HistoryEntry]) -> Vec<Value> {
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Hotspot {
        findings: usize,
        latest_findings: usize,
        blocking: usize,
        max_severity: f64,
    }

    let mut by_file: BTreeMap<String, Hotspot> = BTreeMap::new();
    for entry in entries {
        let mut current: BTreeMap<&str, usize> = BTreeMap::new();
        for row in &entry.report.matrix {
            current.insert(row.file.as_str(), 0);
        }
        for f in &entry.report.findings {
            *current.entry(f.file.as_str()).or_insert(0) += 1;
            let slot = by_file.entry(f.file.clone()).or_default();
            slot.findings += 1;
            if f.action == Action::RequestChanges {
                slot.blocking += 1;
            }
            if f.severity > slot.max_severity {
                slot.max_severity = f.severity;
            }
        }
        // This entry covered these files: it is now their latest word.
        for (file, count) in current {
            if let Some(slot) = by_file.get_mut(file) {
                slot.latest_findings = count;
            }
        }
    }

    let mut ranked: Vec<(String, Hotspot)> = by_file.into_iter().collect();
    ranked.sort_by_key(|(_, h)| std::cmp::Reverse(h.findings));
    ranked
        .into_iter()
        .take(15)
        .map(|(file, h)| {
            json!({
                "file": file,
                "findings": h.findings,
                "latestFindings": h.latest_findings,
                "blocking": h.blocking,
                "maxSeverity": h.max_severity,
            })
        })
        .collect()
}

/// A thumbs up/down from the dashboard. Only the fingerprint and verdict come
/// from the client; file, dimension, mechanism, and probability are taken
/// from the saved report.
#[derive(Deserialize)]
struct FeedbackRequest {
    fingerprint: String,
    vote: Vote,
    #[serde(default)]
    suppress: bool,
}

/// The latest vote per fingerprint: `{ votes: { <fingerprint>: { vote, suppress } } }`.
async fn api_feedback() -> Result<Json<Value>, StatusCode> {
    let log = read_feedback(&feedback_path()).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(votes_body(&log)))
}

/// Records a vote. Cross-site requests are refused twice over: the `Json`
/// extractor requires `Content-Type: application/json` (so a browser must
/// preflight, which this server never approves), and a browser `Origin` must
/// be this dashboard's.
async fn post_feedback(
    State(port): State<u16>,
    headers: HeaderMap,
    Json(request): Json<FeedbackRequest>,
) -> Result<Json<Value>, (StatusCode, &'static str)> {
    if !same_origin(&headers, port) {
        return Err((StatusCode::FORBIDDEN, "Cross-origin feedback refused"));
    }
    if !valid_vote(&request) {
        return Err((StatusCode::UNPROCESSABLE_ENTITY, "Only a down vote can suppress a finding"));
    }
    let StoredReport::Ok { report, .. } = read_report(&report_path()) else {
        return Err((StatusCode::CONFLICT, "No saved report"));
    };
    let entry = feedback_entry(&report, &request).ok_or((StatusCode::NOT_FOUND, "Unknown finding"))?;
    let log = append_feedback(&feedback_path(), entry)
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Could not save feedback"))?;
    Ok(Json(votes_body(&log)))
}

/// Suppression is the "Hide" (down vote) action; an up vote cannot hide.
fn valid_vote(request: &FeedbackRequest) -> bool {
    !(request.suppress && request.vote == Vote::Up)
}

/// A missing `Origin` is a non-browser client (which could write the file
/// directly anyway); a present one must name this dashboard.
fn same_origin(headers: &HeaderMap, port: u16) -> bool {
    match headers.get(header::ORIGIN).map(|o| o.to_str().unwrap_or("")) {
        None => true,
        Some(origin) => {
            let origin = origin.trim().to_lowercase();
            origin == format!("http://127.0.0.1:{port}") || origin == format!("http://localhost:{port}")
        }
    }
}

/// Builds a log entry for the report finding with `request.fingerprint`.
fn feedback_entry(report: &ReviewReport, request: &FeedbackRequest) -> Option<FeedbackEntry> {
    if request.fingerprint.is_empty() {
        return None;
    }
    let finding = report.findings.iter().find(|f| f.fingerprint == request.fingerprint)?;
    Some(FeedbackEntry {
        fingerprint: finding.fingerprint.clone(),
        file: finding.file.clone(),
        line: finding.line,
        dimension: finding.dimension,
        mechanism: finding.mechanism.clone(),
        probability: finding.probability,
        vote: request.vote,
        suppress: request.suppress,
        at: String::new(),
    })
}

fn votes_body(log: &FeedbackLog) -> Value {
    let votes: serde_json::Map<String, Value> = log
        .latest()
        .into_iter()
        .map(|e| (e.fingerprint.clone(), json!({ "vote": e.vote, "suppress": e.suppress })))
        .collect();
    json!({ "votes": votes })
}

async fn index() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], INDEX_HTML)
}

async fn style_css() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/css; charset=utf-8")], STYLE_CSS)
}

async fn app_js() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        APP_JS,
    )
}

async fn not_found() -> impl IntoResponse {
    (StatusCode::NOT_FOUND, "Not found")
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::policy::Dimension;
    use crate::domain::report::{Finding, MatrixRow, ReviewMode};

    #[test]
    fn origin_must_be_this_dashboard_when_present() {
        let mut headers = HeaderMap::new();
        assert!(same_origin(&headers, 4317));
        headers.insert(header::ORIGIN, HeaderValue::from_static("http://localhost:4317"));
        assert!(same_origin(&headers, 4317));
        headers.insert(header::ORIGIN, HeaderValue::from_static("http://127.0.0.1:4317"));
        assert!(same_origin(&headers, 4317));
        assert!(!same_origin(&headers, 9999));
        headers.insert(header::ORIGIN, HeaderValue::from_static("https://evil.example"));
        assert!(!same_origin(&headers, 4317));
        headers.insert(header::ORIGIN, HeaderValue::from_static("null"));
        assert!(!same_origin(&headers, 4317));
    }

    #[test]
    fn feedback_entry_takes_details_from_the_report() {
        let report = ReviewReport {
            findings: vec![Finding {
                file: "src/a.rs".into(),
                line: 7,
                mechanism: "xss".into(),
                probability: 0.81,
                fingerprint: "abc".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let request = |fp: &str| FeedbackRequest { fingerprint: fp.into(), vote: Vote::Down, suppress: true };
        let entry = feedback_entry(&report, &request("abc")).unwrap();
        assert_eq!((entry.file.as_str(), entry.line, entry.probability), ("src/a.rs", 7, 0.81));
        assert!(entry.suppress);
        assert!(feedback_entry(&report, &request("nope")).is_none());
        assert!(feedback_entry(&report, &request("")).is_none());
    }

    #[test]
    fn only_a_down_vote_can_suppress() {
        let request = |vote, suppress| FeedbackRequest { fingerprint: "abc".into(), vote, suppress };
        assert!(valid_vote(&request(Vote::Down, true)));
        assert!(valid_vote(&request(Vote::Down, false)));
        assert!(valid_vote(&request(Vote::Up, false)));
        assert!(!valid_vote(&request(Vote::Up, true)));
    }

    fn finding(file: &str) -> Finding {
        Finding {
            file: file.into(),
            line: 1,
            dimension: Dimension::Correctness,
            probability: 0.9,
            location_confidence: 0.9,
            mechanism: "condition".into(),
            mechanism_confidence: 0.8,
            severity: 2.5,
            severity_confidence: 0.8,
            owner: None,
            owner_confidence: None,
            action: Action::RequestChanges,
            evidence: String::new(),
            ..Default::default()
        }
    }

    fn entry(mode: ReviewMode, screened: &[&str], findings: &[&str]) -> HistoryEntry {
        HistoryEntry {
            sha: "abc".into(),
            saved_at: String::new(),
            saved_nanos: 0,
            report: ReviewReport {
                mode,
                matrix: screened
                    .iter()
                    .map(|f| MatrixRow { file: (*f).into(), probabilities: Default::default() })
                    .collect(),
                findings: findings.iter().map(|f| finding(f)).collect(),
                ..Default::default()
            },
        }
    }

    #[test]
    fn narrow_review_does_not_resolve_unscreened_files() {
        let entries = vec![
            entry(ReviewMode::Codebase, &["a.rs", "b.rs", "c.rs"], &["a.rs", "b.rs"]),
            // A later diff review screens only b.rs and c.rs, and b.rs is clean.
            entry(ReviewMode::Changes, &["b.rs", "c.rs"], &[]),
        ];
        let spots = hotspots(&entries);
        let latest = |file: &str| {
            spots.iter().find(|h| h["file"] == file).unwrap()["latestFindings"].as_u64().unwrap()
        };
        // a.rs was never re-screened: still open. b.rs was, and is clean.
        assert_eq!(latest("a.rs"), 1);
        assert_eq!(latest("b.rs"), 0);
    }
}
