//! SARIF 2.1.0 export for a reviewed report: one `result` per finding, one
//! shared `rule` per distinct mechanism/dimension pair. Consumers (CI
//! uploaders, IDE integrations) key off `ruleId = "{dimension}/{mechanism}"`.
//! Mirrors the copy-as-PR-comment mapping the dashboard uses.

use serde_json::{Value, json};

use crate::domain::policy::{Dimension, mechanisms};
use crate::domain::report::{Action, Finding, ReviewReport};
use crate::review::explain::mechanism_title;

/// The pinned severity→level map shared with the dashboard trend:
/// `RequestChanges` blocks, a routed (owned) finding warns, rest are notes.
fn level(finding: &Finding) -> &'static str {
    match finding.action {
        Action::RequestChanges => "error",
        _ if finding.owner.is_some() => "warning",
        _ => "note",
    }
}

/// The human label for a dimension (for fallback titles and messages).
fn dimension_label(dimension: Dimension) -> &'static str {
    match dimension {
        Dimension::Correctness => "Correctness",
        Dimension::Security => "Security",
        Dimension::Reliability => "Reliability",
        Dimension::Compatibility => "Compatibility",
        Dimension::TestGap => "Test gap",
    }
}

/// Human title for a finding: the deterministic `title` when present,
/// otherwise `"{dimension label}: {mechanism}"`.
fn finding_title(finding: &Finding) -> String {
    finding
        .title
        .clone()
        .unwrap_or_else(|| format!("{}: {}", dimension_label(finding.dimension), finding.mechanism))
}

/// The mechanism description (`why` vocabulary) for a finding, reused as the
/// rule's `help.text`.
fn mechanism_description(finding: &Finding) -> String {
    mechanisms(finding.dimension)
        .iter()
        .find(|(key, _)| *key == finding.mechanism)
        .map(|(_, description)| description.to_string())
        .unwrap_or_default()
}

/// Renders `report` as a SARIF 2.1.0 log. One `result` per finding; distinct
/// `ruleId`s each contribute one `rule` entry so the log stays minimal.
pub fn to_sarif(report: &ReviewReport) -> Value {
    let mut rules: Vec<Value> = Vec::new();

    let results: Vec<Value> = report
        .findings
        .iter()
        .map(|finding| {
            let rule_id = format!("{}/{}", finding.dimension.key(), finding.mechanism);

            // Register a rule once per distinct ruleId (shortDescription uses
            // the deterministic title where one exists).
            if !rules.iter().any(|rule| rule["id"].as_str() == Some(rule_id.as_str())) {
                let short = mechanism_title(finding.dimension, &finding.mechanism)
                    .map(String::from)
                    .unwrap_or_else(|| finding.mechanism.clone());
                rules.push(json!({
                    "id": rule_id,
                    "shortDescription": { "text": short },
                    "help": { "text": mechanism_description(finding) },
                }));
            }

            // A SARIF `region` must carry a location property; omit it
            // entirely when there is no line rather than emit `{}`.
            let mut location = json!({ "artifactLocation": { "uri": finding.file } });
            if finding.line != 0 {
                location["region"] = json!({ "startLine": finding.line });
            }

            json!({
                "ruleId": rule_id,
                "level": level(finding),
                "message": { "text": finding_title(finding) },
                "locations": [{ "physicalLocation": location }],
            })
        })
        .collect();

    json!({
        "version": "2.1.0",
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "runs": [{
            "tool": {
                "driver": {
                    "name": "momus-review",
                    "informationUri": "https://github.com/brianluby/momus-review",
                    "rules": rules,
                },
            },
            "results": results,
        }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(
        mechanism: &str,
        action: Action,
        owner: Option<&str>,
        line: usize,
    ) -> Finding {
        Finding {
            file: "src/x.rs".into(),
            line,
            dimension: Dimension::Security,
            probability: 0.9,
            location_confidence: 0.9,
            mechanism: mechanism.into(),
            mechanism_confidence: 0.8,
            severity: 2.9,
            severity_confidence: 0.8,
            owner: owner.map(String::from),
            owner_confidence: None,
            action,
            evidence: String::new(),
            title: None,
            why: None,
            fix: None,
            test: None,
        }
    }

    #[test]
    fn sarif_has_version_results_and_locations() {
        let report = ReviewReport {
            findings: vec![
                finding("brokenAccessControl", Action::RequestChanges, Some("security"), 42),
                finding("brokenAccessControl", Action::Comment, None, 0),
            ],
            ..Default::default()
        };

        let value = to_sarif(&report);

        assert_eq!(value["version"], "2.1.0");
        assert_eq!(value["$schema"], "https://json.schemastore.org/sarif-2.1.0.json");

        let results = value["runs"][0]["results"].as_array().unwrap();
        assert_eq!(results.len(), 2);

        let blocking = &results[0];
        assert_eq!(blocking["ruleId"], "security/brokenAccessControl");
        assert_eq!(blocking["level"], "error");
        assert_eq!(blocking["message"]["text"], "Security: brokenAccessControl");
        let location = &blocking["locations"][0]["physicalLocation"];
        assert_eq!(location["artifactLocation"]["uri"], "src/x.rs");
        assert_eq!(location["region"]["startLine"], 42);

        let comment = &results[1];
        assert_eq!(comment["level"], "note");
        // line == 0 omits `region` entirely (SARIF regions need a location
        // property); emitting `{}` would be invalid.
        assert!(comment["locations"][0]["physicalLocation"].get("region").is_none());

        // One rule per distinct ruleId, with the deterministic title.
        let rules = value["runs"][0]["tool"]["driver"]["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0]["id"], "security/brokenAccessControl");
        assert_eq!(rules[0]["shortDescription"]["text"], "Broken access control");
        assert!(rules[0]["help"]["text"].as_str().is_some());
    }
}