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

- Cost control: screen everything cheaply, spend follow-up budget (top 8) wisely
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

## Non-Goals (for now)

- Not a prose explainer: findings are structured (evidence, mechanism,
  severity, owner) before they are narrated
- Not a scanner replacement: compilers and linters own facts; Momus judges impact
- Not hosted: local-first, loopback dashboard, your API key, your code stays yours
  except for the Jev API calls you explicitly make (see `docs/security.md`)
