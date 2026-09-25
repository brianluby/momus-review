# Momus Review

Finds fault in the gods' own work — so it can find fault in yours.

Momus (Μῶμος) is the Greek god of satire, mockery, and criticism. Legend has
it he criticized Zeus's own creations. `momus` is a code/security review
product that does the same to your diffs: fast, calibrated, staged judgments
with concrete evidence, severity, and owner routing.

- Crate: `momus-review` (free on crates.io as of 2026-09-25)
- Binary: `momus`
- Engine: [TypeSafe Jev](https://typesafe.ai) — `@typesafe-ai/sdk` in the
  TypeScript prototype; a thin HTTP client speaking the `system_one` wire
  format directly in the Rust port (see `docs/rust-types.md`)
- Status: prototype proven in TypeScript (`jev-review` fork); Rust port of the
  core funnel (`review`/`scan`/`dashboard`) is implemented and smoke-tested
  against a live key

## What It Does

Two modes, one funnel:

- `momus review` — review the current Git diff (tracked changes + untracked files)
- `momus scan` — scan every non-ignored source file under a scope

Both accept one or more scope directories (unioned into one run) plus
`--exclude GLOB` (skip vendored/third-party subtrees), `--follow-ups N`
(opt into a follow-up budget; unlimited by default), and
`--fail-on-blocking` (CI exit contract).

Both run the same staged pipeline: cheap risk screening across five
dimensions (correctness, security, reliability, compatibility, test gap),
then focused follow-ups on the strongest signals only — evidence selection,
mechanism classification, severity scoring, reviewer routing. Results land in
a quiet local dashboard and machine-readable JSON.

## Why Jev, Why This Shape

Jev is a decision model: typed questions (`noul`/`choice`/`score`) against a
state, with calibrated probabilities — fast enough to sit inside code paths.
Momus keeps orchestration in code (thresholds, budgets, ranking) and uses Jev
only for bounded judgments. That gives:

- Cost control: screen everything cheaply, follow up every threshold signal
  (unlimited by default, cap via `--follow-ups N`), so no finding is silently
  dropped by an arbitrary budget
- Calibration: probabilities mean something, so thresholds and routing work
- Composability: narrow calls chain into taint analysis, meta-judgment,
  pairwise ranking — things monolithic prompts fumble

See `docs/jev-pipeline.md` for the full judgment graph.

## Prototype Lineage

Built from a hardened TypeScript prototype:

- Upstream: `devagrawal09/jev-review`
- Fork: `brianluby/jev-review` — security hardening (symlink-safe reads,
  atomic report writes, dashboard Host/CSP hardening) submitted upstream as
  [PR #10](https://github.com/devagrawal09/jev-review/pull/10)
- Prototype also proved Rust language support (`.rs` discovery + test markers)

See `docs/architecture.md` (what exists), `docs/security.md` (threat model +
fixes), `docs/rust-port.md` (migration plan).

## Docs

- `docs/architecture.md` — prototype layout, target Rust layout, module boundaries
- `docs/jev-pipeline.md` — judgment graph, prompts-as-policy, budgets
- `docs/language-support.md` — how file discovery and test context work, adding languages
- `docs/security.md` — threat model, FIND-001–004, secure defaults
- `docs/roadmap.md` — Now/Next/Later, metrics, open bets
- `docs/rust-port.md` — TS→Rust conversion plan, SDK parity checklist
- `docs/rust-types.md` — the shipped Rust types/traits + the `jev_sdk` finding
- `docs/distribution.md` — `cargo install`, `npx`-style runs, CI gating

## Validation

A single `momus scan` of [OWASP Juice Shop](https://owasp.org/www-project-juice-shop/)
at commit `1618a611b` (2026-08-10), vendored assets excluded:

```bash
momus scan . \
  --exclude 'frontend/src/assets/**' \
  --exclude 'data/static/codefixes/**' \
  --exclude 'data/static/contractABIs.ts'
```

| metric | value |
|---|---|
| source files screened | 299 (1,495 cells = 299 × 5 dimensions) |
| test files used as context | 249 |
| signals ≥ 0.7 | 357 |
| located findings | 99 (86 routed to an owner) |
| `request_changes` / `comment` | 34 / 65 |

Findings by dimension: security 39, reliability 40, correctness 12, testGap 8.
Security breaks down 17 authorization / 13 injection / 9 exposure, and the top
hits are the known Juice Shop vulnerability classes — `routes/checkKeys.ts`
exposure (2.74), `routes/search.ts` injection (2.67),
`routes/profileImageUrlUpload.ts` injection (2.64), `routes/login.ts`
injection (2.60), `routes/orderHistory.ts` authorization (2.60).

Conditions: Apple-silicon macOS, `jev-latest` model, concurrency 3 (the
policy default), end-to-end wall time ~46 s. That time is dominated by Jev API
latency — 357 followed signals, each up to four `system_one` calls, on top of
the screen pass — not local computation.

Implementation behind these numbers: the staged pipeline is
[`src/review/workflow.rs`](src/review/workflow.rs) (screen → rank → profile →
locate); judgments in
[`src/review/codebase_judgments.rs`](src/review/codebase_judgments.rs) and
[`src/review/judgments.rs`](src/review/judgments.rs); thresholds and budgets
in [`src/domain/policy.rs`](src/domain/policy.rs). Full judgment graph:
[`docs/jev-pipeline.md`](docs/jev-pipeline.md).

## Non-Goals (for now)

- Not a prose explainer: findings are structured (evidence, mechanism,
  severity, owner) before they are narrated
- Not a scanner replacement: compilers and linters own facts; Momus judges impact
- Not hosted: local-first, loopback dashboard, your API key, your code stays yours
  except for the Jev API calls you explicitly make (see `docs/security.md`)
