//! Exercise the supported auxiliary benchmark slice without model calls.
use momus_review::domain::{report::SourceFile, repository::RepositoryChange};
use momus_review::review::{docs_drift, upgrades};
use std::{collections::BTreeMap, fs, path::Path};

fn files(root: &Path) -> BTreeMap<String, String> {
    fn visit(root: &Path, dir: &Path, output: &mut BTreeMap<String, String>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(root, &path, output);
            } else {
                output.insert(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .to_owned(),
                    fs::read_to_string(path).unwrap(),
                );
            }
        }
    }
    let mut output = BTreeMap::new();
    visit(root, root, &mut output);
    output
}

fn evidence(case: &str) -> (Vec<RepositoryChange>, Vec<SourceFile>) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("eval/momus");
    let manifest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root.join("manifest.json")).unwrap()).unwrap();
    let case = manifest["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == case)
        .unwrap();
    let base = files(&root.join(case["base"].as_str().unwrap()));
    let head = files(&root.join(case["head"].as_str().unwrap()));
    let changes = base
        .keys()
        .chain(head.keys())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .filter(|path| base.get(*path) != head.get(*path))
        .map(|path| RepositoryChange {
            path: path.clone(),
            old_path: None,
            base: base.get(path).cloned().unwrap_or_default(),
            content: head.get(path).cloned(),
        })
        .collect();
    let current = head
        .into_iter()
        .map(|(path, content)| SourceFile { path, content })
        .collect();
    (changes, current)
}

#[test]
fn supported_documentation_pair_has_a_concrete_mismatch_and_fixed_control() {
    let (changes, current) = evidence("docs-client-arity-defect");
    let bad = docs_drift::assess(&changes, &current);
    assert!(bad.unknowns.is_empty(), "{:?}", bad.unknowns);
    assert!(
        bad.findings
            .iter()
            .any(|finding| finding.file == "README.md" && finding.line == 7),
        "{:?}",
        bad
    );
    let (changes, current) = evidence("docs-client-arity-fixed");
    let fixed = docs_drift::assess(&changes, &current);
    assert!(fixed.unknowns.is_empty(), "{:?}", fixed.unknowns);
    assert!(fixed.findings.is_empty(), "{:?}", fixed);
    assert!(
        fixed
            .checks
            .iter()
            .any(|check| check.status == "consistent")
    );
}

#[test]
fn supported_dependency_pair_has_target_notes_but_advisory_is_not_a_defect_label() {
    for case in ["dependency-codec-api-defect", "dependency-codec-api-fixed"] {
        let (changes, current) = evidence(case);
        let summary = upgrades::assess(&changes, &current);
        assert!(
            summary.unknowns.iter().all(|unknown| unknown.contains(
                "target release only; intervening releases and downstream usage remain unverified"
            )),
            "{case}: {:?}",
            summary.unknowns
        );
        assert!(
            summary
                .changes
                .iter()
                .any(|change| change.dependency == "codec-fixture"
                    && change.new_version.as_deref() == Some("2.0.0")
                    && !change.changelog.is_empty()),
            "{case}: {:?}",
            summary
        );
        // Both snapshots upgraded: a local version/changelog advisory is valid
        // in each, and requires separate caller evidence to establish a defect.
        assert!(!summary.findings.is_empty());
    }
}
