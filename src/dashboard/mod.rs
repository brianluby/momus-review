//! Local-only dashboard server: three allowlisted static assets plus the saved
//! review report as JSON. Binds to loopback only. Mirrors `dashboard/server.ts`
//! with the same Host/CSP/nosniff hardening.

use axum::extract::State;
use axum::http::{HeaderValue, Request, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router, routing::get};
use serde_json::{Value, json};

use crate::adapters::report_store::{StoredReport, read_report, report_path};

pub const DEFAULT_PORT: u16 = 4317;

const INDEX_HTML: &str = include_str!("public/index.html");
const STYLE_CSS: &str = include_str!("public/style.css");
const APP_JS: &str = include_str!("public/app.js");

/// Serves the dashboard on `127.0.0.1:<port>` until the process is killed.
pub async fn serve(port: u16) -> anyhow::Result<()> {
    let router = Router::new()
        .route("/api/review", get(api_review))
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