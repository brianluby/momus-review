//! Change-review judgments: screen / profile / locate for diff mode. Every
//! call is narrow and receives patch evidence.

use anyhow::Result;
use serde_json::{Map, Value, json};

use crate::domain::patch::{first_added_line, parse_hunks, split_hunk};
use crate::domain::policy::{
    BLOCKING_SEVERITY, MIN_LOCATION_CONFIDENCE, MIN_META_JUDGE_CONFIDENCE, Probabilities,
    REVIEW_PRIORITY_RUBRIC, ROUTE_SEVERITY, SEVERITY_RUBRIC, Dimension,
};
use crate::domain::report::{Action, ChangedFile, FileProfile, Finding, Hunk};
use crate::review::regions::function_regions;
use crate::review::{meta, strategy::{Screening, Signal}};
use crate::review::typesafe::{
    TypeSafeClient, choice, choice_criteria, mechanism_criteria, noul, score, score_criteria,
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
                "compare": ["file.base", "file.patch"],
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

/// A hunk with more new-side lines than this is split before evidence
/// selection (the same size as codebase mode's evidence regions and the
/// chunks of an untracked file), so the chosen piece, and the finding's
/// line, sit near the code instead of at the start of a large hunk.
const MAX_EVIDENCE_LINES: usize = 80;

/// The evidence candidates for a changed file: its hunks, with each large
/// one split at declaration boundaries (`regions::function_regions` over the
/// hunk's new-side text), or into `MAX_EVIDENCE_LINES` windows where it has
/// none. A newly added file is one hunk, so this is what keeps its findings
/// off line 1.
fn candidate_hunks(file: &ChangedFile) -> Vec<Hunk> {
    let mut candidates = Vec::new();
    for hunk in parse_hunks(&file.patch) {
        let new_side: Vec<&str> = hunk
            .patch
            .split('\n')
            .skip(1)
            .filter(|line| line.starts_with(' ') || line.starts_with('+'))
            .map(|line| &line[1..])
            .collect();
        if new_side.len() <= MAX_EVIDENCE_LINES {
            candidates.push(hunk);
            continue;
        }
        let cuts: Vec<usize> = function_regions(&new_side.join("\n"), &file.path, MAX_EVIDENCE_LINES)
            .iter()
            .skip(1)
            .map(|region| hunk.start_line + region.start_line - 1)
            .collect();
        candidates.extend(split_hunk(&hunk, &cuts));
    }
    for (index, hunk) in candidates.iter_mut().enumerate() {
        hunk.id = format!("hunk_{}", index + 1);
    }
    candidates
}

/// Locates, classifies, scores, and routes a signals from a changed file.
pub async fn locate_signal(
    client: &TypeSafeClient,
    signal: &Signal<ChangedFile>,
) -> Result<Option<Finding>> {
    let hunks = candidate_hunks(&signal.file);
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
                    mechanism_criteria(&signal.file.path, signal.dimension),
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

    // 5. Meta-judge: a second skeptical pass kills unsupported claims.
    let evidence = json!({ "hunk": hunk, "base": signal.file.base });
    if meta::judge(client, signal.dimension, &mechanism, &evidence).await?
        < MIN_META_JUDGE_CONFIDENCE
    {
        return Ok(None);
    }

    // 6. Route: assign an owner only above the routing threshold.
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
        // The first changed line, not the hunk's leading context: what a
        // reader (and a PR review comment) should land on.
        line: first_added_line(hunk),
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
        ..Default::default()
    }))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::patch::first_added_line;

    fn new_file(path: &str, content: &str) -> ChangedFile {
        let lines: Vec<&str> = content.split('\n').collect();
        let body: String = lines.iter().map(|l| format!("\n+{l}")).collect();
        ChangedFile {
            path: path.into(),
            patch: format!("@@ -0,0 +1,{} @@{body}", lines.len()),
            base: String::new(),
        }
    }

    #[test]
    fn a_large_new_rust_file_offers_one_candidate_per_declaration() {
        // Two 60-line functions: 120 lines, one hunk, over the evidence cap.
        let func = |name: &str| {
            let body: String = (0..58).map(|i| format!("    let _{i} = {i};\n")).collect();
            format!("fn {name}() {{\n{body}}}")
        };
        let file = new_file("src/lib.rs", &format!("{}\n{}", func("alpha"), func("beta")));
        let candidates = candidate_hunks(&file);
        let anchors: Vec<usize> = candidates.iter().map(first_added_line).collect();
        assert_eq!(anchors, [1, 61], "one piece per function, anchored on its fn line");
        assert!(candidates[1].patch.lines().nth(1).unwrap().starts_with("+fn beta"));
        let ids: Vec<&str> = candidates.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(ids, ["hunk_1", "hunk_2"]);
    }

    #[test]
    fn a_large_declaration_free_file_splits_into_windows() {
        let content: String = (1..=200).map(|i| format!("echo {i}")).collect::<Vec<_>>().join("\n");
        let candidates = candidate_hunks(&new_file("deploy.sh", &content));
        let anchors: Vec<usize> = candidates.iter().map(first_added_line).collect();
        assert_eq!(anchors, [1, 81, 161]);
    }

    #[test]
    fn small_hunks_are_unchanged() {
        let file = ChangedFile {
            path: "src/a.rs".into(),
            patch: "@@ -1,2 +1,3 @@\n a\n+b\n c\n@@ -40,1 +41,1 @@\n-x\n+y".into(),
            base: String::new(),
        };
        let candidates = candidate_hunks(&file);
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].patch, parse_hunks(&file.patch)[0].patch);
        assert_eq!(candidates[1].id, "hunk_2");
    }
}
