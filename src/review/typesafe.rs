//! Thin HTTP client for the TypeSafe System One API.
//!
//! `jev-sdk` 0.1.0 cannot express the prototype's structured question
//! criteria (its `NoulCriteria`/`Choice`/`Score` criteria are string-only,
//! while the prototype sends JSON objects such as `{ what, examples }` and
//! `not_for`). Per `docs/rust-port.md` Step 0, the underlying API is plain
//! HTTP, so the port speaks the wire format directly and matches the
//! TypeScript SDK's `POST /v1/systemone` exactly.

use std::time::Duration;

use anyhow::{Result, bail};
use serde::Deserialize;
use serde_json::{Map, Value, json};

const DEFAULT_BASE_URL: &str = "https://api.typesafe.ai";
const DEFAULT_MODEL: &str = "jev-latest";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_RETRIES: usize = 3;

/// A `system_one` response: the model id plus answers keyed by question name.
#[derive(Debug, Deserialize)]
pub struct SystemOneResponse {
    pub model: String,
    pub answers: Map<String, Value>,
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

/// A client for the TypeSafe API, constructed from the environment.
#[derive(Clone)]
pub struct TypeSafeClient {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
    model: String,
}

impl TypeSafeClient {
    /// Reads `TYPESAFE_API_KEY` (required), `TYPESAFE_BASE_URL`, and
    /// `TYPESAFE_DEFAULT_MODEL` from the environment.
    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("TYPESAFE_API_KEY")
            .map_err(|_| anyhow::anyhow!("TYPESAFE_API_KEY is not set"))
            .and_then(|k| {
                if k.trim().is_empty() {
                    Err(anyhow::anyhow!("TYPESAFE_API_KEY is empty"))
                } else {
                    Ok(k)
                }
            })?;
        let base_url = std::env::var("TYPESAFE_BASE_URL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        let model = std::env::var("TYPESAFE_DEFAULT_MODEL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());

        let http = reqwest::Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .build()?;
        Ok(Self { http, api_key, base_url, model })
    }

    /// Evaluates one `system_one` request: a state plus a map of named
    /// questions. Retries transient failures — connection/timeout errors and
    /// `429`/`529`/`5xx` responses — with backoff.
    pub async fn system_one(&self, state: Value, questions: Value) -> Result<SystemOneResponse> {
        let url = format!("{}/v1/systemone", self.base_url);
        let body = json!({ "state": state, "questions": questions, "model": self.model });

        for attempt in 0..=MAX_RETRIES {
            let resp = match self
                .http
                .post(&url)
                .bearer_auth(&self.api_key)
                .json(&body)
                .send()
                .await
            {
                Ok(resp) => resp,
                Err(e) if is_retryable_transport(&e) && attempt < MAX_RETRIES => {
                    backoff(attempt).await;
                    continue;
                }
                Err(e) => return Err(e.into()),
            };

            let status = resp.status();
            if status.is_success() {
                return Ok(resp.json().await?);
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

/// Builds a `choice` criteria map from a name→description slice, preserving
/// order (the prototype keeps `noIssue` last).
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