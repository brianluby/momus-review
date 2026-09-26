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
  core funnel (`review`/`scan`/`dashboard`) shipped and validated on OWASP
  Juice Shop through a golden-set eval harness (`momus-eval`), with evidence
  excerpts, an OWASP-aligned security mechanism vocabulary, and crypto/
  misconfig screen steering

## What It Does

Two modes, one funnel:

- `momus review` — review the current Git diff (tracked changes + untracked files)
- `momus scan` — scan every non-ignored source file under a scope

Both accept one or more scope directories (unioned into one run) plus
`--exclude GLOB` (skip vendored/third-party subtrees), `--follow-ups N`
(opt into a follow-up budget; unlimited by default), and
`--fail-on-blocking` (CI exit contract).

Source languages: the 20-language consensus set (Python, JavaScript,
TypeScript, Java, C#, C++, C, Go, Rust, PHP, Ruby, Kotlin, Swift, shell, SQL,
R, Scala, Dart, Lua, PowerShell), each with its own mechanism vocabulary — see
`docs/language-support.md`.

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
- Prototype also proved Rust language support (`.rs` discovery + test markers);
  the Rust port now carries the full 20-language consensus set

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
- `docs/security-taxonomy.md` — code-findable security classes vs. screen coverage, and the steering decisions
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
| located findings | 106 (90 routed to an owner) |
| `request_changes` / `comment` | 31 / 75 |

Findings by dimension: security 38, reliability 41, correctness 16, testGap 9,
compatibility 2. Security now classifies with the OWASP-aligned vocabulary —
`brokenAccessControl` 14, `pathTraversal` 7, `brokenAuthentication` 3,
`noSqlInjection` 3, `sqlInjection` 2, `xss` 2, `sensitiveDataExposure` 2, plus
`xxe`, `ssrf`, and `cryptographicFailure` — and the top hits map to the known
Juice Shop classes: `routes/checkKeys.ts` `sensitiveDataExposure` (2.81),
`routes/search.ts` `sqlInjection` (2.68),
`routes/profileImageUrlUpload.ts` `ssrf` (2.66), `routes/login.ts`
`sqlInjection` (2.60), `routes/orderHistory.ts` `brokenAccessControl` (2.59).

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
