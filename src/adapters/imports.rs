//! Import-graph neighbor resolution for codebase mode. Extracts candidate
//! import targets from source files and builds an undirected adjacency graph
//! so that screening one file can carry its 1-hop callers/callees as context.
//!
//! Resolution is deliberately heuristic: candidates are matched against the
//! scanned file set by path suffix/stem after normalizing extensions, and a
//! module `foo` is treated as interchangeable with `foo/mod` and `foo/index`.
//! The result is a best-effort neighborhood, not an exact module graph.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;

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
        while let Some(stripped) = rest.strip_prefix("super::").or_else(|| rest.strip_prefix("self::")) {
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
    if joined.is_empty() { None } else { Some(joined) }
}

static MOD_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^[ \t]*(?:pub(?:\([^)]*\))?[ \t]+)?mod[ \t]+([A-Za-z_][A-Za-z0-9_]*)[ \t]*;")
        .expect("valid regex")
});
static USE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^[ \t]*use[ \t]+([^;\n]*);").expect("valid regex")
});
static JS_IMPORT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        concat!(
            r#"(?m)"#,
            r#"import\s+[^'"]*?\s+from\s+['"]([^'"]+)['"]"#,
            r#"|import\s+['"]([^'"]+)['"]"#,
            r#"|export\s+[^'"]*?\s+from\s+['"]([^'"]+)['"]"#,
            r#"|require\s*\(\s*['"]([^'"]+)['"]"#,
        ),
    )
    .expect("valid regex")
});

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

fn extract_js_imports(content: &str) -> Vec<String> {
    JS_IMPORT_REGEX
        .captures_iter(content)
        .filter_map(|c| c.iter().skip(1).find_map(|g| g).map(|m| m.as_str().to_string()))
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
    if path.ends_with(".rs") {
        extract_rust_imports(content)
    } else {
        extract_js_imports(content)
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
    if matches!(segments.last().map(String::as_str), Some("mod") | Some("index")) {
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
    if matches!(segments.last().map(String::as_str), Some("mod") | Some("index")) {
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

/// Undirected import adjacency: an edge between `a` and `b` exists when `b`
/// resolves to a scanned file imported by `a` (so neighbors include both
/// files `a` imports and files that import `a`).
pub struct ImportGraph {
    adjacency: HashMap<String, Vec<String>>,
}

impl ImportGraph {
    pub fn build(files: &[SourceFile]) -> ImportGraph {
        let mut adjacency: HashMap<String, Vec<String>> = files
            .iter()
            .map(|f| (f.path.clone(), Vec::new()))
            .collect();

        let resolver = Resolver::new(files);
        for f in files {
            let importer = normalize_path(&f.path);
            let is_rust = f.path.ends_with(".rs");
            for target in extract_imports(&f.path, &f.content) {
                if let Some(resolved) = resolver.resolve(&target, &importer, is_rust) {
                    adjacency.entry(f.path.clone()).or_default().push(resolved.clone());
                    adjacency.entry(resolved).or_default().push(f.path.clone());
                }
            }
        }

        ImportGraph { adjacency }
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
        SourceFile { path: path.to_string(), content: content.to_string() }
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