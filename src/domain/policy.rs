//! Review policy: thresholds, limits, and the vocabulary of concerns the
//! reviewer screens for. Pure data. Mirrors `domain/config.ts`; file
//! patterns live in `domain/language.rs`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::domain::language::Language;

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

// ---- Vocabularies (per-dimension mechanisms; rubrics; owners) ---------

/// `mechanisms[d]`: the per-dimension `choice` vocabulary (name → description).
/// The order here is the reading order this vocabulary is documented in (the
/// `noIssue` sentinel last); it is *not* preserved on the wire, because the
/// criteria map serializes through `serde_json::Map` — see
/// `review::typesafe::choice_criteria`.
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

/// The language-specific mechanism additions, keyed by language + dimension.
///
/// These are the footguns that only exist in one language's idioms — Rust
/// `unsafe`/`unwrap`, TS `any`/casts, C memory safety, Go's ignored `error`,
/// shell word splitting. `mechanisms_for` splices them ahead of the `other`
/// sentinel, so the generic per-dimension entries stay available as fallbacks
/// and `other`/`noIssue` keep their position.
///
/// A key is shared across languages only when it means exactly the same thing
/// in both (`useAfterFree` for C and C++, `nonNullAssertion` for TS/Kotlin/
/// Swift/Dart) — and a shared key carries one description, not one per
/// language. That is what lets `explain::mechanism_title` stay keyed on
/// dimension + key, and what keeps the single SARIF rule for
/// `{dimension}/{mechanism}` from rendering one finding's help text for
/// another's. Dimensions where a language adds nothing are
/// omitted rather than padded: `testGap` is deliberately language-free — its
/// generic `branch`/`failure`/`boundary`/`integration` entries already name
/// every test gap worth flagging.
const LANGUAGE_MECHANISMS: &[(Language, Dimension, &[(&str, &str)])] = &[
    // ---- Rust (the ticket's `unsafe`/`unwrap`/`panic!`) ----
    (
        Language::Rust,
        Dimension::Correctness,
        &[
            ("unsafeBlock", "An `unsafe` block relies on an invariant the code does not establish or check"),
            ("unwrapPanic", "A fallible call is unwrapped, panicking on an unexpected `None` or `Err`"),
            ("panicPath", "`panic!`/`todo!`/`unreachable!` is reachable from a normal path"),
        ],
    ),
    (
        Language::Rust,
        Dimension::Reliability,
        &[("lockPoisoning", "A poisoned lock is ignored, so work continues on inconsistent state")],
    ),
    // ---- TypeScript ----
    (
        Language::TypeScript,
        Dimension::Correctness,
        &[
            ("anyEscape", "`any` (explicit or inferred) discards the types the rest of the code relies on"),
            ("uncheckedCast", "A cast is applied without validating the runtime type"),
            ("nonNullAssertion", "A non-null assertion or forced unwrap skips a real null check"),
        ],
    ),
    (
        Language::TypeScript,
        Dimension::Security,
        &[
            ("prototypePollution", "Untrusted keys are merged into an object, reaching its prototype"),
            ("dynamicCodeExecution", "Untrusted data is executed as code by an eval or shell-out primitive"),
        ],
    ),
    (
        Language::TypeScript,
        Dimension::Reliability,
        &[("unawaitedPromise", "A promise or future is created but never awaited or returned, so its failure is unobserved")],
    ),
    // ---- JavaScript ----
    (
        Language::JavaScript,
        Dimension::Correctness,
        &[
            ("looseEquality", "A loose `==`/`!=` comparison coerces operands instead of comparing them"),
            ("thisBinding", "A callback loses or rebinds `this`, so the wrong object is used"),
            ("implicitGlobal", "A missing declaration creates or clobbers a global"),
        ],
    ),
    (
        Language::JavaScript,
        Dimension::Security,
        &[
            ("prototypePollution", "Untrusted keys are merged into an object, reaching its prototype"),
            ("dynamicCodeExecution", "Untrusted data is executed as code by an eval or shell-out primitive"),
        ],
    ),
    (
        Language::JavaScript,
        Dimension::Reliability,
        &[("unawaitedPromise", "A promise or future is created but never awaited or returned, so its failure is unobserved")],
    ),
    // ---- Python ----
    (
        Language::Python,
        Dimension::Correctness,
        &[
            ("mutableDefaultArgument", "A mutable default argument is created once and shared across calls"),
            ("lateBindingClosure", "A closure captures a loop variable by reference and sees its last value"),
        ],
    ),
    (
        Language::Python,
        Dimension::Security,
        &[
            ("dynamicCodeExecution", "Untrusted data is executed as code by an eval or shell-out primitive"),
            ("assertForValidation", "An input or authorization check is enforced only by `assert`, which optimized builds remove"),
        ],
    ),
    (
        Language::Python,
        Dimension::Reliability,
        &[
            ("swallowedException", "A caught or rescued error is discarded instead of handled"),
            ("resourceLeak", "A resource is not released on every path (no scoped cleanup)"),
        ],
    ),
    // ---- Java ----
    (
        Language::Java,
        Dimension::Correctness,
        &[
            ("nullDereference", "A possibly-null value is dereferenced without a check"),
            ("equalsHashContract", "`equals`/`hashCode`/`compareTo` disagree, so hash collections misbehave"),
        ],
    ),
    (
        Language::Java,
        Dimension::Security,
        &[("unsafeReflection", "Reflection or a dynamic class load is driven by untrusted input")],
    ),
    (
        Language::Java,
        Dimension::Reliability,
        &[
            ("swallowedException", "A caught or rescued error is discarded instead of handled"),
            ("resourceLeak", "A resource is not released on every path (no scoped cleanup)"),
        ],
    ),
    (
        Language::Java,
        Dimension::Compatibility,
        &[("serializedFormChange", "A change to a serializable class breaks previously persisted data")],
    ),
    // ---- C# ----
    (
        Language::CSharp,
        Dimension::Correctness,
        &[
            ("nullDereference", "A possibly-null value is dereferenced without a check"),
            ("structCopyMutation", "A struct is mutated through a copy, discarding the change"),
        ],
    ),
    (
        Language::CSharp,
        Dimension::Reliability,
        &[
            ("asyncVoid", "An `async void` method cannot be awaited, so its failure escapes the caller"),
            ("resourceLeak", "A resource is not released on every path (no scoped cleanup)"),
        ],
    ),
    // ---- C ----
    (
        Language::C,
        Dimension::Correctness,
        &[
            ("uncheckedReturn", "A return value that signals failure or a short read is ignored"),
            ("pointerArithmetic", "Pointer or index arithmetic can step outside the object it addresses"),
        ],
    ),
    (
        Language::C,
        Dimension::Security,
        &[
            ("bufferOverflow", "A fixed-size buffer is written past its bounds"),
            ("formatString", "Untrusted text is used as a format string"),
            ("useAfterFree", "Memory is used after its lifetime ends"),
        ],
    ),
    (
        Language::C,
        Dimension::Reliability,
        &[("uncheckedAllocation", "An allocation result is used without checking for failure")],
    ),
    (
        Language::C,
        Dimension::Compatibility,
        &[("abiChange", "A struct layout, signature, or calling convention changes incompatibly")],
    ),
    // ---- C++ ----
    (
        Language::Cpp,
        Dimension::Correctness,
        &[
            ("uninitializedUse", "A value is read before it is initialized"),
            ("uncheckedReturn", "A return value that signals failure or a short read is ignored"),
        ],
    ),
    (
        Language::Cpp,
        Dimension::Security,
        &[
            ("useAfterFree", "Memory is used after its lifetime ends"),
            ("outOfBoundsAccess", "A container or array element is accessed outside its bounds"),
        ],
    ),
    (
        Language::Cpp,
        Dimension::Reliability,
        &[
            ("danglingReference", "A reference or iterator outlives the object it addresses"),
            ("exceptionEscapeDestructor", "An exception escapes a destructor or another `noexcept` boundary"),
        ],
    ),
    (
        Language::Cpp,
        Dimension::Compatibility,
        &[("abiChange", "A struct layout, signature, or calling convention changes incompatibly")],
    ),
    // ---- Go ----
    (
        Language::Go,
        Dimension::Correctness,
        &[
            ("ignoredError", "A returned error is discarded instead of handled"),
            ("nilMapOrChannel", "A nil map, channel, or interface value is written to or selected on"),
        ],
    ),
    (
        Language::Go,
        Dimension::Security,
        &[("unescapedTemplate", "Untrusted data is emitted through a raw or unescaped template value")],
    ),
    (
        Language::Go,
        Dimension::Reliability,
        &[
            ("goroutineLeak", "A goroutine blocks forever because nothing can unblock it"),
            ("deferInLoop", "A `defer` inside a loop runs only when the function returns"),
        ],
    ),
    // ---- PHP ----
    (
        Language::Php,
        Dimension::Correctness,
        &[
            ("looseEquality", "A loose `==`/`!=` comparison coerces operands instead of comparing them"),
            ("truthinessCoercion", "A value's truthiness or string coercion selects the wrong branch"),
        ],
    ),
    (
        Language::Php,
        Dimension::Security,
        &[
            ("fileInclusion", "A user-influenced path reaches `include`/`require`"),
            ("massAssignment", "Request parameters are assigned to a model without an allowlist"),
        ],
    ),
    (
        Language::Php,
        Dimension::Reliability,
        &[("errorSuppression", "The `@` operator hides a failure the caller must see")],
    ),
    // ---- Ruby ----
    (
        Language::Ruby,
        Dimension::Correctness,
        &[
            ("nilMethodCall", "A method is called on a value that may be `nil`"),
            ("mutationOfArgument", "An argument or shared object is mutated in place, surprising its owner"),
        ],
    ),
    (
        Language::Ruby,
        Dimension::Security,
        &[
            ("massAssignment", "Request parameters are assigned to a model without an allowlist"),
            ("dynamicCodeExecution", "Untrusted data is executed as code by an eval or shell-out primitive"),
        ],
    ),
    (
        Language::Ruby,
        Dimension::Reliability,
        &[("swallowedException", "A caught or rescued error is discarded instead of handled")],
    ),
    // ---- Kotlin ----
    (
        Language::Kotlin,
        Dimension::Correctness,
        &[
            ("nonNullAssertion", "A non-null assertion or forced unwrap skips a real null check"),
            ("platformTypeNullness", "A Java platform type is treated as non-null without a check"),
            ("uncheckedCast", "A cast is applied without validating the runtime type"),
        ],
    ),
    (
        Language::Kotlin,
        Dimension::Reliability,
        &[("blockingInAsyncContext", "A blocking call inside an async execution context stalls the threads it shares")],
    ),
    // ---- Swift ----
    (
        Language::Swift,
        Dimension::Correctness,
        &[
            ("nonNullAssertion", "A non-null assertion or forced unwrap skips a real null check"),
            ("forcedCast", "A forced cast (`as!`) traps instead of handling a mismatch"),
            ("silentOptionalChain", "An optional chain short-circuits and the failure is never handled"),
        ],
    ),
    (
        Language::Swift,
        Dimension::Reliability,
        &[
            ("retainCycle", "A closure captures `self` strongly, keeping the object alive"),
            ("forcedTry", "`try!` traps instead of propagating a thrown error"),
        ],
    ),
    // ---- Shell ----
    (
        Language::Shell,
        Dimension::Correctness,
        &[("subshellStateLoss", "State changed inside a subshell or pipeline does not reach the caller")],
    ),
    (
        Language::Shell,
        Dimension::Security,
        &[
            ("unquotedExpansion", "An unquoted expansion splits or globs, so untrusted text becomes extra arguments"),
            ("dynamicCodeExecution", "Untrusted data is executed as code by an eval or shell-out primitive"),
        ],
    ),
    (
        Language::Shell,
        Dimension::Reliability,
        &[
            ("ignoredExitStatus", "A failing command's exit status is ignored and the script continues"),
            ("partialPipelineFailure", "Only the last command of a pipeline is checked"),
        ],
    ),
    // ---- SQL ----
    (
        Language::Sql,
        Dimension::Correctness,
        &[
            ("missingWhereClause", "An `UPDATE`/`DELETE` affects every row because a predicate is missing or vacuous"),
            ("nullComparison", "A comparison against `NULL` never matches as intended"),
        ],
    ),
    (
        Language::Sql,
        Dimension::Security,
        &[("overbroadGrant", "A role or grant is broader than the operation needs")],
    ),
    (
        Language::Sql,
        Dimension::Compatibility,
        &[("destructiveMigration", "A migration drops or rewrites data that existing rows or callers cannot survive")],
    ),
    // ---- R ----
    (
        Language::R,
        Dimension::Correctness,
        &[
            ("vectorRecycling", "Vectors of unequal length are combined, silently recycling values"),
            ("naPropagation", "A missing value propagates through the result unchecked"),
        ],
    ),
    // ---- Scala ----
    (
        Language::Scala,
        Dimension::Correctness,
        &[
            ("platformTypeNullness", "A Java platform type is treated as non-null without a check"),
            ("uncheckedCast", "A cast is applied without validating the runtime type"),
        ],
    ),
    (
        Language::Scala,
        Dimension::Reliability,
        &[("blockingInAsyncContext", "A blocking call inside an async execution context stalls the threads it shares")],
    ),
    // ---- Dart ----
    (
        Language::Dart,
        Dimension::Correctness,
        &[
            ("nonNullAssertion", "A non-null assertion or forced unwrap skips a real null check"),
            ("uncheckedCast", "A cast is applied without validating the runtime type"),
        ],
    ),
    (
        Language::Dart,
        Dimension::Reliability,
        &[("unawaitedPromise", "A promise or future is created but never awaited or returned, so its failure is unobserved")],
    ),
    // ---- Lua ----
    (
        Language::Lua,
        Dimension::Correctness,
        &[
            ("implicitGlobal", "A missing declaration creates or clobbers a global"),
            ("nilArithmetic", "A field or upvalue that may be `nil` is used in arithmetic or indexing"),
        ],
    ),
    // ---- PowerShell ----
    (
        Language::PowerShell,
        Dimension::Correctness,
        &[("truthinessCoercion", "A value's truthiness or string coercion selects the wrong branch")],
    ),
    (
        Language::PowerShell,
        Dimension::Security,
        &[
            ("unquotedExpansion", "An unquoted expansion splits or globs, so untrusted text becomes extra arguments"),
            ("dynamicCodeExecution", "Untrusted data is executed as code by an eval or shell-out primitive"),
        ],
    ),
    (
        Language::PowerShell,
        Dimension::Reliability,
        &[("silentErrorContinuation", "`-ErrorAction SilentlyContinue` or a widened preference hides a failed command")],
    ),
];

/// The mechanism additions for one language and dimension (`&[]` when the
/// language adds nothing there).
pub fn language_mechanisms(d: Dimension, language: Language) -> &'static [(&'static str, &'static str)] {
    LANGUAGE_MECHANISMS
        .iter()
        .find(|(lang, dimension, _)| *lang == language && *dimension == d)
        .map_or(&[], |(_, _, entries)| *entries)
}

/// Every `(language, dimension, entries)` row, for vocabulary-wide checks.
pub fn language_mechanism_rows() -> &'static [(Language, Dimension, &'static [(&'static str, &'static str)])] {
    LANGUAGE_MECHANISMS
}

/// The full `choice` vocabulary for a dimension: the generic entries plus the
/// file's language-specific ones. Additions are spliced ahead of the `other`
/// sentinel, so the returned slice keeps the established reading order
/// (generic entries, then the language's, then `other`/`noIssue`); the wire
/// map is keyed, not ordered. Pass `None` when the file's language is
/// unknown.
pub fn mechanisms_for(d: Dimension, language: Option<Language>) -> Vec<(&'static str, &'static str)> {
    let generic = mechanisms(d);
    let additions = language.map_or(&[][..], |lang| language_mechanisms(d, lang));
    if additions.is_empty() {
        return generic.to_vec();
    }
    let sentinel = generic
        .iter()
        .position(|(key, _)| *key == "other")
        .unwrap_or(generic.len());
    let mut entries = Vec::with_capacity(generic.len() + additions.len());
    entries.extend_from_slice(&generic[..sentinel]);
    entries.extend_from_slice(additions);
    entries.extend_from_slice(&generic[sentinel..]);
    entries
}

/// The description for a mechanism key as it applies to `language`, searching
/// the language's vocabulary first and falling back to the generic entries
/// (a report written before the language vocabulary existed, or a finding
/// whose language was inferred differently).
pub fn mechanism_description(
    d: Dimension,
    language: Option<Language>,
    key: &str,
) -> Option<&'static str> {
    if let Some(lang) = language {
        if let Some((_, description)) = language_mechanisms(d, lang)
            .iter()
            .find(|(candidate, _)| *candidate == key)
        {
            return Some(description);
        }
    }
    mechanisms(d)
        .iter()
        .find(|(candidate, _)| *candidate == key)
        .map(|(_, description)| *description)
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
    use crate::domain::language::languages;

    /// The full vocabulary of a dimension for every language: generic keys
    /// plus whatever that language adds.
    fn vocabulary(d: Dimension, language: Language) -> Vec<(&'static str, &'static str)> {
        mechanisms_for(d, Some(language))
    }

    /// The `other`/`noIssue` sentinels close every vocabulary — they are what
    /// the criteria map and the classifier's fallback depend on, so language
    /// additions must never land after them.
    #[test]
    fn language_additions_splice_before_the_sentinels() {
        for language in languages() {
            for d in DIMENSIONS {
                let entries = vocabulary(d, language);
                let keys: Vec<&str> = entries.iter().map(|(key, _)| *key).collect();
                let tail = &keys[keys.len() - 2..];
                assert_eq!(tail, ["other", "noIssue"], "{language:?} {d:?}");
                for (key, _) in mechanisms(d) {
                    assert!(keys.contains(key), "{language:?} {d:?} dropped {key}");
                }
            }
        }
        // An unknown language degrades to the generic vocabulary exactly.
        assert_eq!(mechanisms_for(Dimension::Security, None), mechanisms(Dimension::Security).to_vec());
    }

    /// One key per vocabulary, and a language addition may not shadow a
    /// generic entry (a duplicate key would make the model's `choice` and the
    /// description lookup ambiguous).
    #[test]
    fn language_keys_are_unique_and_do_not_shadow_generic_ones() {
        for language in languages() {
            for d in DIMENSIONS {
                let mut keys: Vec<&str> = Vec::new();
                for (key, description) in vocabulary(d, language) {
                    assert!(!description.is_empty(), "{language:?} {d:?} {key}");
                    assert!(!keys.contains(&key), "duplicate {language:?} {d:?}::{key}");
                    keys.push(key);
                }
            }
        }
    }

    /// Every supported language carries at least one mechanism, so adding a
    /// language forces a deliberate vocabulary decision rather than silently
    /// inheriting the generic list.
    #[test]
    fn every_language_contributes_mechanisms() {
        for language in languages() {
            let total: usize = DIMENSIONS
                .iter()
                .map(|d| language_mechanisms(*d, language).len())
                .sum();
            assert!(total > 0, "{language:?} adds no mechanisms");
        }
    }

    /// The ticket's motivating examples, pinned: Rust `unsafe`/`unwrap`/
    /// `panic!` and the TS `any`/cast pair reach the classifier's vocabulary
    /// for a `.rs` / `.ts` file.
    #[test]
    fn rust_and_typescript_mechanisms_reach_the_vocabulary() {
        let rust: Vec<&str> = vocabulary(Dimension::Correctness, Language::Rust)
            .iter()
            .map(|(key, _)| *key)
            .collect();
        assert!(rust.contains(&"unsafeBlock"));
        assert!(rust.contains(&"unwrapPanic"));
        assert!(rust.contains(&"panicPath"));

        let ts: Vec<&str> = vocabulary(Dimension::Correctness, Language::TypeScript)
            .iter()
            .map(|(key, _)| *key)
            .collect();
        assert!(ts.contains(&"anyEscape"));
        assert!(ts.contains(&"uncheckedCast"));
    }

    /// One key, one definition. The SARIF rule id is
    /// `{dimension}/{mechanism}` with a single shared help text, so a key
    /// whose prose varied by language would render one finding's help for
    /// another's; `why` must not depend on which finding came first either.
    #[test]
    fn each_mechanism_key_has_one_definition() {
        for d in DIMENSIONS {
            let mut seen: Vec<(&str, &str)> = mechanisms(d).to_vec();
            for (_, _, entries) in language_mechanism_rows()
                .iter()
                .filter(|(_, dimension, _)| *dimension == d)
            {
                for (key, description) in *entries {
                    match seen.iter().find(|(seen_key, _)| seen_key == key) {
                        Some((_, first)) => assert_eq!(
                            first, description,
                            "{d:?}::{key} has more than one definition"
                        ),
                        None => seen.push((key, description)),
                    }
                }
            }
        }
    }

    #[test]
    fn mechanism_description_finds_generic_and_language_keys() {
        assert_eq!(
            mechanism_description(Dimension::Security, Some(Language::C), "bufferOverflow"),
            Some("A fixed-size buffer is written past its bounds")
        );
        // Generic keys stay reachable with a language set.
        assert_eq!(
            mechanism_description(Dimension::Security, Some(Language::C), "sqlInjection"),
            mechanisms(Dimension::Security)
                .iter()
                .find(|(key, _)| *key == "sqlInjection")
                .map(|(_, description)| *description)
        );
        // A language that does not define the key falls back to generic, and
        // an unknown key resolves to nothing rather than an empty description.
        assert!(mechanism_description(Dimension::Correctness, Some(Language::Go), "anyEscape").is_none());
        assert!(mechanism_description(Dimension::Correctness, None, "nope").is_none());
    }
}