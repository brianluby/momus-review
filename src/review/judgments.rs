//! Change-review judgments: screen / profile / locate for diff mode. Every
//! call is narrow and receives patch evidence. Mirrors `review/judgments.ts`.

use anyhow::Result;
use serde_json::{Map, Value, json};

use crate::domain::patch::parse_hunks;
use crate::domain::policy::{
    BLOCKING_SEVERITY, MIN_LOCATION_CONFIDENCE, Probabilities, REVIEW_PRIORITY_RUBRIC,
    ROUTE_SEVERITY, SEVERITY_RUBRIC, Dimension, mechanisms,
};
use crate::domain::report::{Action, ChangedFile, FileProfile, Finding};
use crate::review::strategy::{Screening, Signal};
use crate::review::typesafe::{
    TypeSafeClient, choice, choice_criteria, noul, score, score_criteria,
};

/// `changeTypes` — the diff-file category vocabulary.
const CHANGE_TYPES: [(&str, &str); 6] = [
    ("behavior", "Adds or changes runtime behavior"),
    ("interface", "Changes an exported API, type, protocol, or data shape"),
    ("infrastructure", "Changes execution, scheduling, build, or operational plumbing"),
    ("observability", "Changes events, logging, monitoring, or diagnostics"),
    ("refactor", "Restructures implementation without intending behavior changes"),
    ("routine", "A small routine change that fits none of the other categories"),
];

/// Screens one changed file: five `noul` questions, one per dimension.
pub async fn screen_file(
    client: &TypeSafeClient,
    file: &ChangedFile,
    changed_tests: &[ChangedFile],
) -> Result<Screening<ChangedFile>> {
    let state = json!({ "file": file, "changedTests": changed_tests });

    let questions = json!({
        "correctness": noul(
            json!({
                "question": "Does file.patch directly support that this change likely introduces incorrect runtime behavior?",
                "inspect": "file.patch",
                "focus": "Concrete behavior, state, data-flow, or async errors introduced by added or modified lines",
                "ignore": ["Style preferences", "Naming concerns", "Unsupported speculation"],
            }),
            json!({
                "true": {
                    "what": "The patch contains a realistic path to a wrong runtime result",
                    "examples": ["A condition now handles the opposite case", "A value is written to the wrong field"],
                },
                "false": {
                    "what": "The patch is correct, non-behavioral, or lacks direct evidence of a bug",
                    "examples": ["Formatting only", "A refactor that preserves data flow"],
                },
            }),
        ),
        "security": noul(
            json!({
                "question": "Does file.patch directly support that this change introduces or weakens a security boundary?",
                "inspect": "file.patch",
                "focus": "Authorization, injection, secret exposure, trust boundaries, and unsafe defaults",
            }),
            json!({
                "true": {
                    "what": "The patch creates a concrete path around a security control or into an unsafe sink",
                    "examples": ["An authorization check is removed", "Untrusted input reaches command execution"],
                },
                "false": {
                    "what": "No security boundary is weakened by the patch",
                    "not_for": "Code that merely uses security-related names",
                },
            }),
        ),
        "cryptoSecrets": noul(
            json!({
                "question": "Does file.patch directly support that this change introduces or relies on a cryptographic or secret-management weakness?",
                "inspect": "file.patch",
                "focus": "Weak, missing, or misused cryptography; hardcoded secrets or keys; predictable randomness",
            }),
            json!({
                "true": {
                    "what": "The patch uses cryptography or secrets in a way that weakens the boundary",
                    "examples": ["A new hardcoded key or password", "ECB mode or a predictable IV/nonce", "A fast hash added where a KDF is needed"],
                },
                "false": {
                    "what": "Cryptography and secret handling appear appropriate, or the patch does none",
                    "not_for": "Mere use of cryptographic APIs",
                },
            }),
        ),
        "misconfig": noul(
            json!({
                "question": "Does file.patch directly support that this change introduces a security-relevant misconfiguration?",
                "inspect": "file.patch",
                "focus": "Missing security headers, permissive CORS, debug or stack-trace output, directory listing, default or empty credentials, verbose errors",
            }),
            json!({
                "true": {
                    "what": "The patch creates, or fails to tighten, an avoidable exposure",
                    "examples": ["Access-Control-Allow-Origin: * added to an authenticated endpoint", "Debug or verbose error output left enabled", "A default or empty password fallback"],
                },
                "false": {
                    "what": "Security-relevant configuration appears locked down",
                    "not_for": "Configuration that is merely unusual or non-security",
                },
            }),
        ),
        "reliability": noul(
            json!({
                "question": "Does file.patch directly support that this change can crash, race, leak, deadlock, or recover poorly?",
                "inspect": "file.patch",
                "focus": "Realistic resource, concurrency, cancellation, and failure paths",
            }),
            json!({
                "true": {
                    "what": "A changed path can lose work, leak resources, hang, crash, or leave inconsistent state",
                    "examples": ["Cleanup is skipped after failure", "Concurrent work updates shared state unsafely"],
                },
                "false": { "what": "The patch preserves safe lifecycle and failure handling" },
            }),
        ),
        "compatibility": noul(
            json!({
                "question": "Does file.patch directly support that this change can break an existing caller, format, protocol, or public behavior?",
                "inspect": "file.patch",
                "focus": "Externally observed contracts rather than internal implementation details",
            }),
            json!({
                "true": {
                    "what": "An existing consumer can fail because a contract changed without a safe migration",
                    "examples": ["A required field is removed", "A persisted value changes meaning"],
                },
                "false": { "what": "The changed contract remains compatible or is entirely internal" },
            }),
        ),
        "testGap": noul(
            json!({
                "question": "Does file.patch change important behavior without adequate targeted evidence in changedTests?",
                "compare": ["file.patch", "changedTests"],
                "focus": "New branches, boundaries, failure paths, and component interactions",
            }),
            json!({
                "true": {
                    "what": "Important changed behavior has no targeted changed test",
                    "examples": ["A new failure branch has no assertion", "A protocol change lacks a compatibility test"],
                },
                "false": {
                    "what": "Changed tests exercise the important behavior, or the patch is non-behavioral",
                    "examples": ["A focused regression test covers the branch", "Documentation-only change"],
                },
            }),
        ),
    });

    let response = client.system_one(state, questions).await?;
    let security = response
        .noul("security")?
        .max(response.noul("cryptoSecrets")?)
        .max(response.noul("misconfig")?);
    let probabilities = Probabilities::from([
        (Dimension::Correctness, response.noul("correctness")?),
        (Dimension::Security, security),
        (Dimension::Reliability, response.noul("reliability")?),
        (Dimension::Compatibility, response.noul("compatibility")?),
        (Dimension::TestGap, response.noul("testGap")?),
    ]);

    Ok(Screening { file: file.clone(), probabilities })
}

/// Profiles a changed file: category + review priority.
pub async fn profile_file(
    client: &TypeSafeClient,
    file: &ChangedFile,
    screening_probabilities: &Probabilities,
) -> Result<FileProfile> {
    let state = json!({ "file": file, "screeningProbabilities": screening_probabilities });
    let questions = json!({
        "category": choice(
            json!({ "question": "Which category best describes file.patch?", "focus": "Primary purpose of the change" }),
            choice_criteria(&CHANGE_TYPES),
        ),
        "reviewPriority": score(
            Value::String("Rate how closely a human should review file.patch, considering the code and screeningProbabilities.".into()),
            score_criteria(&REVIEW_PRIORITY_RUBRIC),
        ),
    });

    let response = client.system_one(state, questions).await?;
    let (category, category_confidence) = response.choice("category")?;
    let (review_priority, review_priority_confidence) = response.score("reviewPriority")?;

    Ok(FileProfile {
        file: file.path.to_string(),
        category,
        category_confidence,
        review_priority,
        review_priority_confidence,
    })
}

/// Locates, classifies, scores, and routes a signals from a changed file.
pub async fn locate_signal(
    client: &TypeSafeClient,
    signal: &Signal<ChangedFile>,
) -> Result<Option<Finding>> {
    let hunks = parse_hunks(&signal.file.patch);
    if hunks.is_empty() {
        return Ok(None);
    }

    let suspected_concern = json!({
        "dimension": signal.dimension,
        "definition": signal.dimension.definition(),
    });

    // 1. Evidence: pick the strongest hunk.
    let mut criteria = Map::new();
    for hunk in &hunks {
        criteria.insert(
            hunk.id.clone(),
            Value::String(format!("Candidate beginning at changed-file line {}", hunk.start_line)),
        );
    }
    criteria.insert(
        "noMatch".to_string(),
        Value::String("No candidate hunk directly supports the suspected concern".to_string()),
    );
    let location = client
        .system_one(
            json!({
                "file": signal.file.path.to_string(),
                "suspectedConcern": {
                    "dimension": signal.dimension,
                    "definition": signal.dimension.definition(),
                    "screeningProbability": signal.probability,
                },
                "candidateHunks": hunks,
            }),
            json!({
                "evidence": choice(
                    json!({
                        "question": "Which candidate hunk provides the strongest direct evidence for suspectedConcern?",
                        "fallback": "Select noMatch when no hunk provides sufficient evidence",
                    }),
                    Value::Object(criteria),
                ),
            }),
        )
        .await?;
    let (selected_id, selected_confidence) = location.choice("evidence")?;
    if selected_id == "noMatch" || selected_confidence < MIN_LOCATION_CONFIDENCE {
        return Ok(None);
    }
    let Some(hunk) = hunks.iter().find(|h| h.id == selected_id) else {
        return Ok(None);
    };

    // 2. Mechanism: classify the concern (noIssue kills it).
    let classification = client
        .system_one(
            json!({
                "file": signal.file.path.to_string(),
                "suspectedConcern": suspected_concern,
                "selectedEvidence": hunk,
            }),
            json!({
                "mechanism": choice(
                    Value::String("Which mechanism best describes the suspected concern supported by selectedEvidence?".into()),
                    choice_criteria(mechanisms(signal.dimension)),
                ),
            }),
        )
        .await?;
    let (mechanism, mechanism_confidence) = classification.choice("mechanism")?;
    if mechanism == "noIssue" {
        return Ok(None);
    }

    // 3. Severity: score the production impact.
    let impact = client
        .system_one(
            json!({
                "file": signal.file.path.to_string(),
                "suspectedConcern": suspected_concern,
                "selectedEvidence": hunk,
            }),
            json!({
                "severity": score(
                    Value::String("Assuming selectedEvidence exhibits suspectedConcern, rate the likely production impact.".into()),
                    score_criteria(&SEVERITY_RUBRIC),
                ),
            }),
        )
        .await?;
    let (severity, severity_confidence) = impact.score("severity")?;

    // 4. Route: assign an owner only above the routing threshold.
    let mut owner = None;
    let mut owner_confidence = None;
    if severity >= ROUTE_SEVERITY {
        let routing = client
            .system_one(
                json!({
                    "file": signal.file.path.to_string(),
                    "concern": {
                        "dimension": signal.dimension,
                        "mechanism": mechanism.clone(),
                        "severity": severity,
                    },
                    "selectedEvidence": hunk,
                }),
                json!({
                    "owner": choice(
                        Value::String("Which reviewer is best suited to investigate this concern?".into()),
                        choice_criteria(crate::domain::policy::OWNERS.as_slice()),
                    ),
                }),
            )
            .await?;
        let (o, oc) = routing.choice("owner")?;
        owner = Some(o);
        owner_confidence = Some(oc);
    }

    let action = if severity >= BLOCKING_SEVERITY {
        Action::RequestChanges
    } else {
        Action::Comment
    };

    Ok(Some(Finding {
        file: signal.file.path.to_string(),
        line: hunk.start_line,
        dimension: signal.dimension,
        probability: signal.probability,
        location_confidence: selected_confidence,
        mechanism,
        mechanism_confidence,
        severity,
        severity_confidence,
        owner,
        owner_confidence,
        action,
        evidence: hunk.patch.clone(),
        title: None,
        why: None,
        fix: None,
        test: None,
    }))
}