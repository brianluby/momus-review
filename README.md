# Momus Review

Finds fault in the gods' own work — so it can find fault in yours.

Momus (Μῶμος) is the Greek god of satire, mockery, and criticism. Legend has
it he criticized Zeus's own creations. `momus` is a code/security review
product that does the same to your diffs: fast, calibrated, staged judgments
with concrete evidence, severity, and owner routing.

- Crate: `momus-review` (free on crates.io as of 2026-09-25)
- Binary: `momus`
- Engine: [TypeSafe Jev](https://typesafe.ai), through a thin HTTP client
  speaking the `system_one` wire format directly (see `docs/rust-types.md`)
- Status: the core funnel (`review`/`scan`/`dashboard`) shipped in Rust and
  validated on OWASP Juice Shop through a golden-set eval harness
  (`momus-eval`), with evidence excerpts, an OWASP-aligned security mechanism
  vocabulary, and crypto/misconfig screen steering; pull requests get inline
  review comments through a GitHub Action (see "CI")

## What It Does

Two modes, one funnel:

- `momus review` — review the current Git diff (tracked changes + untracked files)
- `momus scan` — scan every non-ignored source file under a scope

plus `momus github-review` to publish a review to its pull request (see
"CI") and `momus dashboard` to browse the latest report locally.

Both accept one or more scope directories (unioned into one run) plus
`--exclude GLOB` (skip vendored/third-party subtrees), `--follow-ups N`
(opt into a follow-up budget; unlimited by default),
`--fail-on-blocking` (CI exit contract), `--no-refine` (skip the
refinement stage below), and `--no-redact` (send code without secret
redaction; see below).

`momus review --base REV` diffs against the merge base of `REV` and `HEAD`
instead of `HEAD`: the branch's commits plus uncommitted changes to tracked
files (untracked files are ignored). In CI, run it on a clean checkout of
the pull request with `--base origin/main` and the reviewed diff is exactly
the PR's diff. The checkout needs enough history to find the merge base,
e.g. `actions/checkout` with `fetch-depth: 0`. Each finding points at the
first changed line of its hunk.

`momus github-review` then publishes the saved report to the pull request
from inside GitHub Actions. It reads `GITHUB_TOKEN` (needs
`pull-requests: write`), `GITHUB_REPOSITORY`, and the `pull_request` event
at `GITHUB_EVENT_PATH`. Findings on a line of the PR's diff become inline
review comments, at most `--max-comments N` (default 10), best-ranked first;
everything else goes into one summary comment with counts, P(revert), usage,
and redaction totals. Re-runs are idempotent: each comment carries a hidden
fingerprint marker, so a finding is posted once, and the summary is edited in
place. The review is a plain comment by default and blocking is left to the
check status (`--fail-on-blocking`); `--event request-changes` requests
changes when an inline finding blocks. Published text is redacted like
outgoing requests. `--dry-run` reads the pull request and prints what would
be posted. Check out the PR head (`ref: ${{ github.event.pull_request.head.sha }}`),
not the default merge commit, so line numbers match the PR's diff; if GitHub
still rejects an anchor, those findings move to the summary.

Source languages: the 20-language consensus set (Python, JavaScript,
TypeScript, Java, C#, C++, C, Go, Rust, PHP, Ruby, Kotlin, Swift, shell, SQL,
R, Scala, Dart, Lua, PowerShell), each with its own mechanism vocabulary — see
`docs/language-support.md`.

Both run the same staged pipeline: cheap risk screening across five
dimensions (correctness, security, reliability, compatibility, test gap),
then focused follow-ups on the strongest signals only — evidence selection,
mechanism classification, severity scoring, reviewer routing. A refinement
stage then folds duplicate findings, traces taint for injection classes,
drops findings a single visible fact exonerates, flags split re-screens for a
human, and ranks the top findings pairwise. Results land in a quiet local
dashboard and machine-readable JSON; 👍/👎/Hide in the dashboard tunes
per-dimension thresholds and suppresses findings on the next run.

## Configuration

Read from the environment (or `.env`):

| variable | default | purpose |
|---|---|---|
| `TYPESAFE_API_KEY` | — | API key; required unless the base URL is a local server |
| `TYPESAFE_BASE_URL` | `https://api.typesafe.ai` | any server speaking `POST /v1/systemone` |
| `TYPESAFE_DEFAULT_MODEL` | `jev-latest` | model id sent with each request |
| `TYPESAFE_TIMEOUT_SECS` | `60` | per-request timeout |
| `MOMUS_CONCURRENCY` | `3` | parallel `system_one` requests |
| `MOMUS_REDACT` | `on` | redact secrets before sending (`off` or `--no-redact` disables) |

To run against a local System One server such as
[Winnow-12B](https://github.com/EldanRing/winnow-inference), point the base
URL at it. Winnow accepts the default `jev-latest` as an input alias (the
response identifies the loaded model), so no model override is needed;
`TYPESAFE_DEFAULT_MODEL=Winnow-12B` is the explicit spelling of the id the
server advertises, and other servers may require it. No key is needed for
`localhost`/loopback addresses. A local server may decide one request at a
time, so concurrent requests queue behind it — lower the concurrency and
raise the timeout if you see that:

```bash
TYPESAFE_BASE_URL=http://127.0.0.1:8091 TYPESAFE_DEFAULT_MODEL=Winnow-12B \
  MOMUS_CONCURRENCY=1 TYPESAFE_TIMEOUT_SECS=300 momus review
```

Screening thresholds in `src/domain/policy.rs` were tuned against hosted Jev
and may need recalibrating for another model.

## CI

The GitHub Action reviews each pull request's diff and posts the findings as
inline review comments plus one summary comment:

```yaml
name: momus
on:
  pull_request:

permissions:
  contents: read
  pull-requests: write

jobs:
  review:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          ref: ${{ github.event.pull_request.head.sha }} # the PR head, not the merge commit
          fetch-depth: 0                                 # history for the merge base
          persist-credentials: false
      - uses: brianluby/momus-review@v0.1.0
        with:
          api-key: ${{ secrets.TYPESAFE_API_KEY }}
```

It downloads the prebuilt `momus` for the runner (Linux x64/arm64, macOS
arm64) and checks its SHA-256, runs `momus review --base <PR base sha>`, and
then `momus github-review`. Inputs:

| input | default | purpose |
|---|---|---|
| `api-key` | — | TypeSafe API key; when empty (and no `base-url`) the review is skipped with a notice |
| `base-url` / `model` | API defaults | `TYPESAFE_BASE_URL` / `TYPESAFE_DEFAULT_MODEL` |
| `paths` | `.` | scope directories, whitespace-separated |
| `exclude` | — | gitignore-style globs, one per line |
| `fail-on-blocking` | `true` | fail the check when a finding requests changes (after posting) |
| `max-comments` | `10` | new inline comments per run; the rest go in the summary |
| `sarif` | `false` | also upload SARIF to code scanning (add `security-events: write`) |
| `version` | `latest` | a release tag, `latest`, or `source` to build the action's own checkout |
| `github-token` | `github.token` | token for reading the PR and posting |

Fork pull requests are skipped (secrets are not available to them), and
`pull_request_target` is not supported; see `docs/security.md` for why.
Re-runs never repeat a comment: findings already posted are skipped and the
summary is edited in place. Other CI systems can run
`momus review --base origin/main --fail-on-blocking` and use its exit code
and `--sarif` output directly.

## Why Jev, Why This Shape

Jev is a decision model: typed questions (`noul`/`choice`/`score`) against a
state, with calibrated probabilities — fast enough to sit inside code paths.
Momus keeps orchestration in code (thresholds, budgets, ranking) and uses Jev
only for bounded judgments. That gives:

- Cost control: screen everything cheaply, follow up every threshold signal
  (unlimited by default, cap via `--follow-ups N`), so no finding is silently
  dropped by an arbitrary budget; every report carries per-run `usage`
  (call count, plus token totals when the server reports them)
- Calibration: probabilities mean something, so thresholds and routing work
- Composability: narrow calls chain into taint analysis, meta-judgment,
  pairwise ranking — things monolithic prompts fumble

See `docs/jev-pipeline.md` for the full judgment graph.

## Design Lineage

Momus is an independent Rust implementation. Its design draws on ideas from
[`devagrawal09/jev-review`](https://github.com/devagrawal09/jev-review): a
staged funnel of cheap Jev screens followed by focused follow-ups, a diff
mode and a codebase mode, and a local dashboard. No code was taken from that
project.

The security issues found while reviewing that design (symlink-safe reads,
atomic report writes, dashboard Host/CSP hardening) were reported and fixed
upstream in [PR #10](https://github.com/devagrawal09/jev-review/pull/10), and
momus implements the same protections itself (`docs/security.md`).

See `docs/architecture.md` (what exists), `docs/security.md` (threat model +
fixes), `docs/implementation.md` (build decisions).

## Docs

- `docs/architecture.md` — module layout, layer rules, key invariants
- `docs/jev-pipeline.md` — judgment graph, prompts-as-policy, budgets
- `docs/language-support.md` — how file discovery and test context work, adding languages
- `docs/security.md` — threat model, FIND-001–005, CI (GitHub Action) security
- `docs/roadmap.md` — Now/Next/Later, metrics, open bets
- `docs/implementation.md` — build decisions (thin client vs. `jev_sdk`), pending upgrades
- `docs/rust-types.md` — the shipped Rust types/traits + the `jev_sdk` finding
- `docs/security-taxonomy.md` — code-findable security classes vs. screen coverage, and the steering decisions
- `docs/distribution.md` — install, prebuilt binaries, the GitHub Action, releasing

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
  except for the Jev API calls you explicitly make (see `docs/security.md`).
  Secrets are redacted from each request's `state` (the code and context
  momus sends; the question text is momus's own and goes unchanged): API keys and tokens (AWS,
  GitHub, GitLab, Slack, Stripe, Google, Anthropic, OpenAI), JWTs, PEM private
  keys, URL credentials, quoted values assigned to secret-looking names, and
  long random-looking literals become typed placeholders such as
  `<redacted:aws-access-key>`, so hardcoded-secret findings still surface.
  Each report's `redactions` field counts what was hidden per rule.
