# Language Support

Discovery is extension-based (no parsing), and `domain/language.rs` is the
single source of truth: `Language::from_path` decides whether a file is
reviewed, `Language::is_test_name` / `domain::language::is_test_path` decide
whether it is test context, and `policy::mechanisms_for` decides which
mechanisms the classifier may choose for it. Change discovery
(`adapters/git.rs`), codebase discovery, and test-context selection all route
through those tables — there is no second extension list to keep in sync.

## Supported languages (20)

Chosen by cross-source agreement across TIOBE (Sep 2026), RedMonk (Jan 2026),
the Stack Overflow 2025 survey, and GitHub Octoverse 2025. Twelve languages
rank top-20 in at least three of the four — Python, JavaScript, Java, C#, C++,
PHP, Go, TypeScript, C, shell, Rust, Swift — six more in two (SQL, Ruby,
PowerShell, R, Kotlin, Dart), and Lua (Stack Overflow #16) and Scala (RedMonk
#14) round the set out on one strong axis each.

Excluded despite ranking: HTML/CSS (markup, not code the five dimensions
describe), Assembly and Objective-C (legacy, declining), and the TIOBE-only
legacy entries (COBOL, Ada, Fortran, Scratch, Visual Basic, Delphi). Every one
of them is a row in `language.rs` plus a mechanism table away from being
supported. `docs/security-taxonomy.md` records the per-language security
classes this adds.

| language | extensions | test naming | test-body markers |
|---|---|---|---|
| Python | `.py` `.pyi` `.pyw` | `test_*`, `*_test` | `def test`, `self.assert`, `pytest.raises`, `unittest` |
| JavaScript | `.js` `.jsx` `.mjs` `.cjs` | `*.spec.*`, `*.test.*` | `describe(`, `it(`, `test(`, `expect(` |
| TypeScript | `.ts` `.tsx` `.mts` `.cts` | `*.spec.*`, `*.test.*` | as JavaScript |
| Java | `.java` | `*Test`, `*Tests`, `*IT`, `*Spec` | `@Test`, `assertEquals`, `assertThat(`, `assertThrows(` |
| C# | `.cs` | `*Test`, `*Tests` | `[Fact]`, `[Theory]`, `Assert.` |
| C++ | `.cpp` `.cc` `.cxx` `.hpp` `.hh` `.hxx` `.h` | `test_*`, `*_test`, `*Test` | `TEST(`, `TEST_F(`, `EXPECT_`, `ASSERT_` |
| C | `.c` | `test_*`, `*_test`, `*Test` | as C++ |
| Go | `.go` | `*_test` | `func Test`, `t.Error`, `t.Fatal`, `t.Run(` |
| Rust | `.rs` | `*_test` | `#[test]`, `#[tokio::test]`, `#[cfg(test)]` |
| PHP | `.php` | `*Test` | `function test`, `assertSame(`, `@test` |
| Ruby | `.rb` `.rake` | `*_spec`, `*_test` | `describe `, `context `, `it `, `expect(` |
| Kotlin | `.kt` `.kts` | `*Test`, `*Tests`, `*Spec` | `@Test`, `assertEquals`, `assertThat(` |
| Swift | `.swift` | `*Test`, `*Tests` | `XCTAssert`, `func test` |
| Shell | `.sh` `.bash` `.zsh` `.bats` | `*.bats`, `test_*` | `@test`, `assert_`, `bats_` |
| SQL | `.sql` | — | — |
| R | `.r` | `test-*` | `test_that(`, `expect_`, `context(` |
| Scala | `.scala` `.sc` | `*Spec`, `*Test`, `*Suite` | `"should"`, `should `, `expect(`, `assert(` |
| Dart | `.dart` | `*_test` | `test(`, `expect(`, `group(` |
| Lua | `.lua` | `*_spec` | `describe(`, `it(`, `assert_equal` |
| PowerShell | `.ps1` `.psm1` `.psd1` | `*Tests.ps1`, `*.tests.*` | `describe `, `should `, `assert-` |

Directory conventions shared by every language, at any depth: `test/`,
`tests/`, `__tests__/`. Three languages add their own directory: `spec/` for
JavaScript/TypeScript and Ruby, `testdata/` for Go. A directory rule moves
files out of review and into test context, so it is scoped to the languages
whose ecosystem actually uses it — `spec/parser.rs` is still reviewed.

Extensions match case-insensitively, so `.R` is R and `.H` is C++. `.h` is
shared with C and maps to C++ (headers in current repositories are
overwhelmingly C++, and the two vocabularies share `useAfterFree`).

## Per-language mechanisms

Dimensions stay fixed (correctness, security, reliability, compatibility, test
gap); the `choice` vocabulary for the mechanism question sharpens per language.
`policy::mechanisms_for(dimension, language)` returns the generic entries with
the language's additions spliced ahead of the `other` sentinel, so `other` and
`noIssue` keep their positions and a language never shadows a generic key.

Examples: Rust `unsafeBlock` / `unwrapPanic` / `panicPath`, TypeScript
`anyEscape` / `uncheckedCast` / `nonNullAssertion`, Go `ignoredError` /
`goroutineLeak`, C `bufferOverflow` / `formatString` / `useAfterFree`, shell
`unquotedExpansion` / `ignoredExitStatus`, SQL `missingWhereClause`. A key is
shared across languages only when it means the same thing in both
(`useAfterFree` for C and C++, `nonNullAssertion` for TS/Kotlin/Swift/Dart),
which lets `explain::mechanism_title` stay keyed on dimension + key.

`testGap` deliberately has no language additions: its generic
`branch`/`failure`/`boundary`/`integration` entries already name every test gap
worth flagging.

Every vocabulary is checked exhaustively by tests: keys are unique within a
(language, dimension) and never shadow a generic key; each language contributes
at least one mechanism; every key has a title
(`review/explain.rs`); every **security** key maps to an evaluator category
(`bin/momus-eval.rs`).

## Adding a Language

1. Add a `LanguageSpec` row to `SPECS` in `domain/language.rs`: key,
   extensions, test-name conventions, test-body markers.
2. Add its mechanism rows to `policy::LANGUAGE_MECHANISMS` — at least one, and
   a title per key in `explain::mechanism_title` (the exhaustive test enforces
   both).
3. **Security** keys additionally need a branch in
   `bin/momus-eval.rs::mechanism_categories`, or the evaluator stops counting
   that class (also enforced by a test).
4. Extend the docs table above and, if the language's footguns change the
   security picture, `docs/security-taxonomy.md`.
5. Smoke-test: a fixture repo with a file in the new language plus a non-source
   file; assert discovery (matrix) and, where possible, that a mechanism the
   language owns comes back classified.

## Limits

- **Import graph** (`adapters/imports.rs`) extracts edges for Rust and JS/TS
  only. Every other language contributes no edges — a missing neighborhood
  rather than a neighborhood guessed with another language's patterns.
- **Region splitting** (`review/regions.rs`) uses Rust declaration patterns for
  `.rs` and the generic JS/TS-ish patterns for everything else; a language it
  does not recognize falls back to fixed-size windows (80 lines, 160 for
  screening), the same as before.
- No generated-file detection, no secret redaction before upload.
- Both adapters require git (`rev-parse`/`diff`/`ls-files`); non-git scopes
  fail.

## Verification

`cargo test` covers the discovery tables, the vocabulary invariants, and the
path → language → criteria chain (`review/typesafe.rs`). Beyond that, a
seven-language fixture (Rust, Go, C, shell, Python, TypeScript, SQL) scanned
with `momus scan`:

- `matrix` lists every fixture file — `.c`, `.go`, `.py`, `.rs`, `.sh`, `.sql`,
  `.ts` are all discovered and screened.
- Classified language mechanisms come back with their own titles and
  descriptions: `py/app.py` → `dynamicCodeExecution` ("Untrusted text is
  executed as code (`eval`, `exec`)"), `sh/deploy.sh` → `ignoredExitStatus`
  ("A failing command's exit status is ignored and the script continues").
