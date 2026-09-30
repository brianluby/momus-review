//! Content-addressed result cache for `system_one` requests
//! (docs/scaling.md §2). One work unit is one request, keyed by
//!
//! ```text
//! sha256(base_url, model, questions JSON, redacted state JSON)
//! ```
//!
//! and stored as one JSON file per key holding **the response only** —
//! never the request state or questions — so the cache is not sensitive
//! the way the report is. Writes go through `report_store::write_atomic`
//! (unique temp file + rename), so concurrent requests at
//! `MOMUS_CONCURRENCY` cannot leave a partial entry behind.
//!
//! Mutable aliases are resolved from live responses in each run, never from
//! a saved alias map. Pin a versioned model for a completely offline warm run.
//!
//! Failures are never cached (only successful responses are stored), and
//! every I/O here is best-effort: a read or write that fails costs a
//! re-request, never the review.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::adapters::report_store::write_atomic;

/// The local result cache. Clones of the client share one; the alias map
/// lock is held only for map reads and inserts, never across an await
/// (the same discipline as the redaction log).
#[derive(Debug)]
pub struct ResultCache {
    /// `None` = disabled (`--no-cache`): every lookup misses, stores no-op.
    dir: Option<PathBuf>,
    aliases: Arc<Mutex<BTreeMap<String, String>>>,
}

impl ResultCache {
    /// `MOMUS_CACHE_DIR`, or `reviews/cache` beside the report store.
    pub fn from_env() -> Self {
        let dir = std::env::var("MOMUS_CACHE_DIR")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("reviews").join("cache"));
        Self::open(dir)
    }

    /// The cache at `dir`. The first live response resolves aliases anew in every run.
    pub fn open(dir: PathBuf) -> Self {
        Self {
            dir: Some(dir),
            aliases: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// A disabled cache: lookups always miss and nothing is written.
    pub fn disabled() -> Self {
        Self {
            dir: None,
            aliases: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// The versioned id `requested` last resolved to, or `requested` itself
    /// while unknown (the first live call resolves it).
    pub fn resolve_model(&self, requested: &str) -> String {
        self.aliases
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(requested)
            .cloned()
            .unwrap_or_else(|| requested.to_string())
    }

    /// Records a live resolution for this run only.
    pub fn record_model(&self, requested: &str, resolved: &str) {
        if requested == resolved {
            return;
        }
        let mut aliases = self.aliases.lock().unwrap_or_else(|e| e.into_inner());
        if aliases
            .get(requested)
            .is_some_and(|known| known == resolved)
        {
            return;
        }
        aliases.insert(requested.to_string(), resolved.to_string());
    }

    /// Whether disk lookups and writes are enabled for this cache.
    pub fn enabled(&self) -> bool {
        self.dir.is_some()
    }

    /// The cached response for this unit, if a successful one was stored.
    /// `model` is the id the request would carry; it is resolved through
    /// the alias map first, so a pinned version never answers for an alias
    /// that has moved. A missing or corrupt entry is a miss.
    pub fn lookup(
        &self,
        base_url: &str,
        model: &str,
        state: &Value,
        questions: &Value,
    ) -> Option<Value> {
        let dir = self.dir.as_ref()?;
        let key = key(base_url, &self.resolve_model(model), state, questions);
        let text = std::fs::read_to_string(dir.join(format!("{key}.json"))).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Stores a successful response for this unit. `model` is the
    /// **resolved** id the response itself reported (not the requested
    /// alias), so the entry is filed where subsequent lookups for that
    /// model — alias-resolved or explicit — will find it. Best-effort: a
    /// failed write only costs a re-request.
    pub fn store(
        &self,
        base_url: &str,
        model: &str,
        state: &Value,
        questions: &Value,
        response: &Value,
    ) {
        let Some(dir) = &self.dir else { return };
        let key = key(base_url, model, state, questions);
        if let Ok(text) = serde_json::to_string_pretty(response) {
            let _ = write_atomic(&dir.join(format!("{key}.json")), &format!("{text}\n"));
        }
    }
}

/// The content address of one unit: sha256 over a canonical serialization
/// of everything that determines the answer — the server, the model, the
/// policy questions, and the redacted state. Any wording, policy,
/// redaction-rule, or model change produces a different key. `serde_json`
/// sorts object keys, so the serialization is deterministic.
fn key(base_url: &str, model: &str, state: &Value, questions: &Value) -> String {
    let unit = serde_json::json!({
        "base_url": base_url,
        "model": model,
        "questions": questions,
        "state": state,
    });
    // A `Value` has only string keys and writes to a String: cannot fail.
    let canonical = serde_json::to_string(&unit).expect("serialize a JSON value");
    hex(&Sha256::digest(canonical.as_bytes()))
}

/// Lowercase hex — the 64-character cache file stem.
fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const URL: &str = "http://127.0.0.1:9";
    const MODEL: &str = "jev-latest";

    fn state() -> Value {
        json!({ "file": { "path": "a.rs", "content": "pub fn a() -> u32 { 7 }\n" } })
    }

    fn questions() -> Value {
        json!({ "correctness": noul(), "category": choice() })
    }

    fn noul() -> Value {
        json!({ "type": "noul", "instructions": "any defect?", "criteria": {} })
    }

    fn choice() -> Value {
        json!({ "type": "choice", "instructions": "file role?", "criteria": { "domain": "core logic" } })
    }

    fn response(id: u32) -> Value {
        json!({ "model": "stub-1.0", "answers": { "correctness": { "noul": 0.1 } }, "unit": id })
    }

    fn cache() -> (tempfile::TempDir, ResultCache) {
        let dir = tempfile::tempdir().unwrap();
        let cache = ResultCache::open(dir.path().to_path_buf());
        (dir, cache)
    }

    #[test]
    fn the_key_is_sensitive_to_every_unit_input() {
        let base = key(URL, MODEL, &state(), &questions());
        // Deterministic: the same inputs address the same unit.
        assert_eq!(base, key(URL, MODEL, &state(), &questions()));

        let other_url = key("http://127.0.0.1:10", MODEL, &state(), &questions());
        let other_model = key(URL, "jev-1.13.0", &state(), &questions());
        let other_questions = key(URL, MODEL, &state(), &json!({ "correctness": noul() }));
        let other_state = key(
            URL,
            MODEL,
            &json!({ "file": { "path": "a.rs", "content": "pub fn a() -> u32 { 8 }\n" } }),
            &questions(),
        );
        let all = [base, other_url, other_model, other_questions, other_state];
        for (i, a) in all.iter().enumerate() {
            for (j, b) in all.iter().enumerate() {
                assert_eq!(a == b, i == j, "keys {i} and {j} conflate inputs");
            }
        }
    }

    #[test]
    fn a_stored_entry_round_trips_and_holds_the_response_only() {
        let (dir, cache) = cache();
        cache.store(URL, MODEL, &state(), &questions(), &response(1));

        assert_eq!(
            cache.lookup(URL, MODEL, &state(), &questions()),
            Some(response(1))
        );

        // One file per key, named by the 64-hex content address.
        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(entries.len(), 1, "exactly one file per key");
        let stem = entries[0].file_stem().unwrap().to_str().unwrap();
        assert_eq!(stem.len(), 64, "sha256 hex stem: {stem}");
        assert!(stem.chars().all(|c| c.is_ascii_hexdigit()));

        // The store holds keys and answers, never code or policy text: the
        // state content and the question wording must not appear anywhere.
        let body = std::fs::read_to_string(&entries[0]).unwrap();
        assert!(body.contains("stub-1.0"), "the response is stored: {body}");
        assert!(!body.contains("pub fn a"), "state content leaked: {body}");
        assert!(
            !body.contains("any defect?"),
            "question text leaked: {body}"
        );
    }

    #[test]
    fn a_corrupt_entry_is_a_miss_and_is_overwritten() {
        let (dir, cache) = cache();
        cache.store(URL, MODEL, &state(), &questions(), &response(1));
        let entry = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .next()
            .unwrap();
        std::fs::write(&entry, "{ not json").unwrap();

        assert_eq!(
            cache.lookup(URL, MODEL, &state(), &questions()),
            None,
            "corrupt entry misses"
        );
        cache.store(URL, MODEL, &state(), &questions(), &response(2));
        assert_eq!(
            cache.lookup(URL, MODEL, &state(), &questions()),
            Some(response(2))
        );
    }

    #[test]
    fn a_disabled_cache_never_answers_or_writes() {
        let cache = ResultCache::disabled();
        assert_eq!(cache.lookup(URL, MODEL, &state(), &questions()), None);
        cache.store(URL, MODEL, &state(), &questions(), &response(1));
        assert_eq!(cache.lookup(URL, MODEL, &state(), &questions()), None);
    }

    /// The alias mechanism: entries are stored under the versioned id the
    /// response reported, and lookups through the alias find them — across
    /// runs, after a fresh live resolution. When the alias moves, the pinned
    /// entries stop matching; they are never served for the new version.
    #[test]
    fn aliases_pin_the_resolved_model_version() {
        let (dir, cache) = cache();
        cache.record_model("jev-latest", "jev-1.13.0");
        cache.store(URL, "jev-1.13.0", &state(), &questions(), &response(1));

        // Same run, through the alias.
        assert_eq!(
            cache.lookup(URL, "jev-latest", &state(), &questions()),
            Some(response(1))
        );
        // Explicit versioned requests find it too.
        assert_eq!(
            cache.lookup(URL, "jev-1.13.0", &state(), &questions()),
            Some(response(1))
        );

        // A fresh run must observe a live response before trusting an alias.
        let reopened = ResultCache::open(dir.path().to_path_buf());
        assert_eq!(reopened.resolve_model("jev-latest"), "jev-latest");
        assert_eq!(
            reopened.lookup(URL, "jev-latest", &state(), &questions()),
            None
        );

        // The alias silently moves: the 1.13.0 entry must not answer.
        reopened.record_model("jev-latest", "jev-1.14.0");
        assert_eq!(
            reopened.lookup(URL, "jev-latest", &state(), &questions()),
            None
        );
        // ...and answers under the new version are filed separately.
        reopened.store(URL, "jev-1.14.0", &state(), &questions(), &response(2));
        assert_eq!(
            reopened.lookup(URL, "jev-latest", &state(), &questions()),
            Some(response(2))
        );
    }

    /// An identity resolution (server echoes the requested id) records
    /// nothing: there is no alias to pin.
    #[test]
    fn a_versioned_model_records_no_alias() {
        let (dir, cache) = cache();
        cache.record_model("jev-1.13.0", "jev-1.13.0");
        assert!(!dir.path().join("models.json").exists());
        assert_eq!(cache.resolve_model("jev-1.13.0"), "jev-1.13.0");
    }

    /// Concurrent lookups and stores share the cache safely: every entry
    /// written in parallel reads back whole, and racing writers of one key
    /// leave one complete file (the temp+rename discipline), never a
    /// partial one.
    #[test]
    fn concurrent_stores_leave_every_entry_readable() {
        let (_dir, cache) = cache();
        let cache = Arc::new(cache);
        let handles: Vec<_> = (0..8)
            .map(|t| {
                let cache = Arc::clone(&cache);
                std::thread::spawn(move || {
                    for i in 0..25 {
                        let id = t * 25 + i;
                        let state = json!({ "unit": id });
                        cache.store(URL, MODEL, &state, &questions(), &response(id as u32));
                        assert!(cache.lookup(URL, MODEL, &state, &questions()).is_some());
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("no thread panicked");
        }
        for id in 0..(8 * 25) {
            let state = json!({ "unit": id });
            assert_eq!(
                cache.lookup(URL, MODEL, &state, &questions()),
                Some(response(id as u32)),
                "unit {id} corrupted"
            );
        }

        // Racing writers of the same key: whichever rename lands last wins
        // wholesale; the entry always parses as exactly one response.
        let writers: Vec<_> = (0..8)
            .map(|w| {
                let cache = Arc::clone(&cache);
                std::thread::spawn(move || {
                    cache.store(URL, MODEL, &state(), &questions(), &response(w));
                })
            })
            .collect();
        for writer in writers {
            writer.join().expect("no thread panicked");
        }
        let hit = cache
            .lookup(URL, MODEL, &state(), &questions())
            .expect("entry present");
        assert!(hit["unit"].as_u64().unwrap() < 8, "a whole response: {hit}");
    }
}
