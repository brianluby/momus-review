//! Language support: which source languages are discovered, their file
//! extensions, test-file conventions, and test-body markers.
//!
//! Discovery is extension-based (no parsing), so adding a language is a row in
//! `SPECS` plus, when the language has distinctive footguns, entries in
//! `policy::LANGUAGE_MECHANISMS`. `Language::from_path` is the single source of
//! truth for "is this a file we review": change discovery
//! (`adapters/git.rs`), codebase discovery, and test-context selection all
//! route through it instead of keeping their own extension lists.
//!
//! Scope is the 20-language consensus set documented in
//! `docs/language-support.md` (top-20 agreement across TIOBE 2026, RedMonk
//! Jan-2026, the Stack Overflow 2025 survey, and Octoverse 2025).

use serde::{Deserialize, Serialize};

/// A source language momus discovers and reviews.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "camelCase")]
pub enum Language {
    Python,
    JavaScript,
    TypeScript,
    Java,
    CSharp,
    Cpp,
    C,
    Go,
    Rust,
    Php,
    Ruby,
    Kotlin,
    Swift,
    Shell,
    Sql,
    R,
    Scala,
    Dart,
    Lua,
    PowerShell,
}

/// How a test file's *name* marks it as a test, per language. Directory
/// conventions (`tests/`, `spec/`, …) are language-agnostic and handled by
/// `is_test_path`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestName {
    /// The file stem starts with the text (`test_foo.py`).
    Prefix(&'static str),
    /// The file stem ends with the text (`foo_test.go`, `FooTest.java`).
    Suffix(&'static str),
    /// The file name contains the text (`foo.spec.ts`).
    Contains(&'static str),
}

/// The static description of one language.
#[derive(Debug, Clone, Copy)]
pub struct LanguageSpec {
    pub language: Language,
    /// Log/eval key: `"rust"`, `"typescript"`, …
    pub key: &'static str,
    /// Lowercase extensions, with the leading dot. Matched case-insensitively
    /// (so `.R` is R and `.H` is C++).
    pub extensions: &'static [&'static str],
    /// Name conventions that mark a test file in this language.
    pub test_names: &'static [TestName],
    /// Lowercase substrings that mark a test body, used to pick excerpt
    /// windows out of a related test file (`compact_test`). Matched
    /// case-insensitively.
    pub test_markers: &'static [&'static str],
}

/// `.h`/`.hh`/`.hxx` are shared between C and C++; they map to C++ because
/// headers in current repositories are overwhelmingly C++, and the C++
/// mechanism vocabulary is a superset of the C one for header-level concerns
/// (`useAfterFree` is shared; `outOfBoundsAccess` reads correctly for C too).
pub const SPECS: &[LanguageSpec] = &[
    LanguageSpec {
        language: Language::Python,
        key: "python",
        extensions: &[".py", ".pyi", ".pyw"],
        test_names: &[TestName::Prefix("test_"), TestName::Suffix("_test")],
        test_markers: &["def test", "self.assert", "pytest.raises", "unittest"],
    },
    LanguageSpec {
        language: Language::JavaScript,
        key: "javascript",
        extensions: &[".js", ".jsx", ".mjs", ".cjs"],
        test_names: &[TestName::Contains(".spec."), TestName::Contains(".test.")],
        test_markers: &["describe(", "describe.", "it(", "it.", "test(", "test.", "expect("],
    },
    LanguageSpec {
        language: Language::TypeScript,
        key: "typescript",
        extensions: &[".ts", ".tsx", ".mts", ".cts"],
        test_names: &[TestName::Contains(".spec."), TestName::Contains(".test.")],
        test_markers: &["describe(", "describe.", "it(", "it.", "test(", "test.", "expect("],
    },
    LanguageSpec {
        language: Language::Java,
        key: "java",
        extensions: &[".java"],
        test_names: &[
            TestName::Suffix("Test"),
            TestName::Suffix("Tests"),
            TestName::Suffix("IT"),
            TestName::Suffix("Spec"),
        ],
        test_markers: &["@test", "assertequals", "assertthat(", "assertthrows("],
    },
    LanguageSpec {
        language: Language::CSharp,
        key: "csharp",
        extensions: &[".cs"],
        test_names: &[TestName::Suffix("Test"), TestName::Suffix("Tests")],
        test_markers: &["[fact]", "[theory]", "[test]", "assert."],
    },
    LanguageSpec {
        language: Language::Cpp,
        key: "cpp",
        extensions: &[".cpp", ".cc", ".cxx", ".hpp", ".hh", ".hxx", ".h"],
        test_names: &[
            TestName::Prefix("test_"),
            TestName::Suffix("_test"),
            TestName::Suffix("Test"),
        ],
        test_markers: &["test(", "test_f(", "expect_", "assert_", "check("],
    },
    LanguageSpec {
        language: Language::C,
        key: "c",
        extensions: &[".c"],
        test_names: &[
            TestName::Prefix("test_"),
            TestName::Suffix("_test"),
            TestName::Suffix("Test"),
        ],
        test_markers: &["test(", "test_f(", "expect_", "assert_", "check("],
    },
    LanguageSpec {
        language: Language::Go,
        key: "go",
        extensions: &[".go"],
        test_names: &[TestName::Suffix("_test")],
        test_markers: &["func test", "t.error", "t.fatal", "t.run(", "t.parallel("],
    },
    LanguageSpec {
        language: Language::Rust,
        key: "rust",
        extensions: &[".rs"],
        test_names: &[TestName::Suffix("_test")],
        test_markers: &["#[test]", "#[tokio::test]", "#[cfg(test)]", "fn "],
    },
    LanguageSpec {
        language: Language::Php,
        key: "php",
        extensions: &[".php"],
        test_names: &[TestName::Suffix("Test")],
        test_markers: &["function test", "assertsame(", "asserttrue(", "expectexception("],
    },
    LanguageSpec {
        language: Language::Ruby,
        key: "ruby",
        extensions: &[".rb", ".rake"],
        test_names: &[TestName::Suffix("_spec"), TestName::Suffix("_test")],
        test_markers: &["describe ", "context ", "it ", "expect(", "assert_"],
    },
    LanguageSpec {
        language: Language::Kotlin,
        key: "kotlin",
        extensions: &[".kt", ".kts"],
        test_names: &[
            TestName::Suffix("Test"),
            TestName::Suffix("Tests"),
            TestName::Suffix("Spec"),
        ],
        test_markers: &["@test", "assertequals", "assertthat(", "assertthrows("],
    },
    LanguageSpec {
        language: Language::Swift,
        key: "swift",
        extensions: &[".swift"],
        test_names: &[TestName::Suffix("Tests"), TestName::Suffix("Test")],
        test_markers: &["xctassert", "func test"],
    },
    LanguageSpec {
        language: Language::Shell,
        key: "shell",
        extensions: &[".sh", ".bash", ".zsh", ".bats"],
        test_names: &[TestName::Suffix(".bats"), TestName::Prefix("test_")],
        test_markers: &["@test", "assert_", "bats_"],
    },
    LanguageSpec {
        language: Language::Sql,
        key: "sql",
        extensions: &[".sql"],
        test_names: &[],
        test_markers: &[],
    },
    LanguageSpec {
        language: Language::R,
        key: "r",
        extensions: &[".r"],
        test_names: &[TestName::Prefix("test-")],
        test_markers: &["test_that(", "expect_", "context("],
    },
    LanguageSpec {
        language: Language::Scala,
        key: "scala",
        extensions: &[".scala", ".sc"],
        test_names: &[
            TestName::Suffix("Spec"),
            TestName::Suffix("Test"),
            TestName::Suffix("Suite"),
        ],
        test_markers: &["\"should\"", "should ", "expect(", "assert("],
    },
    LanguageSpec {
        language: Language::Dart,
        key: "dart",
        extensions: &[".dart"],
        test_names: &[TestName::Suffix("_test")],
        test_markers: &["test(", "expect(", "group(", "setup("],
    },
    LanguageSpec {
        language: Language::Lua,
        key: "lua",
        extensions: &[".lua"],
        test_names: &[TestName::Suffix("_spec")],
        test_markers: &["describe(", "it(", "assert_equal", "assert("],
    },
    LanguageSpec {
        language: Language::PowerShell,
        key: "powershell",
        extensions: &[".ps1", ".psm1", ".psd1"],
        test_names: &[TestName::Suffix("Tests.ps1"), TestName::Contains(".tests.")],
        test_markers: &["describe ", "should ", "assert-"],
    },
];

/// Directory names whose files are test context in every language.
const TEST_DIRS: [&str; 6] = ["test", "tests", "__tests__", "spec", "specs", "testdata"];

impl Language {
    /// The static description of this language.
    pub fn spec(self) -> &'static LanguageSpec {
        SPECS
            .iter()
            .find(|spec| spec.language == self)
            .expect("every Language variant has a spec")
    }

    /// The log/eval key (`"rust"`, `"typescript"`, …).
    pub fn key(self) -> &'static str {
        self.spec().key
    }

    /// The language of `path`, from its extension. `None` for a file we do not
    /// review (or one with no extension).
    pub fn from_path(path: &str) -> Option<Language> {
        let (_, extension) = path.rsplit_once('.')?;
        if extension.is_empty() || extension.contains('/') {
            return None;
        }
        SPECS
            .iter()
            .find(|spec| {
                spec.extensions
                    .iter()
                    .any(|candidate| extension.eq_ignore_ascii_case(&candidate[1..]))
            })
            .map(|spec| spec.language)
    }

    /// True when `file_name` (the last path component) follows this language's
    /// test-file naming convention. The stem is checked too, so `_test` matches
    /// `store_test.go` while `.spec.` still matches `Widget.spec.ts`.
    pub fn is_test_name(self, file_name: &str) -> bool {
        let stem = file_name
            .rsplit_once('.')
            .map_or(file_name, |(stem, _)| stem);
        self.spec().test_names.iter().any(|convention| {
            matches_convention(file_name, *convention) || matches_convention(stem, *convention)
        })
    }

    /// Lowercase substrings that mark a test body in this language.
    pub fn test_markers(self) -> &'static [&'static str] {
        self.spec().test_markers
    }
}

/// Case-insensitive match of one naming convention against a file name or
/// stem, without allocating a lowercased copy.
fn matches_convention(name: &str, convention: TestName) -> bool {
    match convention {
        TestName::Prefix(text) => name
            .get(..text.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(text)),
        TestName::Suffix(text) => match name.len().checked_sub(text.len()) {
            Some(start) => name
                .get(start..)
                .is_some_and(|tail| tail.eq_ignore_ascii_case(text)),
            None => false,
        },
        TestName::Contains(text) => name
            .as_bytes()
            .windows(text.len())
            .any(|window| window.eq_ignore_ascii_case(text.as_bytes())),
    }
}

/// Every supported language, in vocabulary order.
pub fn languages() -> impl Iterator<Item = Language> {
    SPECS.iter().map(|spec| spec.language)
}

/// True when `path` is a file this tool reviews.
pub fn is_source_path(path: &str) -> bool {
    Language::from_path(path).is_some()
}

/// True when `path` is test context rather than code under review: a file in a
/// test directory, or one named per its language's test convention.
pub fn is_test_path(path: &str) -> bool {
    let mut components = path.split('/').peekable();
    let mut file_name = path;
    while let Some(component) = components.next() {
        if components.peek().is_none() {
            file_name = component;
            break;
        }
        if TEST_DIRS
            .iter()
            .any(|dir| component.eq_ignore_ascii_case(dir))
        {
            return true;
        }
    }
    Language::from_path(path).is_some_and(|language| language.is_test_name(file_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_language_has_a_distinct_key_and_at_least_one_extension() {
        let mut keys: Vec<&str> = Vec::new();
        for language in languages() {
            let spec = language.spec();
            assert!(
                !spec.extensions.is_empty(),
                "{} has no extensions",
                spec.key
            );
            assert!(
                spec.extensions.iter().all(|e| e.starts_with('.')),
                "{} has a malformed extension",
                spec.key
            );
            assert!(
                !keys.contains(&spec.key),
                "duplicate language key {:?}",
                spec.key
            );
            keys.push(spec.key);
        }
        assert_eq!(keys.len(), 20, "the consensus set is 20 languages");
    }

    #[test]
    fn from_path_covers_every_declared_extension() {
        for language in languages() {
            for extension in language.spec().extensions {
                let path = format!("src/file{extension}");
                assert_eq!(
                    Language::from_path(&path),
                    Some(language),
                    "{path} should be {language:?}"
                );
            }
        }
    }

    #[test]
    fn from_path_is_case_insensitive_and_rejects_non_sources() {
        assert_eq!(Language::from_path("analysis/x.R"), Some(Language::R));
        assert_eq!(Language::from_path("src/Widget.H"), Some(Language::Cpp));
        assert_eq!(Language::from_path("README.md"), None);
        assert_eq!(Language::from_path("data.csv"), None);
        assert_eq!(Language::from_path("Makefile"), None);
        assert_eq!(Language::from_path("src/lib.rs.bak"), None);
        assert_eq!(Language::from_path(""), None);
    }

    #[test]
    fn shared_keys_are_stable() {
        // The keys the dashboard and the SARIF ruleIds carry.
        assert_eq!(Language::Rust.key(), "rust");
        assert_eq!(Language::CSharp.key(), "csharp");
        assert_eq!(Language::PowerShell.key(), "powershell");
    }

    #[test]
    fn test_paths_follow_language_conventions() {
        // Directory conventions, language-agnostic.
        assert!(is_test_path("tests/wiring.py"));
        assert!(is_test_path("src/__tests__/helpers.ts"));
        assert!(is_test_path("src/test/java/Widget.java"));
        assert!(is_test_path("spec/models/user_spec.rb"));
        assert!(!is_test_path("src/testing_util.rs"));
        assert!(!is_test_path("src/latest.rs"));

        // Name conventions per language.
        assert!(is_test_path("pkg/test_parser.py"));
        assert!(is_test_path("pkg/parser_test.py"));
        assert!(is_test_path("internal/store/store_test.go"));
        assert!(is_test_path("src/main/java/WidgetTest.java"));
        assert!(is_test_path("src/Widget.spec.ts"));
        assert!(is_test_path("src/Widget.test.tsx"));
        assert!(is_test_path("Tests/WidgetTests.swift"));
        assert!(is_test_path("scripts/install.bats"));
        assert!(is_test_path("R/test-parse.R"));

        // A file in a test directory counts as test context regardless of its
        // extension (discovery never yields non-source files in the first
        // place, so this only matters for the classification itself).
        assert!(is_test_path("tests/fixture.json"));

        // Language-specific name conventions must not leak across languages:
        // `foo_test.go` is Go's convention, not Rust's reading of `_test`.
        assert!(!is_test_path("src/parser_helpers.py"));
        assert!(!is_test_path("src/Widget.java"));
    }

    #[test]
    fn test_markers_are_lowercase_because_matching_is_case_insensitive() {
        for language in languages() {
            for marker in language.test_markers() {
                assert_eq!(
                    *marker,
                    marker.to_lowercase(),
                    "{:?} marker {marker:?} must be lowercase",
                    language
                );
            }
        }
    }
}
