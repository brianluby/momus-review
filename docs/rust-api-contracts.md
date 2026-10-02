# Rust API contracts

Momus's Rust API documentation focuses on contracts a caller can get wrong:
repository identity, evidence completeness, diff locations, redaction, budgets,
caches and approval gates. These contracts live beside the APIs in rustdoc.
This guide maps the starting points; it does not define a documentation coverage
percentage or require comments on every internal helper.

Build the API reference and run its executable examples with the repository's
pinned toolchain:

```sh
cargo doc --locked --no-deps
cargo test --locked --doc
```

The reference starts at `target/doc/momus_review/index.html`. CI also checks
broken rustdoc links and invalid code-block attributes. The examples run offline
without model credentials; examples needing temporary cache files use isolated
temporary directories.

| API | Contract to read before use |
| --- | --- |
| [`domain::report`](../src/domain/report.rs) | Plain report data is not proof of completed review. Read skipped/deferred work, scope and optional assessment artifacts alongside findings. |
| [`domain::patch`](../src/domain/patch.rs) | Supply per-file unified diffs; understand one-based anchors, deletion mapping and parser fallbacks before using locations. |
| [`domain::repository`](../src/domain/repository.rs) | Removed content, present empty text and unavailable baselines have different meanings. Callers must preserve provenance. |
| [`domain::redact`](../src/domain/redact.rs) | Redaction is best-effort. Newline preservation does not preserve byte columns; JSON keys and unmatched secrets need separate care. |
| [`adapters::git`](../src/adapters/git.rs) | Working-tree input and pinned commits provide different evidence. Scope, UTF-8, path guards and bounded auxiliary omissions affect completeness. |
| [`adapters::exclude`](../src/adapters/exclude.rs) | Exclusions use a limited glob language; malformed patterns are errors and callers supply normalized repository-relative paths. |
| [`adapters::cache`](../src/adapters/cache.rs) | Cache keys bind specific inputs, but parsed cache JSON is not authenticated evidence. Storage remains caller-owned and may contain sensitive response text. |
| [`review::budget`](../src/review/budget.rs) | Reserve before an attempt. Failed requests and retries consume reservations; exhaustion records deferral and does not enqueue a rerun. |
| [`review::merge_confidence`](../src/review/merge_confidence.rs) | Missing labels stay unknown. Validation checks supplied metadata; observed estimates require mature, supported history and remain distinct from heuristic scores. |
| [`adapters::approval`](../src/adapters/approval.rs) | Invalid inputs and unavailable evidence differ. Eligibility is opt-in, head-bound and fail-closed; posting an approval is an external action with its own failure modes. |

Doctests exercise representative caller contracts, including serialization
distinctions, patch anchors, redaction boundaries, cache isolation, exclusions,
budget exhaustion, unknown outcomes and default-disabled approval. They
complement the unit and integration tests; they do not establish empirical
model accuracy or real-history calibration.

When changing a documented contract, update its rustdoc example and the
appropriate regression tests together. Explain preconditions, unknown states
and failure behavior before adding prose for obvious fields. Avoid `no_run` or
`ignore` for examples that can run locally without external services.

For workflow-level context, see [architecture](architecture.md),
[security](security.md), [scaling](scaling.md) and
[merge-confidence outcomes](merge-confidence.md).
