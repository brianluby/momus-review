//! Function-aware source-region splitting for codebase mode, without a
//! parser: declaration-start lines at column 0 delimit regions, oversized
//! declarations subdivide uniformly, and files without declarations fall
//! back to the old fixed-size slicing.

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

/// A source-region window: `{ id, startLine, content }`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Region {
    pub id: String,
    pub start_line: usize,
    pub content: String,
}

static RUST_DECL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"^(pub(\((crate|super|self|in\s+[\w:]+)\))?\s+)?(async\s+)?(unsafe\s+)?(extern\s+"[^"]*"\s+)?((?:fn|impl|trait|struct|enum|union|mod)\b|macro_rules!)"#,
    )
    .expect("valid Rust declaration regex")
});

static TS_DECL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(export\s+(default\s+)?)?(abstract\s+)?(async\s+)?(function|class|interface|enum|type|const|let|var)\b",
    )
    .expect("valid TS/JS declaration regex")
});

/// Splits `content` into declaration-aligned regions. `.rs` paths use Rust
/// patterns; everything else uses TS/JS patterns. A line starts a region
/// only when its first non-whitespace character begins a declaration, i.e.
/// the declaration keyword sits at column 0 (indented items such as impl
/// methods or class methods never split).
pub fn function_regions(content: &str, path: &str, max_region_lines: usize) -> Vec<Region> {
    let lines: Vec<&str> = content.split('\n').collect();
    let is_rust = is_rust(path);
    let decl = if is_rust { &RUST_DECL } else { &TS_DECL };

    let raw_starts: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| decl.is_match(line))
        .map(|(index, _)| index)
        .collect();

    if raw_starts.is_empty() {
        return uniform_regions(&lines, max_region_lines);
    }

    // Pull each declaration's boundary back across the contiguous attribute /
    // doc / decorator lines immediately above it, so a declaration's evidence
    // retains its `#[cfg]`/`#[test]` gate, `///` docs, or `@decorator`.
    let decl_starts: Vec<usize> = raw_starts
        .iter()
        .map(|&start| {
            let mut boundary = start;
            while boundary > 0 && is_attached_attribute(lines[boundary - 1], is_rust) {
                boundary -= 1;
            }
            boundary
        })
        .collect();

    let mut regions: Vec<Region> = Vec::new();

    // A non-blank leading preamble (imports, uses, comments) becomes its own
    // region(s), chunked to stay within `max_region_lines`; the first
    // declaration region then keeps its true `start_line`.
    let first_decl = decl_starts[0];
    let has_preamble = lines[..first_decl]
        .iter()
        .any(|line| !line.trim().is_empty());
    if has_preamble {
        regions.extend(uniform_regions(&lines[..first_decl], max_region_lines));
    }

    for (position, &start) in decl_starts.iter().enumerate() {
        let end = decl_starts.get(position + 1).copied().unwrap_or(lines.len());
        let mut chunk_start = start;
        while chunk_start < end {
            let chunk_end = (chunk_start + max_region_lines).min(end);
            regions.push(build_region(&lines, regions.len() + 1, chunk_start, chunk_end));
            chunk_start = chunk_end;
        }
    }

    regions
}

fn is_rust(path: &str) -> bool {
    path.ends_with(".rs")
}

/// True when `line` annotates the *following* declaration rather than
/// trailing after the previous one: Rust attributes (`#[…]`) and doc comments
/// (`///`, `//!`), or TS/JS decorators (`@…`) and doc comments.
fn is_attached_attribute(line: &str, is_rust: bool) -> bool {
    let trimmed = line.trim_start();
    if is_rust {
        trimmed.starts_with("#[") || trimmed.starts_with("///") || trimmed.starts_with("//!")
    } else {
        trimmed.starts_with('@') || trimmed.starts_with("///")
    }
}

/// Builds one `Region` from the `start..end` line range (1-based `start_line`).
fn build_region(lines: &[&str], id: usize, start: usize, end: usize) -> Region {
    Region {
        id: format!("R{id}"),
        start_line: start + 1,
        content: lines[start..end].join("\n"),
    }
}

fn uniform_regions(lines: &[&str], lines_per_region: usize) -> Vec<Region> {
    let count = lines.len().div_ceil(lines_per_region);
    (0..count)
        .map(|i| {
            let start = i * lines_per_region;
            let end = lines.len().min(start + lines_per_region);
            build_region(lines, i + 1, start, end)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_declarations_split_but_nested_methods_do_not() {
        let src = "use std::io;\n\n// module comment\n\nfn alpha() {\n    body();\n}\n\nfn beta() {\n    other();\n}\n\nimpl Foo {\n    fn method(&self) {}\n}\n";
        let regions = function_regions(src, "src/lib.rs", 80);
        assert_eq!(regions.len(), 4);

        assert_eq!(regions[0].start_line, 1);
        assert_eq!(regions[0].content, "use std::io;\n\n// module comment\n");

        assert_eq!(regions[1].start_line, 5);
        assert_eq!(regions[1].content, "fn alpha() {\n    body();\n}\n");

        assert_eq!(regions[2].start_line, 9);
        assert_eq!(regions[2].content, "fn beta() {\n    other();\n}\n");

        assert_eq!(regions[3].start_line, 13);
        assert_eq!(regions[3].content, "impl Foo {\n    fn method(&self) {}\n}\n");
    }

    #[test]
    fn ts_class_methods_stay_with_their_class() {
        let src = "import x from \"./x\";\n\nclass Foo {\n  method() {}\n  other() {}\n}\n\nfunction bar() {}\n\nconst baz = () => {};\n";
        let regions = function_regions(src, "src/a.ts", 80);
        assert_eq!(regions.len(), 4);

        assert_eq!(regions[0].start_line, 1);
        assert_eq!(regions[0].content, "import x from \"./x\";\n");
        assert_eq!(regions[1].content, "class Foo {\n  method() {}\n  other() {}\n}\n");
        assert_eq!(regions[2].content, "function bar() {}\n");
        assert_eq!(regions[3].content, "const baz = () => {};\n");
    }

    #[test]
    fn attributes_travel_with_their_declaration() {
        let src = "fn alpha() {}\n\n#[cfg(test)]\n#[test]\nfn beta() {}\n";
        let regions = function_regions(src, "src/lib.rs", 80);
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[0].content, "fn alpha() {}\n");
        // beta's region keeps its gate/test attributes.
        assert_eq!(regions[1].start_line, 3);
        assert_eq!(regions[1].content, "#[cfg(test)]\n#[test]\nfn beta() {}\n");
    }

    #[test]
    fn long_preamble_chunks_and_decl_keeps_line() {
        let src = "// h1\n// h2\n// h3\n// h4\n// h5\n// h6\nfn main() {}\n";
        let regions = function_regions(src, "src/lib.rs", 3);
        assert_eq!(regions.len(), 3);
        assert_eq!(regions[0].start_line, 1);
        assert_eq!(regions[0].content, "// h1\n// h2\n// h3");
        assert_eq!(regions[1].content, "// h4\n// h5\n// h6");
        // The first declaration keeps its true line, not the preamble's 1.
        assert_eq!(regions[2].start_line, 7);
        assert_eq!(regions[2].content, "fn main() {}\n");
    }

    #[test]
    fn macro_rules_declaration_gets_its_own_region() {
        let src = "fn first() {}\n\nmacro_rules! my_macro {\n    () => {};\n}\n\nfn after() {}\n";
        let regions = function_regions(src, "src/lib.rs", 80);
        assert_eq!(regions.len(), 3);
        assert_eq!(regions[1].start_line, 3);
        assert_eq!(regions[1].content, "macro_rules! my_macro {\n    () => {};\n}\n");
        assert_eq!(regions[2].start_line, 7);
    }

    #[test]
    fn oversized_declaration_subdivides_uniformly() {
        let src = "fn big() {\n  a\n  b\n  c\n  d\n  e\n}";
        let regions = function_regions(src, "src/lib.rs", 3);
        assert_eq!(regions.len(), 3);
        assert_eq!(regions[0].start_line, 1);
        assert_eq!(regions[0].content, "fn big() {\n  a\n  b");
        assert_eq!(regions[1].start_line, 4);
        assert_eq!(regions[1].content, "  c\n  d\n  e");
        assert_eq!(regions[2].start_line, 7);
        assert_eq!(regions[2].content, "}");
    }

    #[test]
    fn no_declarations_falls_back_to_uniform_splitting() {
        let src = "// constants\nconst MAX: usize = 5;\n\n// marker\nstatic NAME: &str = \"x\";";
        let regions = function_regions(src, "src/lib.rs", 2);
        assert_eq!(regions.len(), 3);
        assert_eq!(regions[0].start_line, 1);
        assert_eq!(regions[0].content, "// constants\nconst MAX: usize = 5;");
        assert_eq!(regions[1].content, "\n// marker");
        assert_eq!(regions[2].content, "static NAME: &str = \"x\";");
    }

    #[test]
    fn language_detection_selects_decl_patterns() {
        // `.rs` uses Rust patterns: an indented `fn` inside an `impl` does
        // not split (the impl at column 0 is the only declaration).
        let rs = function_regions("impl Foo {\n    fn inner() {}\n}\n", "src/lib.rs", 80);
        assert_eq!(rs.len(), 1);
        assert!(rs[0].content.contains("fn inner"));

        // `.ts` uses TS/JS patterns: a top-level `function` at column 0
        // starts its own region.
        let ts = function_regions(
            "class A {\n  m() {}\n}\n\nfunction outer() {}\n",
            "src/a.ts",
            80,
        );
        assert_eq!(ts.len(), 2);
        assert!(ts[1].content.starts_with("function outer"));
    }
}