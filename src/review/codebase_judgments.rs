//! Codebase-scan judgments: screen / profile / locate for whole-file mode.
//! These ask whether an issue exists in complete source, rather than whether
//! a patch introduced one. Mirrors `review/codebase-judgments.ts`.

use std::collections::BTreeSet;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::domain::policy::{
    BLOCKING_SEVERITY, DIMENSIONS, MIN_LOCATION_CONFIDENCE, Probabilities, REVIEW_PRIORITY_RUBRIC,
    ROUTE_SEVERITY, SEVERITY_RUBRIC, Dimension, mechanisms,
};
use crate::domain::report::{Action, FileProfile, Finding, SourceFile};
use crate::review::strategy::{Screening, Signal};
use crate::review::typesafe::{
    TypeSafeClient, choice, choice_criteria, noul, score, score_criteria,
};

const REGION_LINES: usize = 80;
const SCREEN_REGION_LINES: usize = 160;
const MAX_RELATED_TESTS: usize = 4;
const MAX_TEST_SNIPPET_CHARS: usize = 1_800;

/// `fileRoles` — the source-file role vocabulary.
const FILE_ROLES: [(&str, &str); 6] = [
    ("entrypoint", "Application, command, route, or public package entry point"),
    ("boundary", "Authentication, validation, serialization, or external-system boundary"),
    ("domain", "Core business rules, state transitions, or domain behavior"),
    ("persistence", "Database, cache, filesystem, migration, or durable state"),
    ("infrastructure", "Runtime, scheduling, networking, build, or operational plumbing"),
    ("utility", "Shared helper, adapter, formatting, or low-level utility"),
];

/// A source-region window: `{ id, startLine, content }`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Region {
    id: String,
    start_line: usize,
    content: String,
}

/// Screens one source file per 160-line region, then max-merges per dimension.
pub async fn screen_source_file(
    client: &TypeSafeClient,
    file: &SourceFile,
    test_files: &[SourceFile],
) -> Result<Screening<SourceFile>> {
    let related_tests = select_related_tests(file, test_files);
    let mut results: Vec<Probabilities> = Vec::new();

    for region in source_regions(&file.content, SCREEN_REGION_LINES) {
        let state = json!({
            "file": { "path": file.path, "startLine": region.start_line, "content": region.content },
            "relatedTests": related_tests,
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
                    "focus": "Contradictions visible in this source, not guesses about unknown historical versions",
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
        results.push(Probabilities::from([
            (Dimension::Correctness, response.noul("correctness")?),
            (Dimension::Security, response.noul("security")?),
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

    Ok(Screening { file: file.clone(), probabilities })
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
pub async fn locate_source_signal(
    client: &TypeSafeClient,
    signal: &Signal<SourceFile>,
) -> Result<Option<Finding>> {
    let regions = source_regions(&signal.file.content, REGION_LINES);
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
                    choice_criteria(mechanisms(signal.dimension)),
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

    // 4. Route.
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
    }))
}

// ---- Helpers (mirror the private fns in codebase-judgments.ts) ---------

fn source_regions(content: &str, lines_per_region: usize) -> Vec<Region> {
    let lines: Vec<&str> = content.split('\n').collect();
    let count = lines.len().div_ceil(lines_per_region);
    (0..count)
        .map(|i| {
            let start = i * lines_per_region;
            let end = lines.len().min(start + lines_per_region);
            Region {
                id: format!("R{}", i + 1),
                start_line: start + 1,
                content: lines[start..end].join("\n"),
            }
        })
        .collect()
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn dirname(path: &str) -> String {
    match path.rsplit_once('/') {
        Some((d, _)) => d.to_string(),
        None => ".".to_string(),
    }
}

fn stem(path: &str) -> String {
    match basename(path).rsplit_once('.') {
        Some((s, _)) => s.to_string(),
        None => basename(path).to_string(),
    }
}

fn select_related_tests(file: &SourceFile, test_files: &[SourceFile]) -> Vec<SourceFile> {
    let stem = stem(&file.path);
    let directory = dirname(&file.path);

    let mut scored: Vec<(i32, &SourceFile)> = test_files
        .iter()
        .map(|test| {
            let score = usize::from(test.path.contains(&stem)) as i32 * 2
                + usize::from(test.path.starts_with(&directory)) as i32;
            (score, test)
        })
        .filter(|(score, _)| *score > 0)
        .collect();

    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.path.cmp(&b.1.path)));
    scored
        .into_iter()
        .take(MAX_RELATED_TESTS)
        .map(|(_, test)| compact_test(test, &stem))
        .collect()
}

fn compact_test(test: &SourceFile, source_stem: &str) -> SourceFile {
    let lines: Vec<&str> = test.content.split('\n').collect();
    let mut selected: BTreeSet<usize> = BTreeSet::new();
    let stem = source_stem.to_lowercase();

    for (index, line) in lines.iter().enumerate() {
        let lower = line.to_lowercase();
        if lower.contains(&stem)
            || lower.contains("describe(")
            || lower.contains("describe.")
            || lower.contains("test(")
            || lower.contains("test.")
            || lower.contains("it(")
            || lower.contains("it.")
            || lower.contains("#[test]")
            || lower.contains("#[tokio::test]")
            || lower.contains("fn ")
            || lower.contains("assert")
        {
            for nearby in index.saturating_sub(2)..=(lines.len() - 1).min(index + 2) {
                selected.insert(nearby);
            }
        }
    }

    let mut content: String = selected
        .iter()
        .map(|&i| lines[i])
        .collect::<Vec<_>>()
        .join("\n");
    if content.is_empty() {
        content = test.content.clone();
    }
    if content.len() > MAX_TEST_SNIPPET_CHARS {
        let side = (MAX_TEST_SNIPPET_CHARS - 7) / 2;
        let head: String = content.chars().take(side).collect();
        let tail: String = content.chars().rev().take(side).collect::<String>().chars().rev().collect();
        content = format!("{head}\n...\n{tail}");
    }

    SourceFile { path: test.path.clone(), content }
}