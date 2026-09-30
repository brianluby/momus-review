//! Thin HTTP client for the TypeSafe System One API.
//!
//! `jev-sdk` 0.1.0 cannot express momus's structured question criteria (its
//! `NoulCriteria`/`Choice`/`Score` criteria are string-only, while momus
//! sends JSON objects such as `{ what, examples }` and `not_for`). The API is
//! plain HTTP, so this client speaks the `POST /v1/systemone` wire format
//! directly (see `docs/implementation.md`).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use std::time::Duration;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::adapters::cache::ResultCache;
use crate::domain::language::Language;
use crate::domain::policy::{Dimension, mechanisms_for};
use crate::domain::redact::{RedactionLog, Redactions, redact_value};
use crate::domain::report::{CacheSummary, UsageSummary};

const DEFAULT_BASE_URL: &str = "https://api.typesafe.ai";
const DEFAULT_MODEL: &str = "jev-latest";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_RETRIES: usize = 3;

/// Token usage attached to a `system_one` response. Both hosted Jev and
/// Winnow report it on every response (measured 2026-09-29, ticket #32);
/// `Option` keeps a server that omits the block parseable.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Usage {
    /// Winnow sends snake_case; a camelCase server would otherwise read as 0.
    #[serde(alias = "inputTokens")]
    pub input_tokens: u64,
    #[serde(alias = "outputTokens")]
    pub output_tokens: u64,
}

/// A `system_one` response: the model id plus answers keyed by question name.
#[derive(Debug, Serialize, Deserialize)]
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
    /// Redact secrets from every state before it is sent (`MOMUS_REDACT`).
    redact: bool,
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
            bail!(
                "TYPESAFE_API_KEY is not set (required unless TYPESAFE_BASE_URL is a local server)"
            );
        }
        let model = var("TYPESAFE_DEFAULT_MODEL").unwrap_or_else(|| DEFAULT_MODEL.to_string());
        let timeout = match var("TYPESAFE_TIMEOUT_SECS") {
            Some(raw) => Duration::from_secs(parse_positive("TYPESAFE_TIMEOUT_SECS", &raw)? as u64),
            None => DEFAULT_TIMEOUT,
        };
        let redact = match var("MOMUS_REDACT") {
            Some(raw) => parse_switch("MOMUS_REDACT", &raw)?,
            None => true,
        };
        Ok(Self {
            api_key,
            base_url,
            model,
            timeout,
            redact,
        })
    }
}

/// Parses an on/off setting, naming the variable on error.
fn parse_switch(name: &str, raw: &str) -> Result<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "on" | "yes" => Ok(true),
        "0" | "false" | "off" | "no" => Ok(false),
        _ => bail!("{name} must be on or off, got '{raw}'"),
    }
}

/// Whether `url` points at this machine (`localhost`, `127.0.0.0/8`, `::1`).
fn is_loopback(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    let Some(host) = parsed.host_str() else {
        return false;
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
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
    cache: Arc<ResultCache>,
    cache_hits: Arc<AtomicU64>,
    cache_misses: Arc<AtomicU64>,
    redact: bool,
    redactions: Arc<Mutex<RedactionLog>>,
}

impl TypeSafeClient {
    /// Reads `TYPESAFE_API_KEY`, `TYPESAFE_BASE_URL`, `TYPESAFE_DEFAULT_MODEL`,
    /// `TYPESAFE_TIMEOUT_SECS`, and `MOMUS_REDACT` from the environment. The
    /// key is required unless the base URL is a local server.
    pub fn from_env() -> Result<Self> {
        Ok(
            Self::from_config(ClientConfig::from_lookup(|name| std::env::var(name).ok())?)?
                .with_cache(ResultCache::from_env()),
        )
    }

    fn from_config(config: ClientConfig) -> Result<Self> {
        let ClientConfig {
            api_key,
            base_url,
            model,
            timeout,
            redact,
        } = config;
        let http = reqwest::Client::builder().timeout(timeout).build()?;
        Ok(Self {
            http,
            api_key,
            base_url,
            model,
            usage: Arc::new(UsageMeter::default()),
            cache: Arc::new(ResultCache::disabled()),
            cache_hits: Arc::new(AtomicU64::new(0)),
            cache_misses: Arc::new(AtomicU64::new(0)),
            redact,
            redactions: Arc::new(Mutex::new(RedactionLog::default())),
        })
    }

    /// Uses a local cache. Clones share model resolutions and counters.
    pub fn with_cache(mut self, cache: ResultCache) -> Self {
        self.cache = Arc::new(cache);
        self
    }

    /// Bypasses both reads and writes (`--no-cache`).
    pub fn without_cache(self) -> Self {
        self.with_cache(ResultCache::disabled())
    }

    pub fn cache_summary(&self) -> CacheSummary {
        CacheSummary {
            hits: self.cache_hits.load(Ordering::Relaxed),
            misses: self.cache_misses.load(Ordering::Relaxed),
        }
    }

    /// Turns off secret redaction (`--no-redact`): states are sent verbatim.
    pub fn without_redaction(mut self) -> Self {
        self.redact = false;
        self
    }

    /// Evaluates one `system_one` request: a state plus a map of named
    /// questions. Retries transient failures — connection/timeout errors and
    /// `429`/`529`/`5xx` responses — with backoff.
    ///
    /// Unless redaction is off, every string in `state` is scrubbed of
    /// secrets first (`domain::redact`); `questions` are policy text we own.
    pub async fn system_one(
        &self,
        mut state: Value,
        questions: Value,
    ) -> Result<SystemOneResponse> {
        if self.redact {
            let mut found = Redactions::default();
            redact_value(&mut state, &mut found);
            if !found.is_empty() {
                // Held only for a hash insert, never across an await.
                self.redactions
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .record(found);
            }
        }
        // Redaction precedes addressing: the key covers exactly the sent state.
        // An unresolved mutable alias must observe a live response this run.
        let resolved = self.cache.resolve_model(&self.model);
        let unresolved_alias = self.model.ends_with("latest") && resolved == self.model;
        let cached = if !unresolved_alias && self.cache.enabled() {
            let cache = self.cache.clone();
            let base_url = self.base_url.clone();
            let model = self.model.clone();
            let state = state.clone();
            let questions = questions.clone();
            tokio::task::spawn_blocking(move || cache.lookup(&base_url, &model, &state, &questions))
                .await
                .ok()
                .flatten()
        } else {
            None
        };
        if let Some(value) = cached
            && let Ok(mut response) = serde_json::from_value::<SystemOneResponse>(value)
            && response.model == resolved
            && valid_answers(&response, &questions)
        {
            response.usage = None;
            self.cache_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(response);
        }
        self.cache_misses.fetch_add(1, Ordering::Relaxed);
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
                if valid_answers(&resp, &questions) {
                    self.cache.record_model(&self.model, &resp.model);
                    if self.cache.enabled() && !resp.model.ends_with("latest") {
                        // Persist only the typed model and answers, never the
                        // server's extra fields, usage, questions, or state.
                        let cached = json!({ "model": resp.model, "answers": resp.answers });
                        let cache = self.cache.clone();
                        let base_url = self.base_url.clone();
                        let model = resp.model.clone();
                        let state = state.clone();
                        let questions = questions.clone();
                        let _ = tokio::task::spawn_blocking(move || {
                            cache.store(&base_url, &model, &state, &questions, &cached)
                        })
                        .await;
                    }
                }
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
        UsageSummary {
            cache: self.cache_summary(),
            ..self.usage.summary()
        }
    }

    /// Distinct secret values redacted per rule across the whole review.
    pub fn redaction_summary(&self) -> BTreeMap<String, usize> {
        self.redactions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .summary()
    }
}

/// Malformed or incomplete successful responses must not poison later runs.
fn valid_answers(response: &SystemOneResponse, questions: &Value) -> bool {
    !response.model.is_empty()
        && questions.as_object().is_some_and(|qs| {
            qs.iter().all(|(id, q)| match q["type"].as_str() {
                Some("noul") => response.noul(id).is_ok_and(|n| (0.0..=1.0).contains(&n)),
                Some("choice") => response
                    .choice(id)
                    .is_ok_and(|(_, c)| (0.0..=1.0).contains(&c)),
                Some("score") => response
                    .score(id)
                    .is_ok_and(|(n, c)| n.is_finite() && (0.0..=1.0).contains(&c)),
                _ => false,
            })
        })
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
    /// Adds one successful response; one without `usage` (none observed:
    /// hosted Jev reports it, ticket #32) still counts the call.
    fn record(&self, usage: Option<Usage>) {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if let Some(usage) = usage {
            self.input_tokens
                .fetch_add(usage.input_tokens, Ordering::Relaxed);
            self.output_tokens
                .fetch_add(usage.output_tokens, Ordering::Relaxed);
        }
    }

    fn summary(&self) -> UsageSummary {
        UsageSummary {
            calls: self.calls.load(Ordering::Relaxed),
            input_tokens: self.input_tokens.load(Ordering::Relaxed),
            output_tokens: self.output_tokens.load(Ordering::Relaxed),
            cache: CacheSummary::default(),
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
    Value::Array(
        levels
            .iter()
            .map(|l| Value::String((*l).to_string()))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(vars: &[(&str, &str)]) -> Result<ClientConfig> {
        ClientConfig::from_lookup(|name| {
            vars.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (*v).to_string())
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
        assert!(c.redact, "redaction is on by default");
    }

    #[test]
    fn redaction_can_be_switched_off() {
        let local = ("TYPESAFE_BASE_URL", "http://localhost:8091");
        assert!(!config(&[local, ("MOMUS_REDACT", "off")]).unwrap().redact);
        assert!(!config(&[local, ("MOMUS_REDACT", "0")]).unwrap().redact);
        assert!(config(&[local, ("MOMUS_REDACT", "ON")]).unwrap().redact);
        assert!(config(&[local, ("MOMUS_REDACT", "maybe")]).is_err());
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

    /// Hosted Jev and Winnow both attach `usage` to every response
    /// (ticket #32); a response without it still parses and counts as a
    /// call, summing tokens only when they are reported.
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
        meter.record(Some(Usage {
            input_tokens: 8,
            output_tokens: 0,
        }));

        let totals = meter.summary();
        assert_eq!(totals.calls, 3);
        assert_eq!(totals.input_tokens, 120);
        assert_eq!(totals.output_tokens, 3);
    }

    /// Starts a loopback System One stub that records each request body.
    async fn recording_server() -> (String, Arc<Mutex<Vec<Value>>>) {
        use axum::{Json, Router, routing::post};
        let bodies: Arc<Mutex<Vec<Value>>> = Arc::default();
        let sink = bodies.clone();
        let app = Router::new().route(
            "/v1/systemone",
            post(move |Json(body): Json<Value>| {
                let sink = sink.clone();
                async move {
                    sink.lock().unwrap().push(body);
                    Json(json!({ "model": "stub", "answers": {} }))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (url, bodies)
    }

    #[tokio::test]
    async fn cached_units_resume_without_billing_and_no_cache_bypasses() {
        let (url, bodies) = recording_server().await;
        let dir = tempfile::tempdir().unwrap();
        let make = || stub_client(&url, true).with_cache(ResultCache::open(dir.path().into()));
        let state = json!({ "unit": "AKIAIOSFODNN7EXAMPLE" });
        let client = make();
        client.system_one(state.clone(), json!({})).await.unwrap();
        client.system_one(state.clone(), json!({})).await.unwrap();
        assert_eq!(bodies.lock().unwrap().len(), 1);
        assert_eq!(client.usage_summary().calls, 1);
        assert_eq!(client.cache_summary(), CacheSummary { hits: 1, misses: 1 });
        assert_eq!(client.redaction_summary().values().sum::<usize>(), 1);
        // Pin the resolved model and reopen the directory like a fresh run.
        let mut resumed = make();
        resumed.model = "stub".into();
        resumed.system_one(state.clone(), json!({})).await.unwrap();
        assert_eq!(resumed.usage_summary().calls, 0);
        assert_eq!(resumed.cache_summary().hits, 1);
        resumed
            .without_cache()
            .system_one(state, json!({}))
            .await
            .unwrap();
        assert_eq!(bodies.lock().unwrap().len(), 2);
        // Changed state and policy must miss (empty question map stays valid).
        client
            .system_one(json!({ "unit": "changed" }), json!({}))
            .await
            .unwrap();
        client
            .system_one(
                json!({ "unit": "changed" }),
                json!({ "q": { "type": "noul" } }),
            )
            .await
            .unwrap();
        // Missing answers are never reused, despite a 200 response.
        client
            .system_one(
                json!({ "unit": "changed" }),
                json!({ "q": { "type": "noul" } }),
            )
            .await
            .unwrap();
        assert_eq!(bodies.lock().unwrap().len(), 5);
    }

    fn stub_client(url: &str, redact: bool) -> TypeSafeClient {
        let config = config(&[("TYPESAFE_BASE_URL", url)]).unwrap();
        TypeSafeClient::from_config(ClientConfig { redact, ..config }).unwrap()
    }

    /// The wire body carries placeholders, never the secret; questions pass
    /// through untouched; the report-facing summary counts distinct values.
    #[tokio::test]
    async fn secrets_never_reach_the_wire() {
        let (url, bodies) = recording_server().await;
        let client = stub_client(&url, true);
        let secret = "AKIAIOSFODNN7EXAMPLE";
        let state =
            json!({ "file": { "path": "a.ts", "content": format!("const k = \"{secret}\";\n") } });
        let questions = json!({ "q": noul(json!("mentions AKIAIOSFODNN7EXAMPLE?"), json!({})) });

        client
            .system_one(state.clone(), questions.clone())
            .await
            .unwrap();
        client.system_one(state, questions).await.unwrap();

        let sent = bodies.lock().unwrap();
        assert_eq!(sent.len(), 2);
        let wire = sent[0].to_string();
        assert!(
            !wire.contains(&format!("\"{secret}")),
            "secret left in state: {wire}"
        );
        assert_eq!(
            sent[0]["state"]["file"]["content"],
            "const k = \"<redacted:aws-access-key>\";\n"
        );
        assert_eq!(sent[0]["questions"], sent[1]["questions"]);
        assert_eq!(
            sent[0]["questions"]["q"]["instructions"],
            "mentions AKIAIOSFODNN7EXAMPLE?"
        );
        assert_eq!(
            client.redaction_summary(),
            BTreeMap::from([("aws-access-key".to_string(), 1)])
        );
    }

    #[tokio::test]
    async fn no_redact_sends_state_verbatim() {
        let (url, bodies) = recording_server().await;
        let client = stub_client(&url, true).without_redaction();
        let state = json!({ "content": "k = AKIAIOSFODNN7EXAMPLE" });
        client.system_one(state.clone(), json!({})).await.unwrap();
        assert_eq!(bodies.lock().unwrap()[0]["state"], state);
        assert!(client.redaction_summary().is_empty());
    }

    /// A server that reports camelCase token keys still meters (serde alias).
    #[test]
    fn usage_accepts_camel_case_keys() {
        let resp: SystemOneResponse = serde_json::from_str(
            r#"{ "model": "x", "answers": {},
                "usage": { "inputTokens": 10, "outputTokens": 4 } }"#,
        )
        .unwrap();
        assert_eq!(
            resp.usage,
            Some(Usage {
                input_tokens: 10,
                output_tokens: 4
            })
        );
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
            value
                .as_object()
                .expect("criteria is a map")
                .keys()
                .cloned()
                .collect()
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
