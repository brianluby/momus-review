//! Content-addressed result cache for `system_one` requests
//! (docs/scaling.md §2). One work unit is one request, keyed by
//!
//! ```text
//! sha256(base_url, model, questions JSON, redacted state JSON)
//! ```
//!
//! and stored as one JSON file per key holding the supplied response. The
//! adapter does not add request state or questions to that file, but a response
//! can itself contain source text or secrets. Use caller-controlled storage
//! with the same access policy as the response. Writes use
//! `report_store::write_atomic`, so racing
//! successful writes replace the entry as a whole.
//!
//! `TypeSafeClient` resolves an unknown requested ID ending in `latest` from a
//! live response each run; no alias map is saved. Explicit stable IDs can reuse
//! entries without that initial alias resolution. Missing or invalid entries
//! still require a live request in the client.
//!
//! This adapter accepts arbitrary JSON; it does not validate a response's
//! answers, model identity or authenticity. `TypeSafeClient` stores validated
//! successful answers under a stable response model, checks cached answers and
//! model identity, and makes a live request on a miss or invalid response.
//! Malformed JSON and I/O errors are misses here. The filename hashes the
//! request inputs, not the stored response: valid but edited JSON is not
//! detected by this adapter. Best-effort I/O does not turn the cache into a
//! source of authenticated review evidence.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::adapters::report_store::write_atomic;

/// Response storage keyed by exact request inputs, with a run-local model map.
///
/// Clients can share this value through an `Arc`. The alias-map lock is held
/// only while reading or updating the map; filesystem operations do not hold
/// it. Neither opening a directory nor reading an entry authenticates its
/// owner or contents.
#[derive(Debug)]
pub struct ResultCache {
    /// `None` = disabled (`--no-cache`): every lookup misses, stores no-op.
    dir: Option<PathBuf>,
    aliases: Arc<Mutex<BTreeMap<String, String>>>,
}

impl ResultCache {
    /// Use nonblank `MOMUS_CACHE_DIR`, otherwise the relative `reviews/cache` path.
    ///
    /// Construction performs no I/O and does not create or verify the directory.
    /// A nonblank environment value is used as supplied, without trimming it.
    pub fn from_env() -> Self {
        let dir = std::env::var("MOMUS_CACHE_DIR")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("reviews").join("cache"));
        Self::open(dir)
    }

    /// Enable storage at `dir` with an empty, in-memory alias map.
    ///
    /// This performs no I/O. Previously stored entries remain available for
    /// explicit model identities, but alias resolutions are not loaded from
    /// disk. The caller must choose the directory and establish which returned
    /// model identities are stable enough to reuse.
    pub fn open(dir: PathBuf) -> Self {
        Self {
            dir: Some(dir),
            aliases: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// Disable disk access: every lookup misses and every store is a no-op.
    ///
    /// The in-memory alias map still works, allowing a client to track a live
    /// response's identity without persisting answers. This example uses no
    /// filesystem or network:
    ///
    /// ```
    /// use momus_review::adapters::cache::ResultCache;
    /// use serde_json::json;
    ///
    /// let cache = ResultCache::disabled();
    /// assert!(!cache.enabled());
    /// assert_eq!(cache.resolve_model("model-latest"), "model-latest");
    /// cache.record_model("model-latest", "model-v1");
    /// assert_eq!(cache.resolve_model("model-latest"), "model-v1");
    /// let state = json!({"file": "example.rs"});
    /// let questions = json!({"correctness": "assess"});
    /// cache.store("https://example.invalid", "model-v1", &state, &questions, &json!({"answer": 1}));
    /// assert!(cache.lookup("https://example.invalid", "model-latest", &state, &questions).is_none());
    /// ```
    pub fn disabled() -> Self {
        Self {
            dir: None,
            aliases: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// Return the last recorded resolution, or the unchanged requested string.
    ///
    /// This is a map lookup, not a server query or verification that an ID is
    /// versioned. A fresh cache instance knows no aliases. `TypeSafeClient`
    /// obtains a live response before reusing an unresolved `latest` alias.
    pub fn resolve_model(&self, requested: &str) -> String {
        self.aliases
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(requested)
            .cloned()
            .unwrap_or_else(|| requested.to_string())
    }

    /// Replace a requested ID's run-local mapping with a caller-supplied ID.
    ///
    /// No producer or version validation occurs here. Equal strings are a no-op
    /// and do not clear an existing mapping; nothing is persisted to disk.
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

    /// Report whether a storage path was configured, not whether it is usable.
    ///
    /// A missing or unwritable directory can still return `true`; lookup/store
    /// handle subsequent I/O failures as a miss or ignored write.
    pub fn enabled(&self) -> bool {
        self.dir.is_some()
    }

    /// Read JSON addressed by URL, resolved model, state and questions.
    ///
    /// `model` is resolved through the run-local map; the other inputs are
    /// serialized exactly as supplied. Disabled storage, an unreadable file,
    /// invalid UTF-8 or invalid JSON returns `None`. Any parseable JSON value
    /// returns `Some`, including a value with the wrong model or answer shape.
    /// The caller must validate those fields before using a hit. Reading follows
    /// ordinary filesystem links and does not authenticate storage contents.
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

    /// Serialize the supplied response under the exact supplied model ID.
    ///
    /// Unlike [`lookup`](Self::lookup), this does not resolve aliases. Pass a
    /// stable ID established from a validated response; this method itself
    /// checks neither successful-answer semantics nor correspondence between
    /// `model` and `response`. Disabled storage and write failures are silent
    /// no-ops. Only the response is serialized, but it can contain sensitive
    /// data copied into that response by the producer.
    ///
    /// Request input changes select different entries. Reordering JSON object
    /// keys preserves the address under this crate's serialization settings:
    ///
    /// ```
    /// use momus_review::adapters::cache::ResultCache;
    /// use serde_json::json;
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let directory = tempfile::tempdir()?;
    /// let cache = ResultCache::open(directory.path().to_path_buf());
    /// let state = json!({"file": "example.rs", "revision": 1});
    /// let questions = json!({"correctness": "assess"});
    /// let response = json!({"answer": 0.2});
    /// let url = "https://example.invalid";
    /// cache.store(url, "model-v1", &state, &questions, &response);
    /// let reordered = json!({"revision": 1, "file": "example.rs"});
    /// assert_eq!(cache.lookup(url, "model-v1", &reordered, &questions), Some(response));
    /// assert!(cache.lookup("https://other.invalid", "model-v1", &state, &questions).is_none());
    /// assert!(cache.lookup(url, "model-v2", &state, &questions).is_none());
    /// assert!(cache.lookup(url, "model-v1", &json!({"revision": 2}), &questions).is_none());
    /// assert!(cache.lookup(url, "model-v1", &state, &json!({"security": "assess"})).is_none());
    /// # Ok(())
    /// # }
    /// ```
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

/// Hash the serialized URL string, model string, questions and state.
/// Object keys are sorted by the crate's `serde_json` configuration. Inputs
/// such as credentials or redaction-rule identity are not separate key fields;
/// a rule change affects the address only when it changes supplied JSON.
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
