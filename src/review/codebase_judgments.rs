//! Codebase-scan judgments: screen / profile / locate for whole-file mode.
//! These ask whether an issue exists in complete source, rather than whether
//! a patch introduced one.

use anyhow::Result;
use serde_json::{Map, Value, json};

use crate::domain::policy::{
    BLOCKING_SEVERITY, DIMENSIONS, Dimension, MIN_LOCATION_CONFIDENCE, MIN_META_JUDGE_CONFIDENCE,
    Probabilities, REVIEW_PRIORITY_RUBRIC, ROUTE_SEVERITY, SEVERITY_RUBRIC,
};
use crate::domain::report::{Action, FileProfile, Finding, SourceFile};
use crate::review::context::{ContextBudget, ContextDrops, select_related_tests};
use crate::review::regions::function_regions;
use crate::review::typesafe::{
    TypeSafeClient, choice, choice_criteria, mechanism_criteria, noul, score, score_criteria,
};
use crate::review::{
    meta,
    strategy::{Screening, Signal},
};

const REGION_LINES: usize = 80;
const SCREEN_REGION_LINES: usize = 160;
const MAX_NEIGHBOR_LINES: usize = 40;
const MAX_NEIGHBOR_CHARS: usize = 1_800;

/// `fileRoles` — the source-file role vocabulary.
pub(crate) const FILE_ROLES: [(&str, &str); 6] = [
    (
        "entrypoint",
        "Application, command, route, or public package entry point",
    ),
    (
        "boundary",
        "Authentication, validation, serialization, or external-system boundary",
    ),
    (
        "domain",
        "Core business rules, state transitions, or domain behavior",
    ),
    (
        "persistence",
        "Database, cache, filesystem, migration, or durable state",
    ),
    (
        "infrastructure",
        "Runtime, scheduling, networking, build, or operational plumbing",
    ),
    (
        "utility",
        "Shared helper, adapter, formatting, or low-level utility",
    ),
];

/// Screens one source file per function-aware region (declaration-aligned,
/// capped at `SCREEN_REGION_LINES`), then max-merges per dimension.
pub async fn screen_source_file(
    client: &TypeSafeClient,
    file: &SourceFile,
    test_files: &[SourceFile],
    neighbors: &[SourceFile],
) -> Result<Screening<SourceFile>> {
    screen_source_file_indexed(client, file, test_files, neighbors, None).await
}

/// Screen a source with optional indexed regions/signatures and bounded related-test context.
pub async fn screen_source_file_indexed(
    client: &TypeSafeClient,
    file: &SourceFile,
    test_files: &[SourceFile],
    neighbors: &[SourceFile],
    index: Option<&crate::review::index::RepoIndex>,
) -> Result<Screening<SourceFile>> {
    let related_tests = index
        .and_then(|i| i.related_tests(&file.path))
        .map(<[SourceFile]>::to_vec)
        .unwrap_or_else(|| select_related_tests(file, test_files));

    let compact_neighbors: Vec<SourceFile> = neighbors.iter().map(compact_neighbor).collect();
    let mut results: Vec<Probabilities> = Vec::new();
    let mut drops = ContextDrops::default();

    let regions = index
        .map(|i| i.regions(file, SCREEN_REGION_LINES))
        .unwrap_or_else(|| function_regions(&file.content, &file.path, SCREEN_REGION_LINES));
    for region in regions {
        // One budget per request: the region under review first, then
        // related tests, then neighbors; whatever does not fit is trimmed
        // and counted.
        let mut budget = ContextBudget::from_env()?;
        let content = budget.take(&region.content);
        let tests: Vec<SourceFile> = related_tests
            .iter()
            .map(|t| SourceFile {
                path: t.path.clone(),
                content: budget.take(&t.content),
            })
            .collect();
        let context_neighbors: Vec<SourceFile> = compact_neighbors
            .iter()
            .map(|n| SourceFile {
                path: n.path.clone(),
                content: budget.take(&n.content),
            })
            .collect();
        let signatures = budget.take(&index.map_or_else(
            || crate::review::regions::export_signatures(&file.content, &file.path),
            |i| i.source_signatures(file),
        ));
        drops = drops + budget.drops;
        let state = json!({
            "file": { "path": file.path, "startLine": region.start_line, "content": content },
            "relatedTests": tests,
            "neighbors": context_neighbors,
            "exportSignatures": signatures,
        });
        let questions = json!({
            "correctness": noul(
                json!({
                    "question": "Does file.content directly support that this code contains incorrect runtime behavior?",
                    "inspect": "file.content",
                    "focus": "Concrete behavior, state, data-flow, or async errors reachable in realistic use",
                    "ignore": ["Style preferences", "Naming concerns", "Missing context with no concrete failure path"],
                }),
                json!({
                    "true": { "what": "The source contains a realistic path to a wrong runtime result", "examples": ["A condition handles the opposite case", "State is updated under the wrong key"] },
                    "false": { "what": "The implementation is coherent or no concrete incorrect path is supported", "not_for": "Unusual code that is still internally consistent" },
                }),
            ),
            "security": noul(
                json!({
                    "question": "Does file.content directly support that this code weakens a security boundary?",
                    "inspect": "file.content",
                    "focus": "Authorization, injection, secret exposure, trust boundaries, and unsafe defaults",
                }),
                json!({
                    "true": { "what": "The source contains a concrete path around a control or into an unsafe sink", "examples": ["A privileged action lacks authorization", "Untrusted input reaches command execution"] },
                    "false": { "what": "No concrete security weakness is supported by this file", "not_for": "Code that merely handles credentials or permissions safely" },
                }),
            ),
            "cryptoSecrets": noul(
                json!({
                    "question": "Does file.content directly support a cryptographic or secret-management weakness?",
                    "inspect": "file.content",
                    "focus": "Weak, missing, or misused cryptography; hardcoded secrets or keys; predictable randomness",
                }),
                json!({
                    "true": { "what": "Cryptography or secrets are used in a way that weakens the boundary", "examples": ["A hardcoded key or password in source", "ECB mode or a predictable IV/nonce", "A fast hash used instead of a password KDF"] },
                    "false": { "what": "Cryptography and secret handling appear appropriate, or the file does none", "not_for": "Mere use of cryptographic APIs" },
                }),
            ),
            "misconfig": noul(
                json!({
                    "question": "Does file.content directly support a security-relevant misconfiguration?",
                    "inspect": "file.content",
                    "focus": "Missing security headers, permissive CORS, debug or stack-trace output, directory listing, default or empty credentials, verbose errors",
                }),
                json!({
                    "true": { "what": "A setting, or its absence, creates avoidable exposure", "examples": ["Access-Control-Allow-Origin: * on an authenticated endpoint", "Debug or verbose error output left enabled", "A default or empty password fallback"] },
                    "false": { "what": "Security-relevant configuration appears locked down", "not_for": "Configuration that is merely unusual or non-security" },
                }),
            ),
            "reliability": noul(
                json!({
                    "question": "Does file.content directly support that this code can crash, race, leak, deadlock, or recover poorly?",
                    "inspect": "file.content",
                    "focus": "Realistic resource, concurrency, cancellation, and failure paths",
                }),
                json!({
                    "true": { "what": "A reachable path can lose work, leak resources, hang, crash, or leave inconsistent state", "examples": ["Cleanup is skipped after failure", "Concurrent work mutates shared state unsafely"] },
                    "false": { "what": "Lifecycle and failure handling appear safe, or no concrete failure path is supported" },
                }),
            ),
            "compatibility": noul(
                json!({
                    "question": "Does file.content directly support an internal inconsistency that can break a caller, format, protocol, or documented behavior?",
                    "inspect": "file.content",
                    "focus": "Contradictions between file.content and its callers/callees in neighbors, or visible in this source — not guesses about unknown historical versions",
                }),
                json!({
                    "true": { "what": "The source contains conflicting contracts or a concrete caller-facing mismatch", "examples": ["A parser and serializer disagree on a required field", "An exported type contradicts runtime behavior"] },
                    "false": { "what": "The visible contracts are internally consistent", "not_for": "Speculation that an API may once have behaved differently" },
                }),
            ),
            "testGap": noul(
                json!({
                    "question": "Does file.content contain important behavior without adequate targeted evidence in relatedTests?",
                    "compare": ["file.content", "relatedTests"],
                    "focus": "Critical branches, boundaries, failure paths, and component interactions",
                    "caution": "A filename mismatch alone is not enough; identify behavior that specifically needs a test",
                }),
                json!({
                    "true": { "what": "Important behavior is present and the related tests do not exercise it", "examples": ["An error-recovery branch has no assertion", "Authorization behavior lacks a denial test"] },
                    "false": { "what": "Related tests cover the important behavior, or this file has no behavior needing direct tests", "examples": ["A focused test covers the boundary", "A declarative constants module"] },
                }),
            ),
        });

        let response = client.system_one(state, questions).await?;
        let security = response
            .noul("security")?
            .max(response.noul("cryptoSecrets")?)
            .max(response.noul("misconfig")?);
        results.push(Probabilities::from([
            (Dimension::Correctness, response.noul("correctness")?),
            (Dimension::Security, security),
            (Dimension::Reliability, response.noul("reliability")?),
            (Dimension::Compatibility, response.noul("compatibility")?),
            (Dimension::TestGap, response.noul("testGap")?),
        ]));
    }

    let probabilities = DIMENSIONS
        .iter()
        .map(|&d| {
            let max = results
                .iter()
                .map(|r| r[&d])
                .fold(f64::NEG_INFINITY, f64::max);
            (d, max)
        })
        .collect();

    Ok(Screening {
        file: file.clone(),
        probabilities,
        dropped: drops,
    })
}

/// Profiles a source file: role + review priority.
pub async fn profile_source_file(
    client: &TypeSafeClient,
    file: &SourceFile,
    screening_probabilities: &Probabilities,
) -> Result<FileProfile> {
    let state = json!({ "file": file, "screeningProbabilities": screening_probabilities });
    let questions = json!({
        "category": choice(
            json!({ "question": "Which role best describes this source file?", "focus": "Primary runtime responsibility" }),
            choice_criteria(&FILE_ROLES),
        ),
        "reviewPriority": score(
            Value::String("Rate how closely a human should review this complete file, considering its role and screeningProbabilities.".into()),
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

/// Locates, classifies, scores, and routes a signal from a source file.
/// `neighbors` (1-hop callers/callees) ride along so the meta-judge sees the
/// same surrounding context the screener did.
pub async fn locate_source_signal(
    client: &TypeSafeClient,
    signal: &Signal<SourceFile>,
    neighbors: &[SourceFile],
) -> Result<Option<Finding>> {
    locate_source_signal_indexed(client, signal, neighbors, None).await
}

/// Locate and classify a source signal using indexed evidence regions when available.
pub async fn locate_source_signal_indexed(
    client: &TypeSafeClient,
    signal: &Signal<SourceFile>,
    neighbors: &[SourceFile],
    index: Option<&crate::review::index::RepoIndex>,
) -> Result<Option<Finding>> {
    let regions = index
        .map(|i| i.regions(&signal.file, REGION_LINES))
        .unwrap_or_else(|| function_regions(&signal.file.content, &signal.file.path, REGION_LINES));
    if regions.is_empty() {
        return Ok(None);
    }

    // 1. Evidence: pick the strongest region.
    let mut criteria = Map::new();
    for region in &regions {
        criteria.insert(
            region.id.clone(),
            Value::String(format!("Source beginning at line {}", region.start_line)),
        );
    }
    criteria.insert(
        "noMatch".to_string(),
        Value::String("No source region directly supports the suspected concern".to_string()),
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
                "candidateRegions": regions,
            }),
            json!({
                "evidence": choice(
                    json!({
                        "question": "Which candidate region provides the strongest direct evidence for suspectedConcern?",
                        "fallback": "Select noMatch when no region provides sufficient evidence",
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
    let Some(region) = regions.iter().find(|r| r.id == selected_id) else {
        return Ok(None);
    };

    let suspected_concern = json!({
        "dimension": signal.dimension,
        "definition": signal.dimension.definition(),
    });

    // 2. Mechanism.
    let classification = client
        .system_one(
            json!({
                "file": signal.file.path.to_string(),
                "suspectedConcern": suspected_concern,
                "selectedEvidence": region,
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

    // 3. Severity.
    let impact = client
        .system_one(
            json!({
                "file": signal.file.path.to_string(),
                "suspectedConcern": suspected_concern,
                "selectedEvidence": region,
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
    let evidence = json!({
        "region": region,
        "neighbors": neighbors.iter().map(compact_neighbor).collect::<Vec<_>>(),
    });
    if meta::judge(client, signal.dimension, &mechanism, &evidence).await?
        < MIN_META_JUDGE_CONFIDENCE
    {
        return Ok(None);
    }

    // 6. Route.
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
                    "selectedEvidence": region,
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
        line: region.start_line,
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
        evidence: region.content.clone(),
        ..Default::default()
    }))
}

// ---- Helpers ---------------------------------------------------------

/// Trim a neighboring source to the supplied refinement-context character limit.
pub(crate) fn compact_neighbor(f: &SourceFile) -> SourceFile {
    let content: String = f
        .content
        .split('\n')
        .take(MAX_NEIGHBOR_LINES)
        .collect::<Vec<_>>()
        .join("\n");
    let content = if content.len() > MAX_NEIGHBOR_CHARS {
        content.chars().take(MAX_NEIGHBOR_CHARS).collect()
    } else {
        content
    };

    SourceFile {
        path: f.path.clone(),
        content,
    }
}
