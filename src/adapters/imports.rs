//! Import-graph neighbor resolution for codebase mode. Extracts candidate
//! import targets from source files and builds an undirected adjacency graph
//! so that screening one file can carry its 1-hop callers/callees as context.
//!
//! Resolution is deliberately heuristic: candidates are matched against the
//! scanned file set by path suffix/stem after normalizing extensions, and a
//! module `foo` is treated as interchangeable with `foo/mod` and `foo/index`.
//! The result is a best-effort neighborhood, not an exact module graph.
//!
//! Extractors exist for Rust and for JS/TS; every other discovered language
//! contributes no edges (a neighborhood, when a language has one, is a
//! follow-up rather than a guess made with another language's patterns).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use crate::domain::language::Language;
use crate::domain::report::SourceFile;

/// A `use …;` path with a braced item list and/or `as` rename is captured by
/// statement, then trimmed here. Returns the module path as `/`-joined local
/// segments (`crate::a::b` → `a/b`, `super::x` → `x`), or `None` for an
/// external-crate path (which cannot resolve to a scanned file).
fn rust_use_target(path: &str) -> Option<String> {
    let path = path.split('{').next().unwrap_or(path);
    let path = path.split(" as ").next().unwrap_or(path);
    let path = path.trim().trim_start_matches("::");

    let local = if let Some(rest) = path.strip_prefix("crate::") {
        rest
    } else {
        let mut rest = path;
        while let Some(stripped) = rest
            .strip_prefix("super::")
            .or_else(|| rest.strip_prefix("self::"))
        {
            rest = stripped;
        }
        if rest == path {
            // No local anchor: a bare external-crate path like `serde::Serialize`.
            return None;
        }
        rest
    };

    let joined = local
        .split("::")
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("/");
    if joined.is_empty() {
        None
    } else {
        Some(joined)
    }
}

static MOD_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^[ \t]*(?:pub(?:\([^)]*\))?[ \t]+)?mod[ \t]+([A-Za-z_][A-Za-z0-9_]*)[ \t]*;")
        .expect("valid regex")
});
static USE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^[ \t]*use[ \t]+([^;\n]*);").expect("valid regex"));
static JS_IMPORT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r#"(?m)"#,
        r#"import\s+[^'"]*?\s+from\s+['"]([^'"]+)['"]"#,
        r#"|import\s+['"]([^'"]+)['"]"#,
        r#"|export\s+[^'"]*?\s+from\s+['"]([^'"]+)['"]"#,
        r#"|require\s*\(\s*['"]([^'"]+)['"]"#,
    ))
    .expect("valid regex")
});

/// Extract Rust use/mod candidates without resolving them to repository paths.
fn extract_rust_imports(content: &str) -> Vec<String> {
    let mut out: Vec<String> = MOD_REGEX
        .captures_iter(content)
        .map(|c| c[1].to_string())
        .collect();
    out.extend(
        USE_REGEX
            .captures_iter(content)
            .filter_map(|c| rust_use_target(&c[1])),
    );
    out
}

/// Extract JavaScript/TypeScript import and require candidates.
fn extract_js_imports(content: &str) -> Vec<String> {
    JS_IMPORT_REGEX
        .captures_iter(content)
        .filter_map(|c| {
            c.iter()
                .skip(1)
                .find_map(|g| g)
                .map(|m| m.as_str().to_string())
        })
        .filter(|s| s.starts_with("./") || s.starts_with("../"))
        .collect()
}

/// Extracts raw candidate import targets from `content`.
///
/// - Rust: identifiers from column-0 or indented `use …;` and `mod <name>;`
///   statements. `use crate::a::b;` → `a/b`; `use super::x;`/`use self::x;`
///   → `x` (a placeholder that resolves by stem later); `mod foo;` → `foo`.
///   Inline `mod foo {` blocks and external-crate `use` paths are skipped.
/// - TS/JS: the specifier of `import … from '…'`, `import '…'`,
///   `export … from '…'`, and `require('…')`, keeping only relative
///   (`./`, `../`) specifiers; bare/package/URL specifiers are dropped.
pub fn extract_imports(path: &str, content: &str) -> Vec<String> {
    match Language::from_path(path) {
        Some(Language::Rust) => extract_rust_imports(content),
        Some(Language::JavaScript) | Some(Language::TypeScript) => extract_js_imports(content),
        // Every other discovered language has no extractor yet: return no
        // edges rather than parsing the file with another language's patterns.
        _ => Vec::new(),
    }
}

/// Splits a file path into extension-less directory + stem segments, dropping
/// a `mod`/`index` leaf (so `foo/mod.rs` and `foo/index.ts` both normalize to
/// the `foo` module path).
fn normalize_path(path: &str) -> Vec<String> {
    let mut segments: Vec<String> = path
        .trim_start_matches("./")
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .map(str::to_string)
        .collect();
    if let Some(last) = segments.last_mut()
        && let Some(dot) = last.rfind('.')
    {
        last.truncate(dot);
    }
    if matches!(
        segments.last().map(String::as_str),
        Some("mod") | Some("index")
    ) {
        segments.pop();
    }
    segments
}

/// Normalizes a candidate target the same way as `normalize_path`: strips a
/// trailing extension and a `mod`/`index` leaf, so a relative `./widget.js`
/// matches `src/widget.ts` and `./widget/index` matches the `widget` module.
/// Also drops `.`/`..` segments (relative-ness is handled by importer-aware
/// resolution, a documented heuristic).
fn candidate_segments(candidate: &str) -> Vec<String> {
    let mut segments: Vec<String> = candidate
        .trim_start_matches("./")
        .split('/')
        .filter(|s| !s.is_empty() && *s != "." && *s != "..")
        .map(str::to_string)
        .collect();
    if let Some(last) = segments.last_mut()
        && let Some(dot) = last.rfind('.')
    {
        last.truncate(dot);
    }
    if matches!(
        segments.last().map(String::as_str),
        Some("mod") | Some("index")
    ) {
        segments.pop();
    }
    segments
}

/// Precomputed resolution index over the scanned files: per-file normalized
/// segments plus a last-segment map so each import resolves without a full
/// scan (imports are few per file; files are many).
struct Resolver {
    norm: Vec<Vec<String>>,
    paths: Vec<String>,
    by_last: HashMap<String, Vec<usize>>,
}

impl Resolver {
    /// Index normalized paths by their final segment for candidate resolution.
    fn new(files: &[SourceFile]) -> Resolver {
        let mut norm = Vec::with_capacity(files.len());
        let mut by_last: HashMap<String, Vec<usize>> = HashMap::new();
        for f in files {
            let segments = normalize_path(&f.path);
            if let Some(last) = segments.last() {
                by_last.entry(last.clone()).or_default().push(norm.len());
            }
            norm.push(segments);
        }
        Resolver {
            paths: files.iter().map(|f| f.path.clone()).collect(),
            norm,
            by_last,
        }
    }

    /// Resolves a candidate to a scanned path. `importer` is the importing
    /// file's normalized segments; matches under the importer's directory are
    /// preferred (so a relative `./helper` resolves to its sibling, not an
    /// unrelated same-stem file), then deeper paths win, then lexicographic
    /// path (deterministic). `is_rust` enables the item-name fallback for
    /// `use crate::a::b::TypeName` (drop the trailing item segment).
    fn resolve(&self, candidate: &str, importer: &[String], is_rust: bool) -> Option<String> {
        let cand = candidate_segments(candidate);
        if cand.is_empty() {
            return None;
        }
        let mut matches = self.suffix_matches(&cand);
        if matches.is_empty() && is_rust && cand.len() > 1 {
            matches = self.suffix_matches(&cand[..cand.len() - 1]);
        }
        let dir_len = importer.len().saturating_sub(1);
        matches
            .into_iter()
            .min_by(|&a, &b| {
                self.goodness(b, importer, dir_len)
                    .cmp(&self.goodness(a, importer, dir_len))
                    .then_with(|| self.paths[a].cmp(&self.paths[b]))
            })
            .map(|i| self.paths[i].clone())
    }

    /// Rank: importer-relative first, then deeper (more specific), tiebroken
    /// by path string at the call site.
    fn goodness(&self, i: usize, importer: &[String], dir_len: usize) -> (bool, usize) {
        let relative = dir_len > 0
            && self.norm[i].len() > dir_len
            && self.norm[i][..dir_len] == importer[..dir_len];
        (relative, self.norm[i].len())
    }

    /// Compare normalized trailing path segments when resolving an import.
    fn suffix_matches(&self, cand: &[String]) -> Vec<usize> {
        let Some(idxs) = self.by_last.get(&cand[cand.len() - 1]) else {
            return Vec::new();
        };
        idxs.iter()
            .copied()
            .filter(|&i| {
                let norm = &self.norm[i];
                norm.len() >= cand.len() && norm[norm.len() - cand.len()..] == cand[..]
            })
            .collect()
    }
}

fn has_js_extension(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" | "mts" | "cts")
    )
}

/// Index exact paths once for architecture-map imports. Lookup cost depends
/// on matching paths, not the full repository inventory for every import.
/// Vectors retain duplicate paths and extension collisions as ambiguous.
struct StrictResolver<'a> {
    rust_paths: HashMap<String, Vec<&'a str>>,
    js_stems: HashMap<PathBuf, Vec<&'a str>>,
}

impl<'a> StrictResolver<'a> {
    fn new(files: &'a [SourceFile]) -> Self {
        let mut rust_paths: HashMap<String, Vec<&str>> = HashMap::new();
        let mut js_stems: HashMap<PathBuf, Vec<&str>> = HashMap::new();
        for file in files {
            let path = Path::new(&file.path);
            if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
                rust_paths
                    .entry(file.path.clone())
                    .or_default()
                    .push(file.path.as_str());
            } else if has_js_extension(path) {
                js_stems
                    .entry(path.with_extension(""))
                    .or_default()
                    .push(file.path.as_str());
            }
        }
        Self {
            rust_paths,
            js_stems,
        }
    }

    fn matching_paths(&self, stem: &Path, is_rust: bool) -> Vec<&'a str> {
        let mut matches = Vec::new();
        if is_rust {
            for path in [
                format!("{}.rs", stem.display()),
                format!("{}/mod.rs", stem.display()),
            ] {
                if let Some(paths) = self.rust_paths.get(&path) {
                    matches.extend(paths.iter().copied());
                }
            }
        } else {
            if let Some(paths) = self.js_stems.get(stem) {
                matches.extend(paths.iter().copied());
            }
            if let Some(paths) = self.js_stems.get(&stem.join("index")) {
                matches.extend(paths.iter().copied());
            }
        }
        matches
    }
}

/// Architecture maps use exact, unique local module paths, independently of
/// the suffix heuristic used for review context. Opaque syntax abstains.
fn strict_edges(file: &SourceFile, resolver: &StrictResolver<'_>) -> Vec<(String, String, usize)> {
    use std::path::Component;
    let language = Language::from_path(&file.path);
    if !matches!(
        language,
        Some(Language::Rust | Language::JavaScript | Language::TypeScript)
    ) || file.content.contains("/*")
        || file.content.contains('`')
        || file.content.contains("r#\"")
        || file.content.contains("#[path")
        || file.content.contains("#[cfg")
    {
        return Vec::new();
    }
    let path = Path::new(&file.path);
    let parent = path.parent().unwrap_or(Path::new(""));
    let module_parent = if matches!(
        path.file_name().and_then(|s| s.to_str()),
        Some("main.rs" | "lib.rs" | "mod.rs")
    ) {
        parent.to_path_buf()
    } else {
        parent.join(path.file_stem().unwrap_or_default())
    };
    let mut depth = 0isize;
    let mut out = Vec::new();
    for (line, text) in file.content.lines().enumerate() {
        let text = text.trim();
        if text.starts_with("//") {
            continue;
        }
        let active = depth == 0
            && (text.starts_with("use ")
                || text.starts_with("mod ")
                || text.starts_with("pub mod ")
                || text.starts_with("import ")
                || text.starts_with("export "));
        if active {
            for candidate in extract_imports(&file.path, text) {
                let mut stem = if language == Some(Language::Rust) {
                    if text.starts_with("use crate::") {
                        let segments: Vec<_> = parent.components().collect();
                        let Some(i) = segments.iter().rposition(|s| s.as_os_str() == "src") else {
                            continue;
                        };
                        let root: PathBuf = segments[..=i].iter().map(|s| s.as_os_str()).collect();
                        root.join(&candidate)
                    } else if text.starts_with("use super::") {
                        // Repeated parent anchors and inline module scopes are unknown.
                        if text.contains("super::super::") {
                            continue;
                        }
                        module_parent
                            .parent()
                            .unwrap_or(Path::new(""))
                            .join(&candidate)
                    } else {
                        module_parent.join(&candidate)
                    }
                } else {
                    parent.join(&candidate)
                };
                let mut parts = Vec::new();
                let mut valid = true;
                for part in stem.components() {
                    match part {
                        Component::Normal(s) => parts.push(s.to_string_lossy().into_owned()),
                        Component::CurDir => {}
                        Component::ParentDir => {
                            if parts.pop().is_none() {
                                valid = false;
                            }
                        }
                        _ => valid = false,
                    }
                }
                if !valid {
                    continue;
                }
                stem = PathBuf::from(parts.join("/"));
                if language != Some(Language::Rust) && has_js_extension(&stem) {
                    stem.set_extension("");
                }
                let mut matches = resolver.matching_paths(&stem, language == Some(Language::Rust));
                if matches.is_empty()
                    && language == Some(Language::Rust)
                    && text.starts_with("use ")
                    && let Some(parent) = stem.parent()
                {
                    matches = resolver.matching_paths(parent, true);
                }
                if matches.len() == 1 && matches[0] != file.path {
                    out.push((file.path.clone(), matches[0].to_string(), line + 1));
                }
            }
        }
        // Conservative top-level declarations only. Dynamic/nested imports
        // and multiline strings are outside the evidence map contract.
        depth += text.matches('{').count() as isize - text.matches('}').count() as isize;
        if depth < 0 {
            // Brace-containing literals or unsupported syntax can invalidate
            // the lightweight scope counter. Never retain a prefix map when
            // later lines disprove its scope assumptions.
            return Vec::new();
        }
    }
    if depth == 0 { out } else { Vec::new() }
}

/// Undirected import adjacency: an edge between `a` and `b` exists when `b`
/// resolves to a scanned file imported by `a` (so neighbors include both
/// files `a` imports and files that import `a`).
pub struct ImportGraph {
    adjacency: HashMap<String, Vec<String>>,
    edges: Vec<(String, String, usize)>,
}

impl ImportGraph {
    /// Build forward and reverse edges by resolving candidates against this inventory.
    pub fn build(files: &[SourceFile]) -> ImportGraph {
        Self::from_candidates(files, |f| extract_imports(&f.path, &f.content))
    }

    /// Builds edges from blob-cached import candidates without re-parsing source.
    pub fn from_candidates(
        files: &[SourceFile],
        candidates: impl Fn(&SourceFile) -> Vec<String>,
    ) -> ImportGraph {
        let mut adjacency: HashMap<String, Vec<String>> =
            files.iter().map(|f| (f.path.clone(), Vec::new())).collect();

        let resolver = Resolver::new(files);
        let strict_resolver = StrictResolver::new(files);
        let mut edges = Vec::new();
        for f in files {
            edges.extend(strict_edges(f, &strict_resolver));
            let importer = normalize_path(&f.path);
            let is_rust = Language::from_path(&f.path) == Some(Language::Rust);
            for target in candidates(f) {
                if let Some(resolved) = resolver.resolve(&target, &importer, is_rust) {
                    adjacency
                        .entry(f.path.clone())
                        .or_default()
                        .push(resolved.clone());
                    adjacency.entry(resolved).or_default().push(f.path.clone());
                }
            }
        }

        edges.sort();
        edges.dedup();
        ImportGraph { adjacency, edges }
    }

    /// Directed imports with a corroborated declaration line. Multiline
    /// imports lacking exact line evidence are omitted from architecture maps.
    pub fn evidenced_edges(&self) -> &[(String, String, usize)] {
        &self.edges
    }

    /// The 1-hop adjacency of `file` (deduped, sorted, excluding self).
    pub fn neighbors(&self, file: &str) -> Vec<&str> {
        let mut seen: HashSet<&str> = HashSet::new();
        let mut out: Vec<&str> = Vec::new();
        if let Some(list) = self.adjacency.get(file) {
            for n in list {
                if n != file && seen.insert(n) {
                    out.push(n.as_str());
                }
            }
        }
        out.sort_unstable();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, content: &str) -> SourceFile {
        SourceFile {
            path: path.to_string(),
            content: content.to_string(),
        }
    }

    #[test]
    fn tour_edges_require_exact_unambiguous_active_imports() {
        let files = vec![
            file("src/nested/a.ts", "import {x} from '../shared';"),
            file("src/nested/shared.ts", "export const x=1;"),
            file("src/shared.ts", "export const x=2;"),
        ];
        assert_eq!(
            ImportGraph::build(&files).evidenced_edges(),
            &[("src/nested/a.ts".into(), "src/shared.ts".into(), 1)]
        );
        let files = vec![
            file("src/index.ts", "import {x} from './foo';"),
            file("other/foo.ts", "export const x=1;"),
        ];
        assert!(ImportGraph::build(&files).evidenced_edges().is_empty());
        let files = vec![
            file(
                "src/a.ts",
                "// import {x} from './foo';\nconst s = \"import {x} from './foo'\";",
            ),
            file("src/foo.ts", "export const x=1;"),
        ];
        assert!(ImportGraph::build(&files).evidenced_edges().is_empty());
        let files = vec![
            file("src/a.ts", "import {x} from './foo';"),
            file("src/foo.ts", "export const x=1;"),
            file("src/foo.js", "export const x=2;"),
        ];
        assert!(ImportGraph::build(&files).evidenced_edges().is_empty());
    }

    #[test]
    fn strict_js_resolution_preserves_dotted_module_names() {
        for import in ["./user.service", "./user.service.js", "./user.service.ts"] {
            let files = vec![
                file(
                    "src/index.ts",
                    &format!("import {{ UserService }} from '{import}';"),
                ),
                file("src/user.service.ts", "export class UserService {}"),
                file("src/user.ts", "export class OtherUser {}"),
                file("src/user/index.ts", "export class OtherUser {}"),
            ];
            assert_eq!(
                ImportGraph::build(&files).evidenced_edges(),
                &[("src/index.ts".into(), "src/user.service.ts".into(), 1)],
                "specifier {import}"
            );
        }
        let files = vec![
            file("src/index.ts", "import './config.dev';"),
            file("src/config.dev.ts", "export const dev = true;"),
            file("src/config.ts", "export const dev = false;"),
        ];
        assert_eq!(
            ImportGraph::build(&files).evidenced_edges(),
            &[("src/index.ts".into(), "src/config.dev.ts".into(), 1)]
        );
        let files = vec![
            file("src/index.ts", "import './worker.wasm';"),
            file("src/worker.ts", "export const worker = true;"),
        ];
        assert!(ImportGraph::build(&files).evidenced_edges().is_empty());
    }

    #[test]
    fn strict_lookup_preserves_rust_and_js_ambiguity_abstention() {
        for files in [
            vec![
                file("src/main.rs", "mod worker;"),
                file("src/worker.rs", "pub fn run() {}"),
                file("src/worker/mod.rs", "pub fn run() {}"),
            ],
            vec![
                file("src/index.ts", "import './user.service';"),
                file("src/user.service.ts", "export class Service {}"),
                file("src/user.service/index.ts", "export class Service {}"),
            ],
            vec![
                file("src/index.ts", "import './user.service.js';"),
                file("src/user.service.ts", "export class Service {}"),
                file("src/user.service.js", "export class Service {}"),
            ],
        ] {
            assert!(ImportGraph::build(&files).evidenced_edges().is_empty());
        }
        let files = vec![
            file("src/main.rs", "use crate::worker::Service;"),
            file("src/worker.rs", "pub struct Service;"),
        ];
        assert_eq!(
            ImportGraph::build(&files).evidenced_edges(),
            &[("src/main.rs".into(), "src/worker.rs".into(), 1)]
        );
    }

    #[test]
    fn strict_lookup_abstains_on_opaque_or_inactive_imports() {
        for declaration in [
            "/* mod worker; */",
            "#[cfg(feature = \"worker\")]\nmod worker;",
            "#[path = \"worker.rs\"]\nmod worker;",
            "const EXAMPLE: &str = r#\"\nmod worker;\n\"#;",
            "fn nested() {\nmod worker;\n}",
        ] {
            let files = vec![
                file("src/main.rs", declaration),
                file("src/worker.rs", "pub fn run() {}"),
            ];
            assert!(
                ImportGraph::build(&files).evidenced_edges().is_empty(),
                "declaration {declaration}"
            );
        }
        let files = vec![
            file("src/index.ts", "const example = `\nimport './worker';\n`;"),
            file("src/worker.ts", "export function run() {}"),
        ];
        assert!(ImportGraph::build(&files).evidenced_edges().is_empty());
    }

    #[test]
    fn strict_edges_drop_prefix_evidence_when_scope_counter_is_unbalanced() {
        for content in [
            "mod worker;\nfn main() {\nlet brace = '{';\n}\nuse crate::worker::run;",
            "mod worker;\nfn main() {\nlet brace = '}';\n}\nuse crate::worker::run;",
            "mod worker;\nconst TEXT: &str = \"{\";\nuse crate::worker::run;",
            "mod worker;\nconst TEXT: &str = \"}\";\nuse crate::worker::run;",
            "mod worker;\nfn main() {",
        ] {
            let files = vec![
                file("src/main.rs", content),
                file("src/worker.rs", "pub fn run() {}"),
            ];
            assert!(
                ImportGraph::build(&files).evidenced_edges().is_empty(),
                "content {content}"
            );
        }
    }

    #[test]
    fn full_line_comment_braces_do_not_change_strict_scope() {
        let files = vec![
            file(
                "src/main.rs",
                "// unmatched braces { {{{\nmod worker;\n// } }\nfn main() {}",
            ),
            file("src/worker.rs", "pub fn run() {}"),
        ];
        assert_eq!(
            ImportGraph::build(&files).evidenced_edges(),
            &[("src/main.rs".into(), "src/worker.rs".into(), 2)]
        );
    }

    #[test]
    fn strict_edges_resolve_many_distinct_modules_from_one_index() {
        let files: Vec<_> = (0..4096)
            .map(|i| {
                file(
                    &format!("src/nodes/node{i}.rs"),
                    &format!("use super::node{};", (i + 1) % 4096),
                )
            })
            .collect();
        // Isolate architecture resolution from the separately cached heuristic
        // neighbor graph while exercising a repository-sized inventory.
        let graph = ImportGraph::from_candidates(&files, |_| Vec::new());
        assert_eq!(graph.evidenced_edges().len(), files.len());
        assert!(
            graph
                .evidenced_edges()
                .iter()
                .all(|(source, target, line)| source != target && *line == 1)
        );
        assert!(graph.evidenced_edges().contains(&(
            "src/nodes/node4095.rs".into(),
            "src/nodes/node0.rs".into(),
            1
        )));
    }
    #[test]
    fn extracts_rust_imports() {
        let content = "//! preamble\nuse crate::a::b;\n  use super::x;\nuse self::y;\nmod foo;\nmod bar { fn inline() {} }\nuse serde::Serialize;\n";
        let got = extract_imports("src/lib.rs", content);
        assert_eq!(got, vec!["foo", "a/b", "x", "y"]);
    }

    #[test]
    fn extracts_js_relative_imports_and_drops_bare() {
        let content = concat!(
            "import { a } from './a';\n",
            "import '../b/helper';\n",
            "export * from '../c';\n",
            "const d = require('./d');\n",
            "import React from 'react';\n",
            "require('https://x/y');\n",
        );
        let got = extract_imports("src/index.ts", content);
        assert_eq!(got, vec!["./a", "../b/helper", "../c", "./d"]);
    }

    #[test]
    fn builds_undirected_graph_and_neighbors() {
        let files = vec![
            file("src/a.rs", "use crate::b;\n"),
            file("src/b.rs", "use crate::a;\n"),
            file("src/isolated.rs", "fn f() {}\n"),
        ];
        let graph = ImportGraph::build(&files);

        assert_eq!(graph.neighbors("src/a.rs"), vec!["src/b.rs"]);
        assert_eq!(graph.neighbors("src/b.rs"), vec!["src/a.rs"]);
        assert!(graph.neighbors("src/isolated.rs").is_empty());
        assert!(graph.neighbors("src/missing.rs").is_empty());
    }
}
