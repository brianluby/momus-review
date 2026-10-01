//! Deterministic, local dependency triage. Semver and release notes are
//! compatibility signals, never calibrated probabilities of breakage.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::domain::feedback::fingerprint;
use crate::domain::policy::Dimension;
use crate::domain::report::{Action, Finding, SourceFile};
use crate::domain::repository::RepositoryChange;

const MAX_DEPENDENCIES: usize = 2048;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UpgradeSummary {
    pub changes: Vec<DependencyChange>,
    pub findings: Vec<Finding>,
    pub unknowns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeEvidence {
    pub path: String,
    /// Zero means the declaration was parsed but its source line is unknown.
    pub line: usize,
    /// `base` or `current`; removed dependencies cite the former explicitly.
    pub snapshot: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DependencyChange {
    pub ecosystem: String,
    pub dependency: String,
    pub scope: String,
    /// `added`, `removed`, or `changed`; downgrades are also changed.
    pub kind: String,
    /// A deterministic signal, not proof of breaking behavior.
    pub risk: String,
    pub old_version: Option<String>,
    pub new_version: Option<String>,
    pub evidence: Vec<UpgradeEvidence>,
    pub changelog: Vec<UpgradeEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Dependency {
    name: String,
    version: String,
    configuration: String,
    line: usize,
}

type Dependencies = BTreeMap<String, Dependency>;

/// Assess only visible local evidence, without network access or executing
/// dependency tools. Unsupported/malformed/absent evidence remains unknown.
pub fn assess(changes: &[RepositoryChange], files: &[SourceFile]) -> UpgradeSummary {
    let mut summary = UpgradeSummary::default();
    for change in changes {
        let name = filename(&change.path);
        let ecosystem = match name {
            "Cargo.toml" | "Cargo.lock" => "cargo",
            "package.json" | "package-lock.json" | "npm-shrinkwrap.json" => "npm",
            _ => {
                if unsupported_manifest(name) {
                    summary.unknowns.push(format!(
                        "{}: dependency ecosystem or lockfile is unsupported; upgrade risk is unknown",
                        change.path
                    ));
                }
                continue;
            }
        };
        let before = parse(name, &change.base);
        let after = parse(name, change.content.as_deref().unwrap_or_default());
        let (mut before, mut after) = match (before, after) {
            (Ok(before), Ok(after)) => (before, after),
            (before, after) => {
                for (snapshot, result) in [("base", before), ("current", after)] {
                    if let Err(reason) = result {
                        summary.unknowns.push(format!(
                            "{} ({snapshot}): {reason}; no dependency comparison was inferred",
                            change.path
                        ));
                    }
                }
                continue;
            }
        };
        if name == "Cargo.lock" {
            pair_cargo_versions(&mut before, &mut after, &mut summary.unknowns, &change.path);
        }
        unsupported_configuration(change, name, &mut summary.unknowns);
        for scope in before.keys().chain(after.keys()).collect::<BTreeSet<_>>() {
            let old = before.get(scope);
            let new = after.get(scope);
            if old.zip(new).is_some_and(|(a, b)| {
                a.name == b.name && a.version == b.version && a.configuration == b.configuration
            }) {
                continue;
            }
            // A renamed npm alias/Cargo package is two distinct dependency
            // identities, rather than a fabricated cross-package upgrade.
            if let (Some(a), Some(b)) = (old, new)
                && a.name != b.name
            {
                record(&mut summary, change, ecosystem, scope, Some(a), None, files);
                record(&mut summary, change, ecosystem, scope, None, Some(b), files);
            } else {
                record(&mut summary, change, ecosystem, scope, old, new, files);
            }
        }
    }
    summary.unknowns.sort();
    summary.unknowns.dedup();
    summary
}

fn unsupported_configuration(change: &RepositoryChange, name: &str, unknowns: &mut Vec<String>) {
    if name == "Cargo.toml" {
        let old = change.base.parse::<toml::Value>().ok();
        let new = change
            .content
            .as_deref()
            .unwrap_or_default()
            .parse::<toml::Value>()
            .ok();
        for field in ["patch", "replace", "features"] {
            if old.as_ref().and_then(|value| value.get(field))
                != new.as_ref().and_then(|value| value.get(field))
            {
                unknowns.push(format!("{}: changed Cargo {field} resolution/configuration is not resolved by dependency triage; compatibility is unknown", change.path));
            }
        }
    } else if name == "package.json" {
        let old = serde_json::from_str::<Value>(&change.base).ok();
        let new = serde_json::from_str::<Value>(change.content.as_deref().unwrap_or_default()).ok();
        for field in [
            "overrides",
            "resolutions",
            "workspaces",
            "peerDependenciesMeta",
        ] {
            if old.as_ref().and_then(|value| value.get(field))
                != new.as_ref().and_then(|value| value.get(field))
            {
                unknowns.push(format!("{}: changed npm {field} resolution/configuration is not resolved by dependency triage; compatibility is unknown", change.path));
            }
        }
    }
}

fn record(
    summary: &mut UpgradeSummary,
    change: &RepositoryChange,
    ecosystem: &str,
    scope: &str,
    old: Option<&Dependency>,
    new: Option<&Dependency>,
    files: &[SourceFile],
) {
    let Some(dep) = new.or(old) else { return };
    let kind = match (old, new) {
        (None, _) => "added",
        (_, None) => "removed",
        _ => "changed",
    };
    let risk = match (old, new) {
        (_, None) => "removal",
        (None, _) => "newDependency",
        (Some(a), Some(b)) if a.version == b.version && a.configuration != b.configuration => {
            "unknown"
        }
        (Some(a), Some(b)) => version_risk(&a.version, &b.version),
    };
    let mut evidence = Vec::new();
    if let Some(dep) = old {
        evidence.push(location(
            change.old_path.as_deref().unwrap_or(&change.path),
            "base",
            &change.base,
            dep.line,
        ));
    }
    if let Some(dep) = new {
        evidence.push(location(
            &change.path,
            "current",
            change.content.as_deref().unwrap_or_default(),
            dep.line,
        ));
    }
    for entry in &evidence {
        if entry.line == 0 {
            summary.unknowns.push(format!(
                "{} ({}): {} ({scope}) declaration line could not be located; parsed dependency metadata is retained without an exact citation",
                entry.path, entry.snapshot, dep.name
            ));
        }
    }
    let release = new.and_then(|d| exact_version(&d.version));
    let changelog =
        release.map_or_else(Vec::new, |version| release_notes(&dep.name, version, files));
    if new.is_some() && changelog.is_empty() {
        summary.unknowns.push(format!(
            "{}: {} ({scope}) has no locally matched dependency/version changelog; breaking behavior is unknown",
            change.path, dep.name
        ));
    } else if !changelog.is_empty() {
        summary.unknowns.push(format!(
            "{}: {} changelog evidence covers the target release only; intervening releases and downstream usage remain unverified",
            change.path, dep.name
        ));
    }
    if risk == "unknown" || new.is_some_and(|d| semver(&d.version).is_none()) {
        summary.unknowns.push(format!(
            "{}: {} ({scope}) version/source/configuration is unresolved, ambiguous, changed without version evidence, or non-semver; compatibility is unknown",
            change.path, dep.name
        ));
    }
    let explicit_break = changelog.iter().any(|e| {
        let text = e.text.to_lowercase();
        ![
            "no breaking",
            "not breaking",
            "non-breaking",
            "no incompatible",
            "not incompatible",
        ]
        .iter()
        .any(|word| text.contains(word))
            && ["breaking", "removed", "migration required", "incompatible"]
                .iter()
                .any(|word| text.contains(word))
    });
    let risk = if explicit_break {
        "changelogBreakingSignal"
    } else {
        risk
    };
    let item = DependencyChange {
        ecosystem: ecosystem.into(),
        dependency: dep.name.clone(),
        scope: scope.into(),
        kind: kind.into(),
        risk: risk.into(),
        old_version: old.map(|d| display_version(&d.version)),
        new_version: new.map(|d| display_version(&d.version)),
        evidence,
        changelog,
    };
    // Report compatibility review needs as advisory findings. Missing release
    // notes alone are unknowns, not invented defects or confidence scores.
    if matches!(
        risk,
        "removal" | "majorChange" | "preOneChange" | "downgrade" | "changelogBreakingSignal"
    ) {
        // Finding locations are current-side coordinates. A removed
        // declaration has only historical evidence, so keep its exact base
        // provenance in evidence and leave the finding unlocated (line 0).
        // This routes the advisory to the summary and omits the SARIF region
        // instead of attaching an old line to unrelated or absent current text.
        let line = item
            .evidence
            .iter()
            .rev()
            .find(|evidence| evidence.snapshot == "current")
            .map_or(0, |evidence| evidence.line);
        let mut finding = Finding {
            file: change.path.clone(), line, dimension: Dimension::Compatibility,
            action: Action::Comment, mechanism: "dependencyUpgrade".into(),
            // Probability refers to observing the signal, never to runtime failure.
            probability: 1.0, location_confidence: if line == 0 { 0.0 } else { 1.0 }, mechanism_confidence: 1.0,
            severity: 1.0, severity_confidence: 1.0,
            evidence: item.evidence.iter().chain(&item.changelog).map(|e| if e.line == 0 {
                format!("{} ({}; declaration line unavailable)", e.path, e.snapshot)
            } else { format!("{}:{} ({}) {}", e.path, e.line, e.snapshot, e.text) }).collect::<Vec<_>>().join("\n"),
            title: Some(format!("Review {} dependency {}: {risk}", item.ecosystem, item.dependency)),
            why: Some("Visible version/removal/release-note evidence requires compatibility review; this signal does not establish a runtime defect or estimate breakage probability.".into()),
            fix: Some("Check affected callers and all intervening upstream release notes; apply required migrations or retain the previous dependency until compatibility is verified.".into()),
            test: Some("Build the affected packages and run API/behavior regression tests against the new resolved dependency; verify removed dependencies have no remaining consumers.".into()),
            ..Default::default()
        };
        finding.fingerprint = fingerprint(&finding);
        summary.findings.push(finding);
    }
    summary.changes.push(item);
}

fn filename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn unsupported_manifest(name: &str) -> bool {
    matches!(
        name,
        "pyproject.toml"
            | "requirements.txt"
            | "poetry.lock"
            | "uv.lock"
            | "Pipfile"
            | "Pipfile.lock"
            | "go.mod"
            | "go.sum"
            | "Gemfile"
            | "Gemfile.lock"
            | "pom.xml"
            | "build.gradle"
            | "build.gradle.kts"
            | "yarn.lock"
            | "pnpm-lock.yaml"
            | "bun.lock"
            | "bun.lockb"
            | "composer.json"
            | "composer.lock"
            | "packages.lock.json"
            | "Package.swift"
            | "Package.resolved"
            | "mix.exs"
            | "mix.lock"
            | "deno.json"
            | "deno.jsonc"
            | "deno.lock"
    ) || name.ends_with(".csproj")
        || (name.starts_with("requirements") && name.ends_with(".txt"))
}

fn parse(name: &str, content: &str) -> Result<Dependencies, String> {
    if content.trim().is_empty() {
        return Ok(BTreeMap::new());
    }
    let deps = match name {
        "Cargo.toml" => cargo_manifest(content),
        "Cargo.lock" => cargo_lock(content),
        "package.json" => npm_manifest(content),
        "package-lock.json" | "npm-shrinkwrap.json" => npm_lock(content),
        _ => Err("unsupported manifest".into()),
    }?;
    if deps.len() > MAX_DEPENDENCIES {
        return Err(format!("dependency count exceeds {MAX_DEPENDENCIES}"));
    }
    Ok(deps)
}

fn cargo_manifest(content: &str) -> Result<Dependencies, String> {
    let root = content
        .parse::<toml::Value>()
        .map_err(|_| "invalid TOML".to_string())?;
    let Some(table) = root.as_table() else {
        return Err("manifest must be a TOML table".into());
    };
    let mut deps = BTreeMap::new();
    cargo_tables(table, "", content, &mut deps)?;
    Ok(deps)
}

fn cargo_tables(
    table: &toml::map::Map<String, toml::Value>,
    scope: &str,
    content: &str,
    deps: &mut Dependencies,
) -> Result<(), String> {
    for (key, value) in table {
        let path = if scope.is_empty() {
            key.clone()
        } else {
            format!("{scope}.{key}")
        };
        if matches!(
            key.as_str(),
            "dependencies" | "dev-dependencies" | "build-dependencies"
        ) {
            let Some(entries) = value.as_table() else {
                return Err(format!("{path} must be a dependency table"));
            };
            for (alias, entry) in entries {
                let (name, version, configuration) = if let Some(version) = entry.as_str() {
                    (alias.clone(), version.to_string(), String::new())
                } else if let Some(detail) = entry.as_table() {
                    let name = detail
                        .get("package")
                        .and_then(toml::Value::as_str)
                        .unwrap_or(alias)
                        .to_string();
                    let version = detail
                        .get("version")
                        .and_then(toml::Value::as_str)
                        .unwrap_or("[unresolved/source/workspace dependency]")
                        .to_string();
                    // A changed Git revision/path is not a semver upgrade. Preserve
                    // source identity for comparison but never display its raw URL.
                    let source = ["git", "rev", "tag", "branch", "path"]
                        .iter()
                        .filter_map(|key| detail.get(*key).map(|v| format!("{key}={v}")))
                        .collect::<Vec<_>>()
                        .join(";");
                    let version = if source.is_empty() {
                        version
                    } else {
                        format!("[source] {version} {source}")
                    };
                    let configuration = [
                        "optional",
                        "default-features",
                        "features",
                        "workspace",
                        "registry",
                    ]
                    .iter()
                    .filter_map(|key| detail.get(*key).map(|value| format!("{key}={value}")))
                    .collect::<Vec<_>>()
                    .join(";");
                    (name, version, configuration)
                } else {
                    return Err(format!(
                        "{path}.{alias} has an unsupported dependency declaration"
                    ));
                };
                deps.insert(
                    format!("{path}/{alias}"),
                    Dependency {
                        name,
                        version,
                        configuration,
                        line: cargo_declaration_line(content, &path, alias).unwrap_or(0),
                    },
                );
            }
        } else if (matches!(key.as_str(), "workspace" | "target") || scope == "target")
            && let Some(nested) = value.as_table()
        {
            cargo_tables(nested, &path, content, deps)?;
        }
    }
    Ok(())
}

fn cargo_lock(content: &str) -> Result<Dependencies, String> {
    #[derive(Deserialize)]
    struct LockFile {
        package: Vec<LockPackage>,
    }
    #[derive(Deserialize)]
    struct LockPackage {
        name: String,
        version: toml::Spanned<String>,
        source: Option<String>,
    }
    let root: LockFile = toml::from_str(content).map_err(|_| {
        "invalid TOML lockfile or missing package inventory/name/version".to_string()
    })?;
    let mut groups: BTreeMap<String, Vec<Dependency>> = BTreeMap::new();
    for package in root.package {
        // Workspace/path packages are not an externally resolved upgrade.
        // Path/source changes are separately visible in Cargo manifests.
        let Some(source) = package.source else {
            continue;
        };
        let identity = format!("{}@{:x}", package.name, Sha256::digest(source.as_bytes()));
        let line = content[..package.version.span().start]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count()
            + 1;
        groups.entry(identity).or_default().push(Dependency {
            name: package.name,
            version: package.version.into_inner(),
            configuration: String::new(),
            line,
        });
    }
    let mut deps = BTreeMap::new();
    for (identity, packages) in groups {
        for package in packages {
            let key = format!("lock/{identity}/{}", package.version);
            deps.insert(key, package);
        }
    }
    Ok(deps)
}

/// Match unchanged versions first, and pair one remaining old/new version of
/// the same package/source only. Multiple candidates require abstention.
fn pair_cargo_versions(
    before: &mut Dependencies,
    after: &mut Dependencies,
    unknowns: &mut Vec<String>,
    path: &str,
) {
    let mut old_groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut new_groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for key in before.keys().filter(|key| !after.contains_key(*key)) {
        if let Some((identity, _)) = key.rsplit_once('/') {
            old_groups
                .entry(identity.into())
                .or_default()
                .push(key.clone());
        }
    }
    for key in after.keys().filter(|key| !before.contains_key(*key)) {
        if let Some((identity, _)) = key.rsplit_once('/') {
            new_groups
                .entry(identity.into())
                .or_default()
                .push(key.clone());
        }
    }
    for (identity, old) in old_groups {
        let Some(new) = new_groups.get(&identity) else {
            continue;
        };
        if old.len() == 1 && new.len() == 1 {
            let old_dep = before
                .remove(&old[0])
                .expect("group is built from this inventory");
            let new_dep = after
                .remove(&new[0])
                .expect("group is built from this inventory");
            let key = format!("{identity}/upgrade");
            before.insert(key.clone(), old_dep);
            after.insert(key, new_dep);
        } else {
            unknowns.push(format!("{path}: parallel resolved versions cannot be paired uniquely; additions/removals are reported without inventing upgrade pairs"));
        }
    }
}

fn npm_manifest(content: &str) -> Result<Dependencies, String> {
    let root: Value =
        serde_json::from_str(content).map_err(|_| "invalid JSON manifest".to_string())?;
    if !root.is_object() {
        return Err("package.json must be an object".into());
    }
    let mut deps = BTreeMap::new();
    for scope in [
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
    ] {
        let Some(value) = root.get(scope) else {
            continue;
        };
        let entries = value
            .as_object()
            .ok_or_else(|| format!("{scope} must be an object"))?;
        for (name, version) in entries {
            let version = version
                .as_str()
                .ok_or_else(|| format!("{scope}.{name} must be a string"))?;
            let (actual_name, actual_version) = npm_alias(name, version);
            deps.insert(
                format!("{scope}/{name}"),
                Dependency {
                    name: actual_name,
                    version: actual_version,
                    configuration: String::new(),
                    line: json_property_line(content, &[scope, name]).unwrap_or(0),
                },
            );
        }
    }
    Ok(deps)
}

fn npm_lock(content: &str) -> Result<Dependencies, String> {
    let root: Value =
        serde_json::from_str(content).map_err(|_| "invalid JSON lockfile".to_string())?;
    if root
        .get("lockfileVersion")
        .is_some_and(|version| !matches!(version.as_u64(), Some(1..=3)))
    {
        return Err("unsupported npm lockfileVersion (supported: 1..=3)".into());
    }
    let mut deps = BTreeMap::new();
    if let Some(packages) = root.get("packages") {
        let packages = packages
            .as_object()
            .ok_or("lock packages must be an object")?;
        for (path, value) in packages {
            if path.is_empty() {
                continue;
            }
            if !path.starts_with("node_modules/") && !path.contains("/node_modules/") {
                continue;
            }
            if value.get("link").and_then(Value::as_bool) == Some(true) {
                continue;
            }
            let name = value
                .get("name")
                .and_then(Value::as_str)
                .or_else(|| path.rsplit("node_modules/").next())
                .ok_or("lock package name missing")?;
            let version = value
                .get("version")
                .and_then(Value::as_str)
                .unwrap_or("[unresolved workspace/link dependency]");
            deps.insert(
                format!("lock/{path}"),
                Dependency {
                    name: name.into(),
                    version: version.into(),
                    configuration: String::new(),
                    line: json_property_line(content, &["packages", path, "version"]).unwrap_or(0),
                },
            );
        }
    } else if let Some(entries) = root.get("dependencies") {
        npm_v1(
            entries,
            "lock",
            content,
            &mut deps,
            &mut vec!["dependencies".to_string()],
            0,
        )?;
    } else {
        return Err("package-lock.json has no supported package inventory".into());
    }
    Ok(deps)
}

fn npm_v1(
    value: &Value,
    scope: &str,
    content: &str,
    deps: &mut Dependencies,
    keys: &mut Vec<String>,
    depth: usize,
) -> Result<(), String> {
    if depth > 32 || deps.len() > MAX_DEPENDENCIES {
        return Err("lock dependency nesting/count exceeds limits".into());
    }
    let entries = value
        .as_object()
        .ok_or("lock dependencies must be an object")?;
    for (name, value) in entries {
        let version = value
            .get("version")
            .and_then(Value::as_str)
            .ok_or("lock dependency version is missing")?;
        let path = format!("{scope}/node_modules/{name}");
        keys.push(name.clone());
        keys.push("version".into());
        let key_refs: Vec<_> = keys.iter().map(String::as_str).collect();
        let (actual_name, actual_version) = npm_alias(name, version);
        deps.insert(
            path.clone(),
            Dependency {
                name: actual_name,
                version: actual_version,
                configuration: String::new(),
                line: json_property_line(content, &key_refs).unwrap_or(0),
            },
        );
        keys.pop();
        if let Some(nested) = value.get("dependencies") {
            keys.push("dependencies".into());
            npm_v1(nested, &path, content, deps, keys, depth + 1)?;
            keys.pop();
        }
        keys.pop();
    }
    Ok(())
}

fn cargo_declaration_line(content: &str, scope: &str, name: &str) -> Option<usize> {
    // Line scanning cannot distinguish section-like text inside TOML
    // multiline strings from declarations. Abstain rather than cite it.
    if content.contains("\"\"\"") || content.contains("'''") {
        return None;
    }
    let mut section = String::new();
    let mut table_anchor = None;
    for (index, line) in content.lines().enumerate() {
        let line = line.trim_start();
        if let Some(header) = line
            .strip_prefix('[')
            .and_then(|line| line.split(']').next())
        {
            section = header.replace(['\"', '\''], "");
            if section == format!("{scope}.{name}") {
                table_anchor = Some(index + 1);
            }
            continue;
        }
        if section == format!("{scope}.{name}")
            && line
                .strip_prefix("version")
                .is_some_and(|rest| rest.trim_start().starts_with('='))
        {
            return Some(index + 1);
        }
        if section == scope
            && (line
                .strip_prefix(name)
                .is_some_and(|rest| rest.trim_start().starts_with('='))
                || line.starts_with(&format!("\"{name}\""))
                || line.starts_with(&format!("'{name}'")))
        {
            return Some(index + 1);
        }
    }
    table_anchor
}

fn npm_alias(name: &str, version: &str) -> (String, String) {
    if let Some(alias) = version.strip_prefix("npm:")
        && let Some((actual, requirement)) = alias.rsplit_once('@')
        && !actual.is_empty()
        && !requirement.is_empty()
    {
        (actual.into(), requirement.into())
    } else {
        (name.into(), version.into())
    }
}

/// Locate a property in a validated JSON document by its object path. Parsing
/// string boundaries prevents repeated names in distinct dependency groups
/// from citing the first unrelated occurrence.
fn json_property_line(content: &str, keys: &[&str]) -> Option<usize> {
    if keys.is_empty() {
        return None;
    }
    let bytes = content.as_bytes();
    let mut start = 0;
    let mut end = bytes.len();
    let mut anchor = 0;
    for wanted in keys {
        let mut cursor = start;
        while cursor < end && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if bytes.get(cursor) != Some(&b'{') {
            return None;
        }
        cursor += 1;
        let mut found = None;
        while cursor < end {
            while cursor < end && (bytes[cursor].is_ascii_whitespace() || bytes[cursor] == b',') {
                cursor += 1;
            }
            if bytes.get(cursor) != Some(&b'"') {
                break;
            }
            let key_start = cursor;
            let key_end = json_value_end(bytes, cursor, end);
            let key: String = serde_json::from_str(&content[key_start..key_end]).ok()?;
            cursor = key_end;
            while cursor < end && (bytes[cursor].is_ascii_whitespace() || bytes[cursor] == b':') {
                cursor += 1;
            }
            let value_start = cursor;
            let value_end = json_value_end(bytes, cursor, end);
            if key == *wanted {
                // serde_json accepts duplicate keys with last-wins semantics.
                // A non-unique declaration cannot safely cite the first key.
                if found.is_some() {
                    return None;
                }
                found = Some((key_start, value_start, value_end));
            }
            cursor = value_end;
        }
        let (position, value_start, value_end) = found?;
        anchor = position;
        start = value_start;
        end = value_end;
    }
    Some(
        content[..anchor]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count()
            + 1,
    )
}

fn json_value_end(bytes: &[u8], start: usize, end: usize) -> usize {
    let mut quoted = false;
    let mut escaped = false;
    let mut depth = 0usize;
    for (index, byte) in bytes.iter().enumerate().take(end).skip(start) {
        if quoted {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                quoted = false;
                if depth == 0 {
                    return index + 1;
                }
            }
        } else {
            match byte {
                b'"' => quoted = true,
                b'{' | b'[' => depth += 1,
                b'}' | b']' => {
                    if depth == 0 {
                        return index;
                    }
                    depth -= 1;
                    if depth == 0 {
                        return index + 1;
                    }
                }
                b',' if depth == 0 => return index,
                _ => {}
            }
        }
    }
    end
}

fn location(path: &str, snapshot: &str, text: &str, line: usize) -> UpgradeEvidence {
    let excerpt = line
        .checked_sub(1)
        .and_then(|index| text.lines().nth(index));
    UpgradeEvidence {
        path: path.into(),
        snapshot: snapshot.into(),
        line: if excerpt.is_some() { line } else { 0 },
        text: excerpt.unwrap_or_default().into(),
    }
}

fn exact_version(raw: &str) -> Option<&str> {
    let raw = raw.trim().strip_prefix('v').unwrap_or(raw.trim());
    let parts: Vec<_> = raw.split('.').collect();
    (parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty()
                && (part.len() == 1 || !part.starts_with('0'))
                && part.bytes().all(|b| b.is_ascii_digit())
        }))
    .then_some(raw)
}

fn semver(raw: &str) -> Option<(u64, u64, u64)> {
    let raw = raw.trim();
    // A single caret/tilde requirement can yield a compatibility signal.
    // Disjunctions, comparator ranges and prereleases intentionally abstain.
    let raw = raw
        .strip_prefix('^')
        .or_else(|| raw.strip_prefix('~'))
        .or_else(|| raw.strip_prefix('='))
        .unwrap_or(raw);
    let parts: Vec<_> = raw.split('.').collect();
    if parts.is_empty()
        || parts.len() > 3
        || parts.iter().any(|part| {
            part.is_empty()
                || (part.len() > 1 && part.starts_with('0'))
                || !part.bytes().all(|byte| byte.is_ascii_digit())
        })
    {
        return None;
    }
    Some((
        parts[0].parse().ok()?,
        parts.get(1).map_or(Some(0), |part| part.parse().ok())?,
        parts.get(2).map_or(Some(0), |part| part.parse().ok())?,
    ))
}

fn display_version(raw: &str) -> String {
    if semver(raw).is_some() {
        raw.into()
    } else {
        "[unresolved or unsupported requirement]".into()
    }
}

fn version_risk(old: &str, new: &str) -> &'static str {
    let (Some(a), Some(b)) = (semver(old), semver(new)) else {
        return "unknown";
    };
    if b < a {
        "downgrade"
    } else if a.0 != b.0 {
        "majorChange"
    } else if a.0 == 0 && (a.1 != b.1 || (a.1 == 0 && a.2 != b.2)) {
        "preOneChange"
    } else {
        "semverCompatibleSignal"
    }
}

fn release_notes(name: &str, version: &str, files: &[SourceFile]) -> Vec<UpgradeEvidence> {
    let mut evidence = Vec::new();
    for file in files {
        let lower = file.path.to_lowercase();
        if !["changelog", "changes", "release"]
            .iter()
            .any(|word| filename(&lower).contains(word))
        {
            continue;
        }
        let identity_path = lower.split('/').any(|part| part == name.to_lowercase())
            || filename(&lower).starts_with(&format!("{}-", name.to_lowercase()));
        let lines: Vec<_> = file.content.lines().collect();
        for (index, line) in lines.iter().enumerate() {
            if !line.trim_start().starts_with('#') {
                continue;
            }
            let tokens: Vec<_> = line
                .split(|c: char| !(c.is_alphanumeric() || matches!(c, '.' | '_' | '-' | '/' | '@')))
                .collect();
            let version_match = tokens
                .iter()
                .any(|token| token.strip_prefix('v').unwrap_or(token) == version);
            let identity_heading = tokens.iter().any(|token| token.eq_ignore_ascii_case(name));
            if !(version_match && (identity_path || identity_heading)) {
                continue;
            }
            let heading_level = line
                .trim_start()
                .bytes()
                .take_while(|byte| *byte == b'#')
                .count();
            evidence.push(location(&file.path, "current", &file.content, index + 1));
            for (offset, body) in lines.iter().skip(index + 1).take(12).enumerate() {
                let body_level = body
                    .trim_start()
                    .bytes()
                    .take_while(|byte| *byte == b'#')
                    .count();
                if body_level > 0 && body_level <= heading_level {
                    break;
                }
                if !body.trim().is_empty() {
                    evidence.push(location(
                        &file.path,
                        "current",
                        &file.content,
                        index + offset + 2,
                    ));
                }
            }
        }
    }
    evidence
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(path: &str, base: &str, current: Option<&str>) -> RepositoryChange {
        RepositoryChange {
            path: path.into(),
            old_path: None,
            base: base.into(),
            content: current.map(str::to_string),
        }
    }
    fn npm(old: &str, new: &str) -> RepositoryChange {
        change(
            "package.json",
            &format!("{{\"dependencies\":{{\"lib\":\"{old}\"}}}}"),
            Some(&format!("{{\"dependencies\":{{\"lib\":\"{new}\"}}}}")),
        )
    }

    #[test]
    fn ordinary_upgrade_is_only_a_signal_and_missing_notes_stay_unknown() {
        let report = assess(&[npm("^1.2.0", "^1.3.0")], &[]);
        assert_eq!(report.changes[0].risk, "semverCompatibleSignal");
        assert!(report.findings.is_empty());
        assert!(
            report
                .unknowns
                .iter()
                .any(|s| s.contains("no locally matched"))
        );
    }

    #[test]
    fn major_preone_downgrade_and_removal_are_actionable_advisories() {
        for (old, new, risk) in [
            ("1.0.0", "2.0.0", "majorChange"),
            ("0.2.0", "0.3.0", "preOneChange"),
            ("0.0.1", "0.0.2", "preOneChange"),
            ("2.0.0", "1.9.0", "downgrade"),
        ] {
            let report = assess(&[npm(old, new)], &[]);
            assert_eq!(report.changes[0].risk, risk);
            assert_eq!(report.findings[0].action, Action::Comment);
            assert!(!report.findings[0].fingerprint.is_empty());
            assert!(report.findings[0].fix.as_ref().unwrap().contains("callers"));
        }
        let report = assess(
            &[change(
                "Cargo.toml",
                "[dependencies]\nlib = \"1.0.0\"\n",
                Some("[dependencies]\n"),
            )],
            &[],
        );
        assert_eq!(report.changes[0].kind, "removed");
        assert_eq!(report.changes[0].evidence[0].snapshot, "base");
        assert_eq!(report.changes[0].evidence[0].line, 2);
    }

    #[test]
    fn only_dependency_and_exact_version_matched_notes_can_support_findings() {
        let files = [
            SourceFile {
                path: "CHANGELOG.md".into(),
                content: "# 1.3.0\nBreaking everything\n".into(),
            },
            SourceFile {
                path: "vendor/other/CHANGELOG.md".into(),
                content: "# 1.3.0\nBreaking everything\n".into(),
            },
            SourceFile {
                path: "vendor/lib/CHANGELOG.md".into(),
                content: "# 1.3.0\n- Breaking: removed legacy API\n# 1.2.0\n- ordinary fix\n"
                    .into(),
            },
        ];
        let report = assess(&[npm("1.2.0", "1.3.0")], &files);
        assert_eq!(report.changes[0].risk, "changelogBreakingSignal");
        assert_eq!(report.changes[0].changelog.len(), 2);
        assert_eq!(report.changes[0].changelog[1].line, 2);
        assert!(
            report.changes[0]
                .changelog
                .iter()
                .all(|e| e.path == "vendor/lib/CHANGELOG.md")
        );
        assert!(
            report
                .unknowns
                .iter()
                .any(|s| s.contains("intervening releases"))
        );
        assert!(
            assess(&[npm("1.2.0", "1.3.0")], &files[..2])
                .findings
                .is_empty()
        );
    }

    #[test]
    fn unavailable_versions_malformed_manifests_and_unsupported_ecosystems_abstain() {
        for version in [
            "latest",
            "workspace:*",
            "^1 || ^2",
            "1.2.3-beta.1",
            "git+https://token@host/repo",
        ] {
            let report = assess(&[npm("1.2.0", version)], &[]);
            assert_eq!(report.changes[0].risk, "unknown");
            assert_eq!(
                report.changes[0].new_version.as_deref(),
                Some("[unresolved or unsupported requirement]")
            );
        }
        let report = assess(
            &[
                change(
                    "Cargo.toml",
                    "[dependencies]\nlib = \"1.0.0\"",
                    Some("invalid ["),
                ),
                change("go.mod", "", Some("module example")),
            ],
            &[],
        );
        assert!(report.changes.is_empty());
        assert_eq!(report.unknowns.len(), 2);
    }

    #[test]
    fn cargo_alias_workspace_target_and_source_versions_are_not_invented() {
        let base = "[workspace.dependencies]\nshared = \"1.0.0\"\n[target.'cfg(unix)'.dependencies]\nalias = { package = \"actual\", version = \"1.0.0\" }\n[dependencies]\nshared = { workspace = true }\n";
        let current = base.replace("1.0.0", "2.0.0");
        let report = assess(&[change("Cargo.toml", base, Some(&current))], &[]);
        assert_eq!(report.changes.len(), 2);
        assert!(report.changes.iter().any(|d| d.dependency == "actual"));
        let report = assess(
            &[change(
                "Cargo.toml",
                "",
                Some("[dependencies]\nshared = { workspace = true }\n"),
            )],
            &[],
        );
        assert!(report.unknowns.iter().any(|s| s.contains("unresolved")));
        let report = assess(
            &[change(
                "Cargo.toml",
                "[dependencies]\nlib = { git = \"https://host/a\", rev = \"a\" }",
                Some("[dependencies]\nlib = { git = \"https://host/a\", rev = \"b\" }"),
            )],
            &[],
        );
        assert_eq!(report.changes[0].risk, "unknown");
    }

    #[test]
    fn lock_only_upgrades_and_nested_npm_packages_are_detected() {
        let base = "version = 3\n\n[[package]]\nname = \"lib\"\nversion = \"1.0.0\"\nsource = \"registry+https://example.test\"\n";
        let current = base.replace("1.0.0", "2.0.0");
        let report = assess(&[change("Cargo.lock", base, Some(&current))], &[]);
        assert_eq!(report.changes[0].risk, "majorChange");
        assert_eq!(report.changes[0].evidence[1].line, 5);
        let base = "{\"lockfileVersion\":3,\"packages\":{\"\":{\"version\":\"1.0.0\"},\"node_modules/lib\":{\"version\":\"1.0.0\"},\"node_modules/a/node_modules/lib\":{\"version\":\"1.0.0\"}}}";
        let report = assess(
            &[change(
                "package-lock.json",
                base,
                Some(&base.replace("\"version\":\"1.0.0\"", "\"version\":\"2.0.0\"")),
            )],
            &[],
        );
        assert_eq!(report.changes.len(), 2);
        assert!(report.changes.iter().all(|d| d.dependency == "lib"));
    }

    #[test]
    fn additions_deletions_renames_and_v1_locks_keep_identity() {
        let mut renamed = npm("1.0.0", "1.0.0");
        renamed.path = "app/package.json".into();
        renamed.old_path = Some("old/package.json".into());
        assert!(assess(&[renamed], &[]).changes.is_empty());
        let base = "{\"dependencies\":{\"lib\":{\"version\":\"1.0.0\",\"dependencies\":{\"child\":{\"version\":\"1.0.0\"}}}}}";
        let report = assess(&[change("package-lock.json", base, None)], &[]);
        assert_eq!(report.changes.len(), 2);
        assert!(report.changes.iter().all(|d| d.kind == "removed"));
        let report = assess(
            &[change(
                "package.json",
                "",
                Some("{\"dependencies\":{\"lib\":\"1.0.0\"}}"),
            )],
            &[],
        );
        assert_eq!(report.changes[0].kind, "added");
        assert_eq!(report.changes[0].risk, "newDependency");
    }

    #[test]
    fn repeated_scopes_and_nested_json_cite_the_changed_declaration() {
        let base = "[dependencies]\nlib = \"1\"\n[dev-dependencies]\nlib = \"1\"\n";
        let current = "[dependencies]\nlib = \"1\"\n[dev-dependencies]\nlib = \"2\"\n";
        let report = assess(&[change("Cargo.toml", base, Some(current))], &[]);
        assert_eq!(report.changes[0].evidence[1].line, 4);
        assert_eq!(report.changes[0].risk, "majorChange");
        let base = "{\n\"dependencies\": {\"lib\": \"1.0.0\"},\n\"devDependencies\": {\n\"lib\": \"1.0.0\"\n}\n}";
        let current = base.replacen("\"lib\": \"1.0.0\"\n", "\"lib\": \"2.0.0\"\n", 1);
        let report = assess(&[change("package.json", base, Some(&current))], &[]);
        assert_eq!(report.changes[0].evidence[1].line, 4);
    }

    #[test]
    fn parallel_cargo_versions_match_only_unique_remaining_pairs() {
        let package = |version: &str| {
            format!(
                "[[package]]\nname = \"lib\"\nversion = \"{version}\"\nsource = \"registry+https://example.test\"\n"
            )
        };
        let base = format!("{}{}", package("1.0.0"), package("2.0.0"));
        let current = format!("{}{}", package("1.0.0"), package("3.0.0"));
        let report = assess(&[change("Cargo.lock", &base, Some(&current))], &[]);
        assert_eq!(report.changes.len(), 1);
        assert_eq!(report.changes[0].old_version.as_deref(), Some("2.0.0"));
        assert_eq!(report.changes[0].new_version.as_deref(), Some("3.0.0"));
        let current = format!("{}{}", package("3.0.0"), package("4.0.0"));
        let report = assess(&[change("Cargo.lock", &base, Some(&current))], &[]);
        assert_eq!(report.changes.len(), 4);
        assert!(
            report
                .unknowns
                .iter()
                .any(|s| s.contains("cannot be paired uniquely"))
        );
    }

    #[test]
    fn release_subsections_and_negative_breaking_statements_are_handled() {
        let notes = |content: &str| SourceFile {
            path: "vendor/lib/CHANGELOG.md".into(),
            content: content.into(),
        };
        let report = assess(
            &[npm("1.2.0", "1.3.0")],
            &[notes(
                "## 1.3.0\n### Breaking Changes\n- Removed legacy API\n## 1.2.0\n- Fixed old bug",
            )],
        );
        assert_eq!(report.changes[0].risk, "changelogBreakingSignal");
        assert_eq!(report.changes[0].changelog.len(), 3);
        let report = assess(
            &[npm("1.2.0", "1.3.0")],
            &[notes("## 1.3.0\n- No breaking changes\n")],
        );
        assert!(report.findings.is_empty());
        let report = assess(
            &[change(
                "package.json",
                "",
                Some("{\"dependencies\":{\"alias\":\"npm:@scope/pkg@1.2.3\"}}"),
            )],
            &[],
        );
        assert_eq!(report.changes[0].dependency, "@scope/pkg");
        assert_eq!(report.changes[0].new_version.as_deref(), Some("1.2.3"));
    }

    #[test]
    fn lock_locations_follow_toml_spans_and_subtable_versions() {
        let base = "[[package]]\nname='lib'\nversion='1.0.0'\nsource='registry+a'\n[[package]]\nname='lib'\nversion='1.0.0'\nsource='registry+b'\n";
        let current = base.replacen(
            "version='1.0.0'\nsource='registry+b'",
            "version='2.0.0'\nsource='registry+b'",
            1,
        );
        let report = assess(&[change("Cargo.lock", base, Some(&current))], &[]);
        assert_eq!(report.changes.len(), 1);
        assert_eq!(report.changes[0].evidence[1].line, 7);
        assert_eq!(report.changes[0].evidence[1].text, "version='2.0.0'");
        let base = "[dependencies.lib]\nversion='1.0.0'\n";
        let report = assess(
            &[change(
                "Cargo.toml",
                base,
                Some("[dependencies.lib]\nversion='2.0.0'\n"),
            )],
            &[],
        );
        assert_eq!(report.changes[0].evidence[1].line, 2);
        assert!(report.findings[0].evidence.contains("2.0.0"));
    }

    #[test]
    fn shrinkwrap_limits_and_invalid_semver_remain_explicit() {
        let report = assess(
            &[change(
                "npm-shrinkwrap.json",
                "",
                Some("{\"dependencies\":{\"lib\":{\"version\":\"1.0.0\"}}}"),
            )],
            &[],
        );
        assert_eq!(report.changes.len(), 1);
        assert_eq!(report.changes[0].ecosystem, "npm");
        for version in ["01.0.0", "1.02.0", "1.0.03"] {
            assert!(semver(version).is_none());
            assert!(exact_version(version).is_none());
        }
        let mut deps = serde_json::Map::new();
        for index in 0..=MAX_DEPENDENCIES {
            deps.insert(format!("dep{index}"), Value::String("1.0.0".into()));
        }
        let manifest = serde_json::json!({"dependencies":deps}).to_string();
        let report = assess(&[change("package.json", "", Some(&manifest))], &[]);
        assert!(report.changes.is_empty());
        assert!(
            report
                .unknowns
                .iter()
                .any(|s| s.contains("dependency count exceeds"))
        );
    }
    #[test]
    fn configuration_changes_are_unknown_and_lock_migrations_do_not_invent_upgrades() {
        let report = assess(
            &[change(
                "Cargo.toml",
                "[dependencies]\nlib = { version='1.0.0', features=['a'] }",
                Some("[dependencies]\nlib = { version='1.0.0', features=['b'] }"),
            )],
            &[],
        );
        assert_eq!(report.changes.len(), 1);
        assert_eq!(report.changes[0].risk, "unknown");
        let report = assess(
            &[
                change(
                    "package.json",
                    "{}",
                    Some(r#"{"overrides":{"lib":"2.0.0"}}"#),
                ),
                change(
                    "Cargo.toml",
                    "",
                    Some("[patch.crates-io]\nlib = { path='local' }"),
                ),
            ],
            &[],
        );
        assert!(report.unknowns.iter().any(|s| s.contains("overrides")));
        assert!(report.unknowns.iter().any(|s| s.contains("patch")));
        let old = r#"{"lockfileVersion":1,"dependencies":{"lib":{"version":"1.0.0"}}}"#;
        let new = r#"{"lockfileVersion":3,"packages":{"node_modules/lib":{"version":"1.0.0"},"packages/local":{"version":"2.0.0"}}}"#;
        assert!(
            assess(&[change("package-lock.json", old, Some(new))], &[])
                .changes
                .is_empty()
        );
        let report = assess(
            &[change(
                "package-lock.json",
                "",
                Some(r#"{"lockfileVersion":99,"packages":{}}"#),
            )],
            &[],
        );
        assert!(
            report
                .unknowns
                .iter()
                .any(|s| s.contains("unsupported npm lockfileVersion"))
        );
    }
    #[test]
    fn removed_dependencies_keep_base_provenance_without_current_side_anchors() {
        use crate::domain::github_review::{Posted, PrFile, SummaryReason, plan};
        use crate::domain::report::ReviewReport;
        let base = "# former preamble\n\n[dependencies]\nremoved = '1.0.0'\n";
        let current = "[dependencies]\n\n\n# unrelated current line four\n";
        let shifted = change("Cargo.toml", base, Some(current));
        let deleted = change("old/Cargo.toml", base, None);
        let mut renamed = change("new/Cargo.toml", base, Some(current));
        renamed.old_path = Some("original/Cargo.toml".into());
        for input in [shifted, deleted, renamed] {
            let summary = assess(std::slice::from_ref(&input), &[]);
            let finding = &summary.findings[0];
            assert_eq!(finding.line, 0);
            assert_eq!(finding.location_confidence, 0.0);
            let expected_base_path = input.old_path.as_deref().unwrap_or(&input.path);
            assert_eq!(summary.changes[0].evidence[0].path, expected_base_path);
            assert_eq!(summary.changes[0].evidence[0].snapshot, "base");
            assert_eq!(summary.changes[0].evidence[0].line, 4);
            assert!(
                finding
                    .evidence
                    .contains(&format!("{expected_base_path}:4 (base) removed = '1.0.0'"))
            );
            let report = ReviewReport {
                findings: summary.findings,
                ..Default::default()
            };
            // Old line 4 is deliberately commentable on the current side:
            // copying the base coordinate would publish against unrelated text.
            let pr_files = [PrFile {
                filename: input.path,
                patch: Some(if input.content.is_none() {
                    "@@ -1,4 +0,0 @@\n-# former preamble\n-\n-[dependencies]\n-removed = '1.0.0'"
                        .into()
                } else {
                    "@@ -4 +4 @@\n-removed = '1.0.0'\n+# unrelated current line four".into()
                }),
            }];
            let publish = plan(&report, &pr_files, &Posted::default(), 10);
            assert!(publish.inline.is_empty());
            assert_eq!(publish.summary_only[0].1, SummaryReason::OutsideDiff);
            let sarif = crate::adapters::sarif::to_sarif(&report);
            assert!(
                sarif["runs"][0]["results"][0]["locations"][0]["physicalLocation"]
                    .get("region")
                    .is_none()
            );
        }
        let summary = assess(&[npm("1.0.0", "2.0.0")], &[]);
        assert_eq!(
            summary.findings[0].line,
            summary.changes[0].evidence[1].line
        );
        assert_eq!(summary.findings[0].location_confidence, 1.0);
    }

    #[test]
    fn decoded_manifest_without_a_unique_source_location_stays_unlocated() {
        // These are valid decoder inputs, beyond the line scanner's syntax.
        for (base, current) in [
            (
                "# unrelated header\n[dependencies]\n\"l\\u0069b\" = '1.0.0'\n",
                "# unrelated header\n[dependencies]\n\"l\\u0069b\" = '2.0.0'\n",
            ),
            (
                "# unrelated header\ndependencies = { lib = '1.0.0' }\n",
                "# unrelated header\ndependencies = { lib = '2.0.0' }\n",
            ),
        ] {
            let summary = assess(&[change("Cargo.toml", base, Some(current))], &[]);
            assert_eq!(summary.changes[0].dependency, "lib");
            assert_eq!(summary.changes[0].new_version.as_deref(), Some("2.0.0"));
            assert_eq!(summary.findings[0].line, 0);
            assert_eq!(summary.findings[0].location_confidence, 0.0);
            assert!(
                summary.changes[0]
                    .evidence
                    .iter()
                    .all(|entry| entry.line == 0 && entry.text.is_empty())
            );
            assert!(
                summary
                    .unknowns
                    .iter()
                    .any(|reason| reason.contains("declaration line could not be located"))
            );
            assert!(!summary.findings[0].evidence.contains("unrelated header"));
            assert!(
                summary.findings[0]
                    .evidence
                    .contains("current; declaration line unavailable")
            );
        }
        let body = "{\n\"dependencies\": {\n\"lib\": \"0.2.0\",\n\"\\u006cib\": \"2.0.0\"\n}\n}";
        let summary = assess(
            &[change(
                "package.json",
                r#"{"dependencies":{"lib":"1.0.0"}}"#,
                Some(body),
            )],
            &[],
        );
        // The JSON decoder chooses the last duplicate key. Neither the first
        // key nor line 1 is a defensible exact declaration citation.
        assert_eq!(summary.changes[0].new_version.as_deref(), Some("2.0.0"));
        assert_eq!(summary.changes[0].evidence[1].line, 0);
        assert!(summary.changes[0].evidence[1].text.is_empty());
        assert_eq!(summary.findings[0].line, 0);
        assert!(
            summary
                .unknowns
                .iter()
                .any(|reason| reason.contains("current")
                    && reason.contains("declaration line could not be located"))
        );
    }

    #[test]
    fn locator_abstention_and_multiline_literal_do_not_fabricate_citations() {
        assert_eq!(json_property_line("{\"present\":1}", &["absent"]), None);
        assert_eq!(
            json_property_line("{\"present\":1}", &["present", "absent"]),
            None
        );
        let escaped = "{\n\"dependencies\": {\n\"l\\u0069b\": \"2.0.0\"\n}\n}";
        assert_eq!(
            json_property_line(escaped, &["dependencies", "lib"]),
            Some(3)
        );
        let misleading = "[package]\ndescription = '''\n[dependencies]\nlib='99.0.0'\n'''\n[dependencies]\nlib='2.0.0'\n";
        assert_eq!(
            parse("Cargo.toml", misleading).unwrap()["dependencies/lib"].version,
            "2.0.0"
        );
        assert_eq!(
            cargo_declaration_line(misleading, "dependencies", "lib"),
            None
        );
        let absent = location("package.json", "current", "unrelated first line", 0);
        assert_eq!(absent.line, 0);
        assert!(absent.text.is_empty());
        assert_eq!(location("package.json", "current", "one line", 9).line, 0);
    }
}
