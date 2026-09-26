//! Finding explanation + suggestion enrichment (title / why / fix / test).
//!
//! The TypeSafe/Jev API only exposes `noul`, `choice`, and `score`; it has no
//! free-text generation. So the enrich stage is split along that seam:
//!
//! * `title` and `why` are deterministic. `title` is a concise human label
//!   mapped from the already-classified mechanism, and `why` is the matching
//!   description from the per-dimension, per-language
//!   `policy::mechanisms_for` vocabulary the classifier chose from.
//! * `fix` and `test` are the only model work: two `choice` questions over
//!   curated, actionable strategy vocabularies, resolved in a single
//!   `system_one` call (see `enrich_suggestions`).
//!
//! The stage is capped at `MAX_ENRICH` findings so the model budget is
//! bounded even when a review surfaces many findings.

use anyhow::Result;

use serde_json::{Value, json};

use crate::domain::language::Language;
use crate::domain::policy::{Dimension, mechanism_description};
use crate::domain::report::Finding;
use crate::review::typesafe::{TypeSafeClient, choice, choice_criteria};

/// The ceiling on model-enriched findings: only the top-N findings by
/// severity get `fix`/`test` suggestions. `title`/`why` are deterministic
/// and applied to every finding regardless.
pub const MAX_ENRICH: usize = 8;

/// A concise human title for a mechanism key, used as the finding title.
/// Covers every key in the full per-language vocabulary
/// (`crate::domain::policy::mechanisms_for(..)` for each supported language);
/// `noIssue` maps to `None` (it is never an emitted finding) and each `other`
/// maps to the dimension's generic issue title.
pub fn mechanism_title(dimension: Dimension, key: &str) -> Option<&'static str> {
    match (dimension, key) {
        (Dimension::Correctness, "condition") => Some("Incorrect condition branch"),
        (Dimension::Correctness, "state") => Some("Incorrect state handling"),
        (Dimension::Correctness, "dataFlow") => Some("Incorrect data flow"),
        (Dimension::Correctness, "asyncControl") => Some("Incorrect async/error handling"),
        (Dimension::Correctness, "other") => Some("Correctness issue"),
        // Language-specific: Rust.
        (Dimension::Correctness, "unsafeBlock") => Some("Unsound `unsafe` block"),
        (Dimension::Correctness, "unwrapPanic") => Some("Panic on unwrapped failure"),
        (Dimension::Correctness, "panicPath") => Some("Reachable panic"),
        // TypeScript / Kotlin / Swift / Dart.
        (Dimension::Correctness, "anyEscape") => Some("`any` escapes the type system"),
        (Dimension::Correctness, "uncheckedCast") => Some("Unchecked cast"),
        (Dimension::Correctness, "nonNullAssertion") => Some("Non-null assertion skips a check"),
        (Dimension::Correctness, "forcedCast") => Some("Forced cast traps"),
        (Dimension::Correctness, "silentOptionalChain") => Some("Optional chain hides the failure"),
        (Dimension::Correctness, "platformTypeNullness") => Some("Platform-type nullness ignored"),
        // JavaScript / PHP / PowerShell.
        (Dimension::Correctness, "looseEquality") => Some("Loose equality comparison"),
        (Dimension::Correctness, "truthinessCoercion") => Some("Coercion picks the wrong branch"),
        (Dimension::Correctness, "thisBinding") => Some("Wrong `this` binding"),
        // Python / Lua.
        (Dimension::Correctness, "mutableDefaultArgument") => Some("Shared mutable default argument"),
        (Dimension::Correctness, "lateBindingClosure") => Some("Closure captures the loop variable"),
        (Dimension::Correctness, "implicitGlobal") => Some("Accidental global"),
        (Dimension::Correctness, "nilArithmetic") => Some("Nil used in arithmetic or indexing"),
        // Java / C# / Ruby.
        (Dimension::Correctness, "nullDereference") => Some("Null dereference"),
        (Dimension::Correctness, "equalsHashContract") => Some("Broken equality/hash contract"),
        (Dimension::Correctness, "structCopyMutation") => Some("Mutation lost on a struct copy"),
        (Dimension::Correctness, "nilMethodCall") => Some("Call on a possibly-nil value"),
        (Dimension::Correctness, "mutationOfArgument") => Some("In-place mutation of shared state"),
        // C / C++.
        (Dimension::Correctness, "uncheckedReturn") => Some("Ignored return value"),
        (Dimension::Correctness, "pointerArithmetic") => Some("Unsafe pointer arithmetic"),
        (Dimension::Correctness, "uninitializedUse") => Some("Use of an uninitialized value"),
        // Go.
        (Dimension::Correctness, "ignoredError") => Some("Ignored error return"),
        (Dimension::Correctness, "nilMapOrChannel") => Some("Nil map/channel misuse"),
        // Shell.
        (Dimension::Correctness, "subshellStateLoss") => Some("Subshell state is discarded"),
        // SQL.
        (Dimension::Correctness, "missingWhereClause") => Some("Missing or vacuous WHERE clause"),
        (Dimension::Correctness, "nullComparison") => Some("Comparison against NULL"),
        // R.
        (Dimension::Correctness, "vectorRecycling") => Some("Silent vector recycling"),
        (Dimension::Correctness, "naPropagation") => Some("Unchecked NA propagation"),
        (Dimension::Security, "brokenAccessControl") => Some("Broken access control"),
        (Dimension::Security, "brokenAuthentication") => Some("Broken authentication"),
        (Dimension::Security, "sqlInjection") => Some("SQL injection"),
        (Dimension::Security, "noSqlInjection") => Some("NoSQL injection"),
        (Dimension::Security, "commandInjection") => Some("Command injection"),
        (Dimension::Security, "xss") => Some("Cross-site scripting (XSS)"),
        (Dimension::Security, "xxe") => Some("XML external entity (XXE)"),
        (Dimension::Security, "ssrf") => Some("Server-side request forgery (SSRF)"),
        (Dimension::Security, "pathTraversal") => Some("Path traversal"),
        (Dimension::Security, "insecureDeserialization") => Some("Insecure deserialization"),
        (Dimension::Security, "cryptographicFailure") => Some("Cryptographic failure"),
        (Dimension::Security, "sensitiveDataExposure") => Some("Sensitive data exposure"),
        (Dimension::Security, "securityMisconfiguration") => Some("Security misconfiguration"),
        (Dimension::Security, "other") => Some("Security issue"),
        // JavaScript / TypeScript / Python / Ruby / Shell / PowerShell.
        (Dimension::Security, "dynamicCodeExecution") => Some("Dynamic code execution"),
        (Dimension::Security, "prototypePollution") => Some("Prototype pollution"),
        (Dimension::Security, "massAssignment") => Some("Mass assignment"),
        (Dimension::Security, "unquotedExpansion") => Some("Unquoted expansion (argument injection)"),
        // Python / Java.
        (Dimension::Security, "assertForValidation") => Some("Validation enforced by `assert`"),
        (Dimension::Security, "unsafeReflection") => Some("Untrusted reflection"),
        // C / C++ memory safety.
        (Dimension::Security, "bufferOverflow") => Some("Buffer overflow"),
        (Dimension::Security, "formatString") => Some("Format-string vulnerability"),
        (Dimension::Security, "useAfterFree") => Some("Use after free"),
        (Dimension::Security, "outOfBoundsAccess") => Some("Out-of-bounds access"),
        // Go / PHP / SQL.
        (Dimension::Security, "unescapedTemplate") => Some("Unescaped template output"),
        (Dimension::Security, "fileInclusion") => Some("Local/remote file inclusion"),
        (Dimension::Security, "overbroadGrant") => Some("Overbroad grant"),
        (Dimension::Reliability, "cleanup") => Some("Missing cleanup"),
        (Dimension::Reliability, "concurrency") => Some("Concurrency / race risk"),
        (Dimension::Reliability, "recovery") => Some("Incomplete failure recovery"),
        (Dimension::Reliability, "crash") => Some("Crash / unexpected termination"),
        (Dimension::Reliability, "other") => Some("Reliability issue"),
        // Rust / JS / TS / Dart / Python / Java / C# / Ruby / Swift / Shell.
        (Dimension::Reliability, "lockPoisoning") => Some("Poisoned lock ignored"),
        (Dimension::Reliability, "unawaitedPromise") => Some("Unawaited promise"),
        (Dimension::Reliability, "swallowedException") => Some("Swallowed exception"),
        (Dimension::Reliability, "resourceLeak") => Some("Resource not closed on all paths"),
        (Dimension::Reliability, "asyncVoid") => Some("`async void` hides failures"),
        (Dimension::Reliability, "uncheckedAllocation") => Some("Unchecked allocation"),
        (Dimension::Reliability, "danglingReference") => Some("Dangling reference or iterator"),
        (Dimension::Reliability, "exceptionEscapeDestructor") => Some("Exception escapes a noexcept boundary"),
        (Dimension::Reliability, "goroutineLeak") => Some("Goroutine leak"),
        (Dimension::Reliability, "deferInLoop") => Some("Deferred cleanup runs too late"),
        (Dimension::Reliability, "errorSuppression") => Some("Suppressed error"),
        (Dimension::Reliability, "blockingInAsyncContext") => Some("Blocking call in async context"),
        (Dimension::Reliability, "retainCycle") => Some("Retain cycle"),
        (Dimension::Reliability, "forcedTry") => Some("Forced `try!`"),
        (Dimension::Reliability, "ignoredExitStatus") => Some("Ignored exit status"),
        (Dimension::Reliability, "partialPipelineFailure") => Some("Pipeline failure masked"),
        (Dimension::Reliability, "silentErrorContinuation") => Some("Suppressed non-terminating error"),
        (Dimension::Compatibility, "api") => Some("Incompatible public API change"),
        (Dimension::Compatibility, "behavior") => Some("Changed caller-visible behavior"),
        (Dimension::Compatibility, "dataFormat") => Some("Incompatible format change"),
        (Dimension::Compatibility, "protocol") => Some("Incompatible protocol change"),
        (Dimension::Compatibility, "other") => Some("Compatibility issue"),
        // C / C++ / Java / SQL.
        (Dimension::Compatibility, "abiChange") => Some("ABI-breaking change"),
        (Dimension::Compatibility, "serializedFormChange") => Some("Serialized form changed"),
        (Dimension::Compatibility, "destructiveMigration") => Some("Destructive migration"),
        (Dimension::TestGap, "branch") => Some("Untested branch"),
        (Dimension::TestGap, "failure") => Some("Untested failure path"),
        (Dimension::TestGap, "boundary") => Some("Untested boundary case"),
        (Dimension::TestGap, "integration") => Some("Untested component interaction"),
        (Dimension::TestGap, "other") => Some("Test gap"),
        (_, "noIssue") => None,
        _ => None,
    }
}

/// Sets `title` and `why` deterministically from the classified mechanism.
/// The vocabulary is the finding's own language (from its path) plus the
/// generic entries, so a Rust `unwrapPanic` explains itself as one. Pure:
/// performs no IO.
pub fn apply_context(finding: &mut Finding) {
    finding.title = mechanism_title(finding.dimension, &finding.mechanism).map(String::from);
    finding.why = mechanism_description(
        finding.dimension,
        Language::from_path(&finding.file),
        &finding.mechanism,
    )
    .map(String::from);
}

/// The fix-strategy vocabulary (label → human actionable description). The
/// `noSuggestion` sentinel must remain last: the model sorts on this order.
const FIX_STRATEGIES: [(&str, &str); 15] = [
    (
        "validateInput",
        "Validate and constrain the input at the boundary before it reaches the risky sink",
    ),
    (
        "authorize",
        "Add or restore an authorization check before the privileged action",
    ),
    (
        "parameterize",
        "Use parameterized/prepared statements instead of assembling the query or command from strings",
    ),
    (
        "encodeOutput",
        "Escape or encode the value before it is emitted as markup, script, or a URL",
    ),
    (
        "confinePath",
        "Normalize the path and confine it to a safe root; reject separators and absolute escapes",
    ),
    (
        "restrictTarget",
        "Allowlist the server-side request target (scheme, host, port) and block internal addresses",
    ),
    (
        "secureSecrets",
        "Move the secret into configuration or a secret store; never hardcode it in source",
    ),
    (
        "strengthenCrypto",
        "Use a strong cipher, a proper KDF/IV/nonce, and a CSPRNG instead of the weak primitive",
    ),
    (
        "disableEntities",
        "Disable external entity and DTD resolution in the XML parser",
    ),
    (
        "validateDeserialization",
        "Validate or allowlist the type before deserializing untrusted data",
    ),
    (
        "handleErrorPath",
        "Handle the error and failure path explicitly: cleanup, rollback, or a closed state",
    ),
    (
        "synchronizeAccess",
        "Serialize or lock access to the shared state across concurrent tasks",
    ),
    (
        "restoreContract",
        "Reinstate or safely migrate the contract so existing callers keep working",
    ),
    (
        "addCoverage",
        "The concrete change is missing test coverage; add a targeted test for this behavior",
    ),
    ("noSuggestion", "No specific fix is suggested"),
];

/// The test-strategy vocabulary (label → human actionable description). The
/// `noSuggestion` sentinel must remain last: the model sorts on this order.
const TEST_STRATEGIES: [(&str, &str); 8] = [
    ("denialTest", "Assert the unauthorized or blocked case is rejected"),
    (
        "boundaryValue",
        "Exercise the boundary value: empty, overflow, or worst-case input",
    ),
    (
        "failurePath",
        "Cover the error or recovery branch that currently has no assertion",
    ),
    (
        "regression",
        "Pin the exact previously-broken behavior with a regression test",
    ),
    (
        "concurrency",
        "Assert safe shared-state or ordering behavior under concurrency",
    ),
    ("contract", "Assert the persisted format, protocol, or public API shape"),
    (
        "injectionVector",
        "Feed the known injection or taint vector and assert it is neutralized",
    ),
    ("noSuggestion", "No specific test is suggested"),
];

/// Suggests a fix and a test strategy for one finding, in a single
/// `system_one` call: two `choice` questions over the strategy vocabularies.
/// The `noSuggestion` label maps to `None`.
pub async fn enrich_suggestions(
    client: &TypeSafeClient,
    finding: &Finding,
) -> Result<(Option<String>, Option<String>)> {
    let state = json!({
        "dimension": finding.dimension.key(),
        "mechanism": finding.mechanism,
        "severity": finding.severity,
        "evidence": finding.evidence,
    });
    let questions = json!({
        "suggestedFix": choice(
            Value::String(
                "Given this located concern and its evidence, which fix strategy best addresses its root cause?".into(),
            ),
            choice_criteria(&FIX_STRATEGIES),
        ),
        "suggestedTest": choice(
            Value::String(
                "Which test would most directly guard against this concern?".into(),
            ),
            choice_criteria(&TEST_STRATEGIES),
        ),
    });

    let response = client.system_one(state, questions).await?;
    let (fix_label, _) = response.choice("suggestedFix")?;
    let (test_label, _) = response.choice("suggestedTest")?;

    let fix = FIX_STRATEGIES
        .iter()
        .find(|(l, _)| *l == fix_label)
        .filter(|(l, _)| *l != "noSuggestion")
        .map(|(_, desc)| desc.to_string());
    let test = TEST_STRATEGIES
        .iter()
        .find(|(l, _)| *l == test_label)
        .filter(|(l, _)| *l != "noSuggestion")
        .map(|(_, desc)| desc.to_string());

    Ok((fix, test))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::policy::mechanisms_for;

    #[test]
    fn mechanism_title_covers_every_mechanism_key() {
        // Exhaustive: every non-`noIssue` key in the vocabulary maps to a
        // title, and `noIssue` maps to None, so an added key can't silently
        // fall back to rendering the raw mechanism key.
        for language in crate::domain::language::languages() {
            for dimension in crate::domain::policy::DIMENSIONS {
                for (key, _description) in mechanisms_for(dimension, Some(language)) {
                    if key == "noIssue" {
                        assert_eq!(mechanism_title(dimension, key), None);
                    } else {
                        assert!(
                            mechanism_title(dimension, key).is_some(),
                            "missing title for {language:?} {dimension:?}::{key}"
                        );
                    }
                }
            }
        }

        // Spot-check representative titles stay human-correct.
        assert_eq!(
            mechanism_title(Dimension::Security, "sqlInjection"),
            Some("SQL injection")
        );
        assert_eq!(
            mechanism_title(Dimension::TestGap, "boundary"),
            Some("Untested boundary case")
        );
        assert_eq!(
            mechanism_title(Dimension::Reliability, "cleanup"),
            Some("Missing cleanup")
        );
    }

    #[test]
    fn apply_context_sets_title_and_why() {
        let mut finding = Finding {
            file: "src/lib.rs".to_string(),
            line: 10,
            dimension: Dimension::Security,
            probability: 0.9,
            location_confidence: 0.8,
            mechanism: "sqlInjection".to_string(),
            mechanism_confidence: 0.7,
            severity: 2.0,
            severity_confidence: 0.8,
            owner: None,
            owner_confidence: None,
            action: crate::domain::report::Action::Comment,
            evidence: String::new(),
            title: None,
            why: None,
            fix: None,
            test: None,
        };
        apply_context(&mut finding);
        assert_eq!(finding.title.as_deref(), Some("SQL injection"));
        assert!(finding.why.is_some());
        assert!(!finding.why.as_deref().unwrap().is_empty());
    }

    #[test]
    fn strategy_vocabularies_end_with_the_no_suggestion_sentinel() {
        assert_eq!(FIX_STRATEGIES.last().unwrap().0, "noSuggestion");
        assert_eq!(TEST_STRATEGIES.last().unwrap().0, "noSuggestion");
    }
}