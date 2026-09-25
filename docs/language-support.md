# Language Support

Discovery is two regexes in `domain/config.ts` plus test-context helpers.
All judgments inspect raw text (`file.patch`, `file.content`) — no parsing —
so new languages are mostly configuration.

## Current State

- `SOURCE_FILE = /\.(?:[cm]?[jt]sx?|rs)$/` — JS/TS (+m/c variants) and Rust.
  Consumed by `adapters/git.ts` (changes) and `adapters/repository-files.ts`
  (codebase). Non-matching files are silently skipped; empty result throws
  `No <subject> files found under <scope>` (`review/workflow.ts`).
- `TEST_FILE = /(?:^|\/)(?:tests?|__tests__)(?:\/|$)|\.(?:spec|test)\.[cm]?[jt]sx?$/`
  — directory half is language-agnostic (`tests/`, `__tests__/`); the
  `.(spec|test).` half is JS-only.
- `compactTest` (`review/codebase-judgments.ts`) trims related tests to
  ±2 lines around markers: stem match, `describe(`/`test(`/`it(`,
  plus Rust `#[test]`, `#[tokio::test]`, `fn `, `assert`. Falls back to
  full content capped at 1800 chars (head/tail split).
- `selectRelatedTests`: stem-in-path (+2) and same-directory (+1), top 4.
- Rust inline `#[cfg(test)]` modules ride along in the source file; `tests/`
  dirs match `TEST_FILE` and serve as test-gap context.

Verified on a fixture crate: codebase scan finds `src/*.rs` + `tests/*.rs`,
change scan diffs edited `.rs`, `skip.go` ignored.

## Adding a Language

1. Extend `SOURCE_FILE` with the extension(s).
2. Extend `TEST_FILE` if the ecosystem has file-level test conventions
   (`test_*.py`, `*_test.go`, `*Test.java`, …). Directory conventions are
   usually already covered.
3. Add test-body markers to `compactTest` (attribute/annotation + assert
   keywords). Keep the fallback: full content, capped.
4. Consider per-language `mechanisms` (e.g. Rust `unsafe`/`unwrap`/`panic!`,
   TS `any`/casts). Dimensions stay fixed; choices sharpen.
5. Smoke-test: fixture repo with src + tests + a non-matching file; assert
   discovery sets for both modes.

## Limits

- Chunking is 80 dumb lines (`domain/patch.ts`, `sourceRegions`) — splits
  mid-function. Function-aware regions (tree-sitter or heuristic) are a
  roadmap item and matter more as languages grow.
- No generated-file detection, no secret redaction before upload.
- Both adapters require git (`rev-parse`/`diff`/`ls-files`); non-git scopes fail.
