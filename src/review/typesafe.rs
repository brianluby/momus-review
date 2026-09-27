//! Thin HTTP client for the TypeSafe System One API.
//!
//! `jev-sdk` 0.1.0 cannot express the prototype's structured question
//! criteria (its `NoulCriteria`/`Choice`/`Score` criteria are string-only,
//! while the prototype sends JSON objects such as `{ what, examples }` and
//! `not_for`). Per `docs/rust-port.md` Step 0, the underlying API is plain
//! HTTP, so the port speaks the wire format directly and matches the
//! TypeScript SDK's `POST /v1/systemone` exactly.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use std::time::Duration;

use anyhow::{Result, bail};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::domain::language::Language;
use crate::domain::policy::{Dimension, mechanisms_for};
use crate::domain::report::UsageSummary;

const DEFAULT_BASE_URL: &str = "https://api.typesafe.ai";
const DEFAULT_MODEL: &str = "jev-latest";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_RETRIES: usize = 3;

/// Token usage attached to a `system_one` response. Winnow sends it on every
/// response; hosted Jev omits the block.
#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Usage {
    /// Winnow sends snake_case; a camelCase server would otherwise read as 0.
    #[serde(alias = "inputTokens")]
    pub input_tokens: u64,
    #[serde(alias = "outputTokens")]
    pub output_tokens: u64,
}

/// A `system_one` response: the model id plus answers keyed by question name.
#[derive(Debug, Deserialize)]
pub struct SystemOneResponse {
    pub model: String,
    pub answers: Map<String, Value>,
    #[serde(default)]
    pub usage: Option<Usage>,
}

impl SystemOneResponse {
    fn answer(&self, id: &str) -> Result<&Value> {
        self.answers
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("missing answer '{id}'"))
    }

    /// The probability for a `noul` question.
    pub fn noul(&self, id: &str) -> Result<f64> {
        self.answer(id)?
            .get("noul")
            .and_then(Value::as_f64)
            .ok_or_else(|| anyhow::anyhow!("answer '{id}' missing .noul"))
    }

    /// The selected label + confidence for a `choice` question.
    pub fn choice(&self, id: &str) -> Result<(String, f64)> {
        let a = self.answer(id)?;
        let label = a
            .get("choice")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("answer '{id}' missing .choice"))?
            .to_string();
        let confidence = a
            .get("confidence")
            .and_then(Value::as_f64)
            .ok_or_else(|| anyhow::anyhow!("answer '{id}' missing .confidence"))?;
        Ok((label, confidence))
    }

    /// The score + confidence for a `score` question.
    pub fn score(&self, id: &str) -> Result<(f64, f64)> {
        let a = self.answer(id)?;
        let score = a
            .get("score")
            .and_then(Value::as_f64)
            .ok_or_else(|| anyhow::anyhow!("answer '{id}' missing .score"))?;
        let confidence = a
            .get("confidence")
            .and_then(Value::as_f64)
            .ok_or_else(|| anyhow::anyhow!("answer '{id}' missing .confidence"))?;
        Ok((score, confidence))
    }
}

/// Client settings resolved from the environment.
#[derive(Debug, PartialEq)]
struct ClientConfig {
    api_key: Option<String>,
    base_url: String,
    model: String,
    timeout: Duration,
}

impl ClientConfig {
    /// Resolves settings through `lookup` (the environment in production).
    /// The API key is required unless the base URL is a loopback address, so
    /// a local System One server (e.g. Winnow) runs without one.
    fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let var = |name: &str| lookup(name).filter(|s| !s.trim().is_empty());

        let base_url = var("TYPESAFE_BASE_URL")
            .map(|s| s.trim().trim_end_matches('/').to_string())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        let api_key = var("TYPESAFE_API_KEY");
        if api_key.is_none() && !is_loopback(&base_url) {
            bail!("TYPESAFE_API_KEY is not set (required unless TYPESAFE_BASE_URL is a local server)");
        }
        let model = var("TYPESAFE_DEFAULT_MODEL").unwrap_or_else(|| DEFAULT_MODEL.to_string());
        let timeout = match var("TYPESAFE_TIMEOUT_SECS") {
            Some(raw) => Duration::from_secs(parse_positive("TYPESAFE_TIMEOUT_SECS", &raw)? as u64),
            None => DEFAULT_TIMEOUT,
        };
        Ok(Self { api_key, base_url, model, timeout })
    }
}

/// Whether `url` points at this machine (`localhost`, `127.0.0.0/8`, `::1`).
fn is_loopback(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else { return false };
    let Some(host) = parsed.host_str() else { return false };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// Parses a positive integer setting, naming the variable on error.
pub fn parse_positive(name: &str, raw: &str) -> Result<usize> {
    match raw.trim().parse::<usize>() {
        Ok(n) if n > 0 => Ok(n),
        _ => bail!("{name} must be a positive integer, got '{raw}'"),
    }
}

/// A client for the TypeSafe API, constructed from the environment.
#[derive(Clone)]
pub struct TypeSafeClient {
    http: reqwest::Client,
    api_key: Option<String>,
    base_url: String,
    model: String,
    usage: Arc<UsageMeter>,
}

impl TypeSafeClient {
    /// Reads `TYPESAFE_API_KEY`, `TYPESAFE_BASE_URL`, `TYPESAFE_DEFAULT_MODEL`,
    /// and `TYPESAFE_TIMEOUT_SECS` from the environment. The key is required
    /// unless the base URL is a local server.
    pub fn from_env() -> Result<Self> {
        let ClientConfig { api_key, base_url, model, timeout } =
            ClientConfig::from_lookup(|name| std::env::var(name).ok())?;
        let http = reqwest::Client::builder().timeout(timeout).build()?;
        Ok(Self {
            http,
            api_key,
            base_url,
            model,
            usage: Arc::new(UsageMeter::default()),
        })
    }

    /// Evaluates one `system_one` request: a state plus a map of named
    /// questions. Retries transient failures — connection/timeout errors and
    /// `429`/`529`/`5xx` responses — with backoff.
    pub async fn system_one(&self, state: Value, questions: Value) -> Result<SystemOneResponse> {
        let url = format!("{}/v1/systemone", self.base_url);
        let body = json!({ "state": state, "questions": questions, "model": self.model });

        for attempt in 0..=MAX_RETRIES {
            let mut request = self.http.post(&url).json(&body);
            if let Some(key) = &self.api_key {
                request = request.bearer_auth(key);
            }
            let resp = match request.send().await {
                Ok(resp) => resp,
                Err(e) if is_retryable_transport(&e) && attempt < MAX_RETRIES => {
                    backoff(attempt).await;
                    continue;
                }
                Err(e) => return Err(e.into()),
            };

            let status = resp.status();
            if status.is_success() {
                let resp: SystemOneResponse = resp.json().await?;
                self.usage.record(resp.usage);
                return Ok(resp);
            }

            let text = resp.text().await.unwrap_or_default();
            if is_retryable_status(status.as_u16()) && attempt < MAX_RETRIES {
                backoff(attempt).await;
                continue;
            }
            bail!("system_one failed ({status}): {text}");
        }
        unreachable!("retry loop always returns or errors")
    }
}

impl TypeSafeClient {
    /// Cumulative usage of every successful `system_one` call this client
    /// has made (shared across clones, so the whole review reports one total).
    pub fn usage_summary(&self) -> UsageSummary {
        self.usage.summary()
    }
}

/// Per-run usage counters. Atomics, not a mutex: `record` runs on the async
/// request path after every successful call and must not block the executor.
#[derive(Debug, Default)]
struct UsageMeter {
    calls: AtomicU64,
    input_tokens: AtomicU64,
    output_tokens: AtomicU64,
}

impl UsageMeter {
    /// Adds one successful response. Servers that omit `usage` (hosted Jev)
    /// still count the call; token sums stay unchanged.
    fn record(&self, usage: Option<Usage>) {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if let Some(usage) = usage {
            self.input_tokens.fetch_add(usage.input_tokens, Ordering::Relaxed);
            self.output_tokens.fetch_add(usage.output_tokens, Ordering::Relaxed);
        }
    }

    fn summary(&self) -> UsageSummary {
        UsageSummary {
            calls: self.calls.load(Ordering::Relaxed),
            input_tokens: self.input_tokens.load(Ordering::Relaxed),
            output_tokens: self.output_tokens.load(Ordering::Relaxed),
        }
    }
}

/// Transient server statuses worth retrying: rate limit, overloaded, and
/// 5xx server errors.
fn is_retryable_status(code: u16) -> bool {
    matches!(code, 429 | 500 | 502 | 503 | 504 | 529)
}

/// Connection and timeout failures are transient; retry them.
fn is_retryable_transport(e: &reqwest::Error) -> bool {
    e.is_connect() || e.is_timeout()
}

async fn backoff(attempt: usize) {
    tokio::time::sleep(Duration::from_millis(500 * (1 << attempt))).await;
}

// ---- Question builders (wire types, mirroring the TS SDK) ---------------

/// `noul(instructions, criteria)` — criteria `{ true: .., false: .. }`.
pub fn noul(instructions: Value, criteria: Value) -> Value {
    json!({ "type": "noul", "instructions": instructions, "criteria": criteria })
}

/// `choice(instructions, criteria)` — criteria is a label→description map.
pub fn choice(instructions: Value, criteria: Value) -> Value {
    json!({ "type": "choice", "instructions": instructions, "criteria": criteria })
}

/// `score(instructions, criteria)` — criteria is an ordered rubric array.
pub fn score(instructions: Value, criteria: Value) -> Value {
    json!({ "type": "score", "instructions": instructions, "criteria": criteria })
}

/// The mechanism `choice` criteria for `dimension` over the file at `path`:
/// the generic per-dimension vocabulary plus the mechanisms of the file's own
/// language (#7). One home for the path → language → vocabulary mapping, so
/// both review modes send the classifier the same criteria for a given file.
pub fn mechanism_criteria(path: &str, dimension: Dimension) -> Value {
    choice_criteria(&mechanisms_for(dimension, Language::from_path(path)))
}

/// Builds a `choice` criteria map from a name→description slice.
///
/// The map does **not** preserve insertion order: this build of `serde_json`
/// has no `preserve_order`, so keys serialize alphabetically and the
/// `noIssue` sentinel is not last on the wire (probe: `{"a","b","noIssue",
/// "other"}` in that order serializes as `a, b, noIssue, other`). Vocabulary
/// order in `policy` documents reading order only; nothing may depend on the
/// order the model receives.
pub fn choice_criteria(entries: &[(&str, &str)]) -> Value {
    let mut map = Map::new();
    for (label, desc) in entries {
        map.insert((*label).to_string(), Value::String((*desc).to_string()));
    }
    Value::Object(map)
}

/// Builds a `score` criteria array from an ordered rubric of levels.
pub fn score_criteria(levels: &[&str]) -> Value {
    Value::Array(levels.iter().map(|l| Value::String((*l).to_string())).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(vars: &[(&str, &str)]) -> Result<ClientConfig> {
        ClientConfig::from_lookup(|name| {
            vars.iter().find(|(k, _)| *k == name).map(|(_, v)| (*v).to_string())
        })
    }

    #[test]
    fn hosted_api_requires_a_key() {
        assert!(config(&[]).is_err());
        assert!(config(&[("TYPESAFE_API_KEY", "  ")]).is_err());
        let c = config(&[("TYPESAFE_API_KEY", "k")]).unwrap();
        assert_eq!(c.api_key.as_deref(), Some("k"));
        assert_eq!(c.base_url, DEFAULT_BASE_URL);
        assert_eq!(c.model, DEFAULT_MODEL);
        assert_eq!(c.timeout, DEFAULT_TIMEOUT);
    }

    #[test]
    fn local_server_runs_without_a_key() {
        let c = config(&[("TYPESAFE_BASE_URL", "http://127.0.0.1:8091/")]).unwrap();
        assert_eq!(c.api_key, None);
        assert_eq!(c.base_url, "http://127.0.0.1:8091");
    }

    #[test]
    fn remote_base_url_still_requires_a_key() {
        assert!(config(&[("TYPESAFE_BASE_URL", "http://gpu-box.lan:8091")]).is_err());
    }

    #[test]
    fn loopback_detection() {
        assert!(is_loopback("http://localhost:8091"));
        assert!(is_loopback("http://127.0.0.2"));
        assert!(is_loopback("http://[::1]:8091"));
        assert!(!is_loopback("https://api.typesafe.ai"));
        assert!(!is_loopback("http://localhost.example.com"));
        assert!(!is_loopback("not a url"));
    }

    /// Winnow attaches `usage` to every response; hosted Jev omits it. Both
    /// parse, and the meter counts calls either way, summing tokens only when
    /// they are reported.
    #[test]
    fn usage_is_optional_and_accumulates() {
        let winnow: SystemOneResponse = serde_json::from_str(
            r#"{ "model": "Winnow-12B", "answers": {},
                "usage": { "input_tokens": 112, "output_tokens": 3 } }"#,
        )
        .unwrap();
        let hosted: SystemOneResponse =
            serde_json::from_str(r#"{ "model": "jev-latest", "answers": {} }"#).unwrap();

        let meter = UsageMeter::default();
        meter.record(winnow.usage);
        meter.record(hosted.usage);
        meter.record(Some(Usage { input_tokens: 8, output_tokens: 0 }));

        let totals = meter.summary();
        assert_eq!(totals.calls, 3);
        assert_eq!(totals.input_tokens, 120);
        assert_eq!(totals.output_tokens, 3);
    }

    /// A server that reports camelCase token keys still meters (serde alias).
    #[test]
    fn usage_accepts_camel_case_keys() {
        let resp: SystemOneResponse = serde_json::from_str(
            r#"{ "model": "x", "answers": {},
                "usage": { "inputTokens": 10, "outputTokens": 4 } }"#,
        )
        .unwrap();
        assert_eq!(resp.usage, Some(Usage { input_tokens: 10, output_tokens: 4 }));
    }

    #[test]
    fn timeout_is_configurable_and_validated() {
        let local = ("TYPESAFE_BASE_URL", "http://localhost:8091");
        let c = config(&[local, ("TYPESAFE_TIMEOUT_SECS", "300")]).unwrap();
        assert_eq!(c.timeout, Duration::from_secs(300));
        assert!(config(&[local, ("TYPESAFE_TIMEOUT_SECS", "0")]).is_err());
        assert!(config(&[local, ("TYPESAFE_TIMEOUT_SECS", "soon")]).is_err());
    }

    /// The chain that gives #7 its teeth: a file's path selects a vocabulary,
    /// and the classifier sees the language's own mechanisms *plus* the
    /// generic entries and the `noIssue` sentinel.
    #[test]
    fn mechanism_criteria_carry_the_file_languages_vocabulary() {
        let keys = |value: &Value| -> Vec<String> {
            value.as_object().expect("criteria is a map").keys().cloned().collect()
        };

        let rust = keys(&mechanism_criteria("src/lib.rs", Dimension::Correctness));
        assert!(rust.contains(&"unsafeBlock".to_string()));
        assert!(rust.contains(&"unwrapPanic".to_string()));
        // Generic entries and the sentinel survive alongside them.
        assert!(rust.contains(&"condition".to_string()));
        assert!(rust.contains(&"noIssue".to_string()));

        let go = keys(&mechanism_criteria("cmd/main.go", Dimension::Correctness));
        assert!(go.contains(&"ignoredError".to_string()));
        assert!(
            !go.contains(&"unsafeBlock".to_string()),
            "another language's vocabulary leaked into a Go file"
        );

        let python = keys(&mechanism_criteria("pkg/app.py", Dimension::Security));
        assert!(python.contains(&"dynamicCodeExecution".to_string()));
        assert!(python.contains(&"sqlInjection".to_string()));

        // A language we do not review still gets a usable generic vocabulary.
        let unknown = keys(&mechanism_criteria("README.md", Dimension::Correctness));
        assert!(unknown.contains(&"condition".to_string()));
        assert!(!unknown.contains(&"unsafeBlock".to_string()));
    }
}
