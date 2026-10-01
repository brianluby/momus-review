//! Local repository tours from bounded source and corroborated import edges.
use crate::{
    adapters::{git::RepositoryEvidence, index_store::IndexStore},
    domain::{
        language::{is_source_path, is_test_path},
        report::SourceFile,
    },
    review::index::RepoIndex,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reference {
    pub path: String,
    pub line: usize,
    pub text: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TourStop {
    pub role: String,
    pub explanation: String,
    pub reference: Reference,
    pub public_surface: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Relationship {
    pub source: Reference,
    pub target: Reference,
    pub kind: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Component {
    pub path: String,
    pub files: usize,
    pub roles: Vec<String>,
    pub reference: Reference,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tour {
    pub head: String,
    pub basis: String,
    pub stops: Vec<TourStop>,
    pub components: Vec<Component>,
    pub relationships: Vec<Relationship>,
    pub unknowns: Vec<String>,
    /// Evidence gaps in inventory, entrypoints or corroborated edges. The
    /// fixed static-map disclosure alone does not mark a tour incomplete.
    pub partial: bool,
}

fn reference(file: &SourceFile, line: usize) -> Reference {
    Reference {
        path: file.path.clone(),
        line,
        text: file
            .content
            .lines()
            .nth(line.saturating_sub(1))
            .unwrap_or_default()
            .chars()
            .take(240)
            .collect(),
    }
}

fn role(file: &SourceFile) -> (&'static str, usize, &'static str) {
    let path = file.path.to_ascii_lowercase();
    let lines: Vec<_> = file.content.lines().collect();
    if let Some(line) = lines.iter().position(|s| {
        s.trim_start().starts_with("fn main(")
            || s.trim_start().starts_with("async fn main(")
            || s.trim_start().starts_with("func main(")
            || s.trim_start().starts_with("if __name__ ==")
            || s.trim_start().starts_with("app.listen(")
            || s.trim_start().starts_with("createServer(")
    }) {
        return (
            "entrypoint",
            line + 1,
            "Runtime entry declaration; follow its imports to the components it assembles.",
        );
    }
    if path == "lib.rs"
        || path == "index.ts"
        || path == "index.js"
        || path == "__init__.py"
        || path.ends_with("/lib.rs")
        || path.ends_with("/__init__.py")
        || path.ends_with("/index.ts")
        || path.ends_with("/index.js")
    {
        return (
            "entrypoint",
            1,
            "Conventional package entry path; exported signatures describe the visible package surface.",
        );
    }
    let role = if path.contains("/domain/") || path.contains("/models/") {
        "domain"
    } else if ["/auth/", "/api/", "/routes/", "/validation/"]
        .iter()
        .any(|s| path.contains(s))
    {
        "boundary"
    } else if ["/storage/", "/database/", "/migrations/", "/cache/"]
        .iter()
        .any(|s| path.contains(s))
    {
        "persistence"
    } else if ["/runtime/", "/cli/", "/config/"]
        .iter()
        .any(|s| path.contains(s))
    {
        "infrastructure"
    } else {
        "utility"
    };
    let explanation = super::codebase_judgments::FILE_ROLES
        .iter()
        .find(|(r, _)| *r == role)
        .map_or("Unclassified component", |(_, e)| *e);
    (role, 1, explanation)
}

pub fn build(mut evidence: RepositoryEvidence, store: &IndexStore) -> Tour {
    let mut redactions = crate::domain::redact::Redactions::default();
    for file in &mut evidence.files {
        file.content = crate::domain::redact::redact(&file.content, &mut redactions);
    }
    let mut unknowns = evidence.unknowns;
    let (tests, files): (Vec<_>, Vec<_>) = evidence
        .files
        .into_iter()
        .filter(|f| is_source_path(&f.path))
        .partition(|f| is_test_path(&f.path));
    let index = RepoIndex::build(&files, &tests, store);
    if index.stats.fallbacks > 0 {
        unknowns.push(format!("{} source/test file(s) lack index metadata (size limit or unsupported content); their public declarations are unknown", index.stats.fallbacks));
    }
    let mut stops: Vec<_> = files
        .iter()
        .map(|file| {
            let (role, line, explanation) = role(file);
            TourStop {
                role: role.into(),
                explanation: format!(
                    "{explanation} Role inferred from source/path; verify runtime behavior."
                ),
                reference: reference(file, line),
                public_surface: index.signatures(&file.path).chars().take(1000).collect(),
            }
        })
        .collect();
    stops.sort_by_key(|s| (s.role != "entrypoint", s.reference.path.clone()));
    if !stops.iter().any(|s| s.role == "entrypoint") {
        unknowns.push(
            "No evidenced conventional entry point; custom framework/bootstrap behavior is unknown"
                .into(),
        );
    }
    let files_by_path: std::collections::HashMap<_, _> = files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect();
    let relationships: Vec<_> = index
        .evidenced_edges()
        .iter()
        .filter_map(|(source, target, line)| {
            let source = files_by_path.get(source.as_str())?;
            let target = files_by_path.get(target.as_str())?;
            Some(Relationship {
                source: reference(source, *line),
                target: reference(target, 1),
                kind: "resolved import".into(),
            })
        })
        .collect();
    let partial = !unknowns.is_empty() || relationships.is_empty();
    if relationships.is_empty() {
        unknowns.push("No corroborated internal import edges; external imports, dynamic loading and unsupported syntax remain unknown".into());
    } else {
        unknowns.push("Map includes only corroborated static internal imports; dynamic calls and external dependency behavior remain unknown".into());
    }
    let mut groups: std::collections::BTreeMap<String, Vec<&TourStop>> =
        std::collections::BTreeMap::new();
    for stop in &stops {
        let parent = std::path::Path::new(&stop.reference.path)
            .parent()
            .and_then(|p| p.to_str())
            .unwrap_or("");
        groups
            .entry(if parent.is_empty() {
                ".".into()
            } else {
                parent.into()
            })
            .or_default()
            .push(stop);
    }
    let components = groups
        .into_iter()
        .map(|(path, stops)| Component {
            path,
            files: stops.len(),
            roles: stops
                .iter()
                .map(|s| s.role.clone())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect(),
            reference: stops[0].reference.clone(),
        })
        .collect();
    Tour {
        head: evidence.head,
        basis:
            "Working-tree RepoIndex static import resolution and heuristic file-role classification"
                .into(),
        partial,
        stops,
        components,
        relationships,
        unknowns,
    }
}

pub fn markdown(tour: &Tour) -> String {
    let clean = |s: &str| s.replace(['\n', '\r', '`', '[', ']', '<', '>'], " ");
    let mut out = format!(
        "# Repository tour\n\nSource: {}\n\n{}\n\n",
        clean(&tour.head),
        clean(&tour.basis)
    );
    out.push_str("## Major component directories\n\nPath groups and inferred roles (not runtime boundaries):\n\n");
    for c in &tour.components {
        out.push_str(&format!(
            "- {}: {} files; {}. Source {}:{}\n",
            clean(&c.path),
            c.files,
            clean(&c.roles.join(", ")),
            clean(&c.reference.path),
            c.reference.line
        ));
    }
    out.push_str("\n## Tour stops\n\n");
    for stop in &tour.stops {
        out.push_str(&format!(
            "- **{}** — {}:{}: {}\n  {}\n",
            clean(&stop.role),
            clean(&stop.reference.path),
            stop.reference.line,
            clean(&stop.explanation),
            clean(&stop.reference.text)
        ));
        if !stop.public_surface.is_empty() {
            out.push_str(&format!(
                "  Public declarations: {}\n",
                clean(&stop.public_surface)
            ));
        }
    }
    out.push_str("\n## Architecture map\n\nDirected static import edges (runtime call relationships are unknown):\n\n");
    for edge in &tour.relationships {
        out.push_str(&format!(
            "- {}:{} → {}:{} ({})\n",
            clean(&edge.source.path),
            edge.source.line,
            clean(&edge.target.path),
            edge.target.line,
            clean(&edge.kind)
        ));
    }
    out.push_str("\n## Evidence limits\n\n");
    for unknown in &tour.unknowns {
        out.push_str(&format!("- {}\n", clean(unknown)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn file(path: &str, text: &str) -> SourceFile {
        SourceFile {
            path: path.into(),
            content: text.into(),
        }
    }
    #[test]
    fn representative_layouts_have_cited_entrypoints_and_edges() {
        for files in [
            vec![
                file("src/main.rs", "mod domain;\nfn main() {}"),
                file("src/domain.rs", "pub fn run() {}"),
            ],
            vec![
                file("src/index.ts", "import {run} from './domain';\nrun();"),
                file("src/domain.ts", "export function run() {}"),
            ],
            vec![
                file("main.py", "import domain\nif __name__ == '__main__': pass"),
                file("domain.py", "def run(): pass"),
            ],
        ] {
            let dir = tempfile::tempdir().unwrap();
            let tour = build(
                RepositoryEvidence {
                    files,
                    ..Default::default()
                },
                &IndexStore::open(dir.path().into()),
            );
            assert!(tour.stops.iter().any(|s| s.role == "entrypoint"));
            if tour.stops.iter().any(|s| s.reference.path.ends_with(".py")) {
                assert!(tour.relationships.is_empty());
                assert!(tour.partial);
            } else {
                assert!(!tour.relationships.is_empty(), "{}", markdown(&tour));
                assert!(
                    !tour.partial,
                    "fixed disclosure does not imply an incomplete inventory"
                );
                assert!(
                    tour.unknowns
                        .iter()
                        .any(|unknown| unknown.contains("dynamic calls"))
                );
            }
            assert!(tour.relationships.iter().all(|e| e.source.line == 1));
        }
    }
    #[test]
    fn incomplete_layout_does_not_invent_edges_or_entrypoints() {
        let dir = tempfile::tempdir().unwrap();
        let tour = build(
            RepositoryEvidence {
                files: vec![file("a.rs", "use external::Thing;")],
                unknowns: vec!["byte limit".into()],
                ..Default::default()
            },
            &IndexStore::open(dir.path().into()),
        );
        assert!(tour.relationships.is_empty());
        assert!(tour.partial);
        assert!(tour.unknowns.iter().any(|s| s.contains("entry point")));
    }

    #[test]
    fn inventory_gaps_remain_partial_even_with_entrypoint_and_edges() {
        let dir = tempfile::tempdir().unwrap();
        let tour = build(
            RepositoryEvidence {
                files: vec![
                    file("src/main.rs", "mod worker;\nfn main() {}"),
                    file("src/worker.rs", "pub fn run() {}"),
                ],
                unknowns: vec!["file limit omitted source context".into()],
                ..Default::default()
            },
            &IndexStore::open(dir.path().into()),
        );
        assert!(!tour.relationships.is_empty());
        assert!(tour.partial);
        assert!(
            tour.unknowns
                .iter()
                .any(|unknown| unknown.contains("file limit"))
        );
    }

    #[test]
    fn missing_entrypoint_or_edge_evidence_still_marks_partial() {
        for files in [
            vec![
                file("src/module.rs", "use crate::worker::run;"),
                file("src/worker.rs", "pub fn run() {}"),
            ],
            vec![file("src/main.rs", "fn main() {}")],
        ] {
            let dir = tempfile::tempdir().unwrap();
            let tour = build(
                RepositoryEvidence {
                    files,
                    ..Default::default()
                },
                &IndexStore::open(dir.path().into()),
            );
            assert!(tour.partial);
        }
    }

    #[test]
    fn oversized_index_metadata_keeps_public_surface_unknown_and_tour_partial() {
        let dir = tempfile::tempdir().unwrap();
        let large = format!("pub fn run() {{}}\n//{}", "x".repeat(2_000_000));
        let tour = build(
            RepositoryEvidence {
                files: vec![
                    file("src/main.rs", "mod worker;\nfn main() {}"),
                    file("src/worker.rs", &large),
                ],
                ..Default::default()
            },
            &IndexStore::open(dir.path().into()),
        );
        assert!(
            !tour.relationships.is_empty(),
            "imports still have exact source evidence"
        );
        assert!(tour.partial);
        assert!(tour.unknowns.iter().any(|unknown| {
            unknown.contains("1 source/test file(s) lack index metadata")
                && unknown.contains("public declarations are unknown")
        }));
        assert!(
            tour.stops
                .iter()
                .find(|stop| stop.reference.path == "src/worker.rs")
                .unwrap()
                .public_surface
                .is_empty()
        );
    }
}
