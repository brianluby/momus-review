//! Review policy: thresholds, limits, file patterns, and the vocabulary of
//! concerns the reviewer screens for. Pure data. Mirrors `domain/config.ts`.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

// The five screening dimensions. Serialized as camelCase so `testGap` matches
// the wire key the dashboard reads (and the `matrix` row keys).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "camelCase")]
pub enum Dimension {
    Correctness,
    Security,
    Reliability,
    Compatibility,
    TestGap,
}

pub const DIMENSIONS: [Dimension; 5] = [
    Dimension::Correctness,
    Dimension::Security,
    Dimension::Reliability,
    Dimension::Compatibility,
    Dimension::TestGap,
];

impl Dimension {
    /// The wire key (`testGap`, `correctness`, …), used for logs.
    pub fn key(self) -> &'static str {
        match self {
            Dimension::Correctness => "correctness",
            Dimension::Security => "security",
            Dimension::Reliability => "reliability",
            Dimension::Compatibility => "compatibility",
            Dimension::TestGap => "testGap",
        }
    }

    /// The one-line concern definition (`config.ts` `dimensions` record),
    /// carried into `locate` state as `suspectedConcern.definition`.
    pub fn definition(self) -> &'static str {
        match self {
            Dimension::Correctness => "The code likely contains incorrect runtime behavior.",
            Dimension::Security => "The code introduces or weakens a security boundary.",
            Dimension::Reliability => {
                "The code can cause a crash, race, leak, deadlock, or poor failure recovery."
            }
            Dimension::Compatibility => {
                "The code can break a caller, persisted format, protocol, or public behavior."
            }
            Dimension::TestGap => "Important behavior lacks adequate targeted test evidence.",
        }
    }
}

/// `Record<Dimension, number>` — the per-file screening probability matrix.
pub type Probabilities = BTreeMap<Dimension, f64>;

/// `dimensionMetadata`: `{ key: Dimension; label: string; short: string }`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DimensionMeta {
    pub key: Dimension,
    pub label: String,
    pub short: String,
}

pub fn dimension_metadata() -> Vec<DimensionMeta> {
    vec![
        DimensionMeta { key: Dimension::Correctness, label: "Correctness".into(), short: "Corr".into() },
        DimensionMeta { key: Dimension::Security, label: "Security".into(), short: "Sec".into() },
        DimensionMeta { key: Dimension::Reliability, label: "Reliability".into(), short: "Rel".into() },
        DimensionMeta { key: Dimension::Compatibility, label: "Compatibility".into(), short: "Compat".into() },
        DimensionMeta { key: Dimension::TestGap, label: "Test gap".into(), short: "Tests".into() },
    ]
}

// Screening signals at or above this probability are followed up.
pub const SCREEN_THRESHOLD: f64 = 0.7;
// Severity is scored on a 0–3 rubric; the dashboard mirrors this ceiling.
pub const SEVERITY_MAX: f64 = 3.0;
// Findings at or above this severity are routed to a reviewer.
pub const ROUTE_SEVERITY: f64 = 1.5;
// Findings at or above this severity request changes instead of a comment.
pub const BLOCKING_SEVERITY: f64 = 2.0;
// Minimum confidence for an evidence-hunk selection to count.
pub const MIN_LOCATION_CONFIDENCE: f64 = 0.55;
// Minimum meta-judge probability for a located finding to survive the second
// skeptical pass; below this the finding is dropped as an unsupported claim.
pub const MIN_META_JUDGE_CONFIDENCE: f64 = 0.55;

/// The prototype's follow-up cap (top-8). The CLI defaults to unlimited
/// follow-ups and offers `--follow-ups N` to re-impose a cap; this constant
/// documents the original budget for reference.
pub const MAX_FOLLOW_UPS: usize = 8;
pub const MAX_PROFILES: usize = 5;
pub const CONCURRENCY: usize = 3;

// The prototype uses two regex literals. `regex::Regex` is not `const`-safe,
// so they become `LazyLock` statics with the initializer at the declaration.
static SOURCE_FILE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\.(?:[cm]?[jt]sx?|rs)$").expect("valid regex")
});

static TEST_FILE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|/)(?:tests?|__tests__)(?:/|$)|\.(?:spec|test)\.[cm]?[jt]sx?$")
        .expect("valid regex")
});

pub fn source_file() -> &'static Regex {
    &SOURCE_FILE
}

pub fn test_file() -> &'static Regex {
    &TEST_FILE
}

// ---- Vocabularies (per-dimension mechanisms; rubrics; owners) ---------

/// `mechanisms[d]`: the per-dimension `choice` vocabulary (name → description).
/// Order is preserved so the `noIssue` sentinel always sorts last in the wire
/// criteria map, matching the prototype.
pub fn mechanisms(d: Dimension) -> &'static [(&'static str, &'static str)] {
    match d {
        Dimension::Correctness => &[
            ("condition", "A condition handles the wrong cases"),
            ("state", "State is read, updated, or retained incorrectly"),
            ("dataFlow", "Data is transformed or passed incorrectly"),
            ("asyncControl", "Asynchronous ordering or error handling is incorrect"),
            ("other", "Another concrete correctness mechanism"),
            ("noIssue", "The selected evidence does not support a concrete correctness issue"),
        ],
        Dimension::Security => &[
            ("brokenAccessControl", "A user can access or change data or actions they are not authorized for"),
            ("brokenAuthentication", "Authentication can be bypassed, or an identity/session is forged"),
            ("sqlInjection", "Untrusted input reaches a SQL query or command"),
            ("noSqlInjection", "Untrusted input reaches a NoSQL query or command"),
            ("commandInjection", "Untrusted input reaches a shell, process, or OS command"),
            ("xss", "Untrusted input is output as markup or script"),
            ("xxe", "XML parsing resolves external entities from untrusted input"),
            ("ssrf", "A server-side request is attacker-influenced (internal or remote)"),
            ("pathTraversal", "A file or URL path is attacker-influenced and not contained"),
            ("insecureDeserialization", "Serialized data is deserialized without validation"),
            ("cryptographicFailure", "Cryptography is missing, weak, or misused"),
            ("sensitiveDataExposure", "Sensitive data is disclosed or unnecessarily exposed"),
            ("securityMisconfiguration", "A default, header, permission, or deployment setting creates avoidable exposure"),
            ("other", "Another concrete security mechanism"),
            ("noIssue", "The selected evidence does not support a concrete security issue"),
        ],
        Dimension::Reliability => &[
            ("cleanup", "A resource or side effect is not cleaned up"),
            ("concurrency", "Concurrency can race, deadlock, or lose work"),
            ("recovery", "Failure or cancellation recovery is incomplete"),
            ("crash", "A realistic path can throw or terminate unexpectedly"),
            ("other", "Another concrete reliability mechanism"),
            ("noIssue", "The selected evidence does not support a concrete reliability issue"),
        ],
        Dimension::Compatibility => &[
            ("api", "A public API or type contract changes incompatibly"),
            ("behavior", "Existing callers observe changed behavior"),
            ("dataFormat", "A persisted or exchanged format changes incompatibly"),
            ("protocol", "An external command or protocol contract changes"),
            ("other", "Another concrete compatibility mechanism"),
            ("noIssue", "The selected evidence does not support a concrete compatibility issue"),
        ],
        Dimension::TestGap => &[
            ("branch", "An important branch lacks targeted coverage"),
            ("failure", "A failure or cancellation path lacks coverage"),
            ("boundary", "A boundary or edge case lacks coverage"),
            ("integration", "An interaction between components lacks coverage"),
            ("other", "Another concrete test gap"),
            ("noIssue", "The selected evidence does not support a concrete test gap"),
        ],
    }
}

/// `reviewPriorityRubric` — ordered score levels (index = score, 0→3).
pub const REVIEW_PRIORITY_RUBRIC: [&str; 4] = [
    "Routine review is sufficient",
    "A focused review of the changed behavior is useful",
    "Careful review is needed before merge",
    "Specialist or immediate review is needed",
];

/// `severityRubric` — ordered severity levels (index = score, 0→3).
pub const SEVERITY_RUBRIC: [&str; 4] = [
    "No meaningful impact or no supported issue",
    "Minor or narrowly limited impact",
    "Significant correctness, reliability, compatibility, or security impact",
    "Critical security, data-loss, or widespread outage impact",
];

/// `owners` — reviewer routing vocabulary (label → description).
pub const OWNERS: [(&str, &str); 5] = [
    ("security", "Security, authentication, authorization, or data exposure"),
    ("api", "Public APIs, compatibility, schemas, or protocols"),
    ("runtime", "Execution, concurrency, resources, or failure recovery"),
    ("testing", "Coverage strategy, fixtures, or regression testing"),
    ("maintainer", "The owning domain or feature maintainer"),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins the discovery contract from `docs/language-support.md`: the
    /// `tests/`/`__tests__/` directory half is language-agnostic; the
    /// `.(spec|test).` file half is JS/TS-only. This is what gives the
    /// `testGap` dimension its related-test context.
    #[test]
    fn test_file_matches_rust_test_dir_only() {
        assert!(test_file().is_match("tests/lib_test.rs"));
        assert!(test_file().is_match("__tests__/foo.rs"));
        assert!(test_file().is_match("src/__tests__/helpers.ts"));
        assert!(!test_file().is_match("src/lib.rs"));
        assert!(!test_file().is_match("testing_util.rs"));
    }

    #[test]
    fn test_file_js_suffix_only_for_js() {
        assert!(test_file().is_match("foo.spec.ts"));
        assert!(test_file().is_match("foo.test.js"));
        // `foo_test.go` has no `tests/` dir and the suffix half is JS-only.
        assert!(!test_file().is_match("foo_test.go"));
    }

    #[test]
    fn source_file_matches_rust_and_ts() {
        assert!(source_file().is_match("src/lib.rs"));
        assert!(source_file().is_match("a/file.tsx"));
        assert!(source_file().is_match("a/file.mjs"));
        assert!(!source_file().is_match("a/file.py"));
        assert!(!source_file().is_match("a/file.go"));
    }
}