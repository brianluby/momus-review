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
    Regex::new(r"(?m)^[ \t]*mod[ \t]+([A-Za-z_][A-Za-z0-9_]*)[ \t]*;").expect("valid regex")
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

/// Normalizes a candidate target: drops `./`, `.`, and `..` segments (the
/// latter are handled by suffix matching, a documented heuristic).
fn candidate_segments(candidate: &str) -> Vec<String> {
    candidate
        .trim_start_matches("./")
        .split('/')
        .filter(|s| !s.is_empty() && *s != "." && *s != "..")
        .map(str::to_string)
        .collect()
}

/// Resolves a candidate to a scanned file path by normalized suffix match,
/// preferring the longest (most directory-specific) match.
fn resolve(candidate: &str, files: &[SourceFile]) -> Option<String> {
    let cand = candidate_segments(candidate);
    if cand.is_empty() {
        return None;
    }
    files
        .iter()
        .filter_map(|f| {
            let norm = normalize_path(&f.path);
            let suffix = norm.len() >= cand.len() && norm[norm.len() - cand.len()..] == cand[..];
            if suffix {
                Some((norm.len(), f.path.as_str()))
            } else {
                None
            }
        })
        .max_by_key(|(len, _)| *len)
        .map(|(_, path)| path.to_string())
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

        for f in files {
            for target in extract_imports(&f.path, &f.content) {
                if let Some(resolved) = resolve(&target, files) {
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