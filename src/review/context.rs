//! Bounded context packs (docs/scaling.md §1): every screen request's state
//! is assembled under one character budget, and related tests are selected
//! the same way in both review modes — at most `MAX_RELATED_TESTS`, each
//! compacted to its marker lines — so no context source can grow a request
//! without limit.

use std::collections::BTreeSet;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::domain::language::Language;
use crate::domain::report::{ChangedFile, SourceFile};
use crate::review::typesafe::parse_positive;

/// At most this many related tests travel with one screened file.
pub const MAX_RELATED_TESTS: usize = 4;

/// A related test's snippet is trimmed to this many characters.
const MAX_TEST_SNIPPET_CHARS: usize = 1_800;

/// Default per-request context budget, in characters. Provisional pending
/// the measured model input limit from ticket #32; sized at roughly 24k
/// tokens with margin.
pub const DEFAULT_CONTEXT_BUDGET_CHARS: usize = 96_000;

/// `MOMUS_CONTEXT_BUDGET_CHARS` overrides the default budget.
const BUDGET_ENV: &str = "MOMUS_CONTEXT_BUDGET_CHARS";

/// Context that did not fit a request's budget and was never sent,
/// accumulated across a run and reported in `workflow`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ContextDrops {
    /// Characters trimmed off the tail of (or wholly dropped from) context.
    pub chars: usize,
    /// How many context items (unit, base, test, neighbor, region) were
    /// trimmed or dropped.
    pub items: usize,
}

impl std::ops::Add for ContextDrops {
    type Output = ContextDrops;
    fn add(self, rhs: ContextDrops) -> ContextDrops {
        ContextDrops {
            chars: self.chars + rhs.chars,
            items: self.items + rhs.items,
        }
    }
}

/// One request's context budget. Sources are added in priority order — the
/// unit under review, then its base (diff mode), related tests, neighbors —
/// and each takes what it needs from the remaining characters; whatever does
/// not fit is trimmed and counted, never sent. The budget counts context
/// characters, not the JSON keys and escaping around them.
#[derive(Debug)]
pub struct ContextBudget {
    remaining: usize,
    /// What this budget could not fit.
    pub drops: ContextDrops,
}

impl ContextBudget {
    /// Reads `MOMUS_CONTEXT_BUDGET_CHARS`, falling back to the default.
    pub fn from_env() -> Result<Self> {
        let cap = match std::env::var(BUDGET_ENV) {
            Ok(raw) => parse_positive(BUDGET_ENV, &raw)?,
            Err(_) => DEFAULT_CONTEXT_BUDGET_CHARS,
        };
        Ok(Self::with_cap(cap))
    }

    pub fn with_cap(cap: usize) -> Self {
        Self {
            remaining: cap,
            drops: ContextDrops::default(),
        }
    }

    /// Returns `text` if it fits the remaining budget, else its fitting
    /// prefix, counting the overflow as dropped.
    pub fn take(&mut self, text: &str) -> String {
        let total = text.chars().count();
        if total <= self.remaining {
            self.remaining -= total;
            text.to_string()
        } else {
            let kept: String = text.chars().take(self.remaining).collect();
            self.drops.chars += total - kept.chars().count();
            self.drops.items += 1;
            self.remaining = 0;
            kept
        }
    }
}

fn basename(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

fn dirname(path: &str) -> String {
    match path.rsplit_once('/') {
        Some((dir, _)) => dir.to_string(),
        None => ".".to_string(),
    }
}

fn stem(path: &str) -> String {
    match basename(path).rsplit_once('.') {
        Some((s, _)) => s.to_string(),
        None => basename(path),
    }
}

/// Relatedness of `test_path` to `source_path`: the source's stem in the
/// test's path counts double, a shared directory prefix counts once.
fn relatedness(source_path: &str, test_path: &str) -> i32 {
    let stem = stem(source_path);
    let directory = dirname(source_path);
    usize::from(test_path.contains(&stem)) as i32 * 2
        + usize::from(test_path.starts_with(&directory)) as i32
}

/// Orders `tests` by relatedness to `source_path` (ties by path, so the
/// selection is deterministic), keeps the top `MAX_RELATED_TESTS`.
fn ranked<'a, T>(
    source_path: &str,
    tests: &'a [T],
    path: impl Fn(&T) -> &str,
) -> Vec<(i32, &'a T)> {
    let mut scored: Vec<(i32, &T)> = tests
        .iter()
        .map(|test| (relatedness(source_path, path(test)), test))
        .filter(|(score, _)| *score > 0)
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| path(a.1).cmp(path(b.1))));
    scored.truncate(MAX_RELATED_TESTS);
    scored
}

/// The test's marker lines (plus two lines of neighborhood each): test
/// declarations, assertions, and mentions of the source's stem, trimmed to
/// `MAX_TEST_SNIPPET_CHARS`. Falls back to the whole content when nothing
/// matches, so a related test never arrives empty.
fn snippet(test_path: &str, content: &str, source_stem: &str) -> String {
    let lines: Vec<&str> = content.split('\n').collect();
    let mut selected: BTreeSet<usize> = BTreeSet::new();
    let stem = source_stem.to_lowercase();
    // Test bodies are matched on the test file's own language markers
    // (`#[test]`, `func Test`, `describe(`, …) plus the language-neutral
    // `assert`, which every ecosystem spells the same way.
    let markers = Language::from_path(test_path).map_or(&[][..], |lang| lang.test_markers());

    for (index, line) in lines.iter().enumerate() {
        let lower = line.to_lowercase();
        if lower.contains(&stem)
            || lower.contains("assert")
            || markers.iter().any(|m| lower.contains(m))
        {
            for nearby in index.saturating_sub(2)..=(lines.len() - 1).min(index + 2) {
                selected.insert(nearby);
            }
        }
    }

    let mut snippet: String = selected
        .iter()
        .map(|&i| lines[i])
        .collect::<Vec<_>>()
        .join("\n");
    if snippet.is_empty() {
        snippet = content.to_string();
    }
    if snippet.chars().count() > MAX_TEST_SNIPPET_CHARS {
        let side = (MAX_TEST_SNIPPET_CHARS - 7) / 2;
        let head: String = snippet.chars().take(side).collect();
        let tail: String = snippet
            .chars()
            .skip(snippet.chars().count() - side)
            .collect();
        snippet = format!("{head}\n...\n{tail}");
    }
    snippet
}

/// Scan mode: the changed file's related tests, selected and compacted.
pub fn select_related_tests(file: &SourceFile, test_files: &[SourceFile]) -> Vec<SourceFile> {
    let stem = stem(&file.path);
    ranked(&file.path, test_files, |t| t.path.as_str())
        .into_iter()
        .map(|(_, test)| compact_test(test, &stem))
        .collect()
}

fn compact_test(test: &SourceFile, source_stem: &str) -> SourceFile {
    SourceFile {
        path: test.path.clone(),
        content: snippet(&test.path, &test.content, source_stem),
    }
}

/// Diff mode: the same selection over changed test files. Each selected
/// test travels as its compacted patch (`base` is bulk a diff does not
/// need; scan mode sends no base either).
pub fn select_related_changed_tests(
    file: &ChangedFile,
    changed_tests: &[ChangedFile],
) -> Vec<ChangedFile> {
    let stem = stem(&file.path);
    ranked(&file.path, changed_tests, |t| t.path.as_str())
        .into_iter()
        .map(|(_, test)| ChangedFile {
            path: test.path.clone(),
            patch: snippet(&test.path, &test.patch, &stem),
            base: String::new(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_budget_keeps_early_sources_whole_and_trims_the_tail() {
        let mut budget = ContextBudget::with_cap(10);
        assert_eq!(budget.take("unit"), "unit");
        assert_eq!(budget.take("base!"), "base!");
        assert_eq!(budget.take("trimmed"), "t");
        assert_eq!(budget.drops, ContextDrops { chars: 6, items: 1 });
    }

    #[test]
    fn a_source_gets_at_most_four_ranked_tests() {
        let source = SourceFile {
            path: "src/widget.rs".into(),
            content: String::new(),
        };
        let mk = |i: usize| SourceFile {
            path: format!("tests/widget_{i}.test.ts"),
            content: String::new(),
        };
        let tests: Vec<SourceFile> = (0..300).map(mk).collect();
        let related = select_related_tests(&source, &tests);
        assert_eq!(related.len(), MAX_RELATED_TESTS);
        let expected = [
            "tests/widget_0.test.ts",
            "tests/widget_1.test.ts",
            "tests/widget_10.test.ts",
            "tests/widget_100.test.ts",
        ];
        assert_eq!(
            related.iter().map(|t| t.path.clone()).collect::<Vec<_>>(),
            expected
        );

        // Unrelated tests (no stem match, no shared directory) never travel.
        let other = SourceFile {
            path: "tests/other.test.ts".into(),
            content: String::new(),
        };
        assert!(select_related_tests(&source, &[other]).is_empty());
    }

    #[test]
    fn a_changed_test_travels_as_its_compacted_patch_without_base() {
        let file = ChangedFile {
            path: "src/widget.rs".into(),
            patch: String::new(),
            base: "old".into(),
        };
        let test = ChangedFile {
            path: "tests/widget.test.ts".into(),
            patch: "import x;\ndescribe('widget', () => {\n  it('works', () => {\n    assert.ok(widget);\n  });\n});\n"
                .into(),
            base: "old test".into(),
        };
        let related = select_related_changed_tests(&file, &[test]);
        assert_eq!(related.len(), 1);
        assert!(related[0].patch.contains("assert.ok(widget);"));
        assert!(related[0].base.is_empty());
    }
}
