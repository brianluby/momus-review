//! Local-only dashboard server: three allowlisted static assets plus the saved
//! review report as JSON. Binds to loopback only. Mirrors `dashboard/server.ts`
//! with the same Host/CSP/nosniff hardening.

use axum::extract::State;
use axum::http::{HeaderValue, Request, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router, routing::get};
use serde_json::{Value, json};

use crate::adapters::report_store::{StoredReport, read_history, read_report, report_path};
use crate::domain::report::Action;

pub const DEFAULT_PORT: u16 = 4317;

const INDEX_HTML: &str = include_str!("public/index.html");
const STYLE_CSS: &str = include_str!("public/style.css");
const APP_JS: &str = include_str!("public/app.js");

/// Serves the dashboard on `127.0.0.1:<port>` until the process is killed.
pub async fn serve(port: u16) -> anyhow::Result<()> {
    let router = Router::new()
        .route("/api/review", get(api_review))
        .route("/api/history", get(api_history))
        .route("/", get(index).head(index))
        .route("/style.css", get(style_css).head(style_css))
        .route("/app.js", get(app_js).head(app_js))
        .fallback(not_found)
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

    // Chronological (ISO-8601 strings sort correctly).
    entries.sort_by(|a, b| a.saved_at.cmp(&b.saved_at));

    let latest = entries.last();

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
                "savedAt": e.saved_at,
                "findings": findings,
                "blocking": blocking,
                "maxSeverity": max_severity,
            })
        })
        .collect();

    // Aggregate every finding across all entries by file.
    use std::collections::BTreeMap;
    let mut by_file: BTreeMap<String, (usize, usize, usize, f64)> = BTreeMap::new();
    for entry in &entries {
        let is_latest = matches!(latest, Some(l) if std::ptr::eq(l, entry));
        for f in &entry.report.findings {
            let slot = by_file.entry(f.file.clone()).or_insert((0, 0, 0, 0.0));
            slot.0 += 1;
            if is_latest {
                slot.1 += 1;
            }
            if f.action == Action::RequestChanges {
                slot.2 += 1;
            }
            if f.severity > slot.3 {
                slot.3 = f.severity;
            }
        }
    }

    let mut hotspots: Vec<(String, usize, usize, usize, f64)> = by_file
        .into_iter()
        .map(|(file, (findings, latest_findings, blocking, max_severity))| {
            (file, findings, latest_findings, blocking, max_severity)
        })
        .collect();
    hotspots.sort_by_key(|h| std::cmp::Reverse(h.1));
    let hotspot_values: Vec<Value> = hotspots
        .into_iter()
        .take(15)
        .map(|(file, findings, latest_findings, blocking, max_severity)| {
            json!({
                "file": file,
                "findings": findings,
                "latestFindings": latest_findings,
                "blocking": blocking,
                "maxSeverity": max_severity,
            })
        })
        .collect();

    Ok(Json(json!({ "entries": entry_values, "hotspots": hotspot_values })))
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