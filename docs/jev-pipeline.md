# Jev Judgment Pipeline

The funnel, as proven in `src/review/`:

```mermaid
flowchart TD
    D[discover files] --> S[screen: noul per dimension<br/>security = 3 nouls, max-merged]
    S -->|p >= 0.7, all signals<br/>(--follow-ups N opts into a cap)| L[locate: choice evidence hunk/region]
    S -->|top 5 by max p| P[profile: choice role + score priority]
    L -->|confidence >= 0.55| M[choice mechanism]
    M -->|not noIssue| V[score severity 0-3]
    V --> J[meta-judge: noul probability >= 0.55 else drop]
    J -->|severity >= 1.5| R[choice owner]
    J -->|severity >= 2.0| A[request_changes else comment]
    R & V -->|top 8 by severity| E[enrich: title/why from mechanism<br/>choice fix + choice test]
```

## Stages

1. **Screen** (`screenFile` / `screenSourceFile`): one `noul` per dimension
   (correctness, security, reliability, compatibility, testGap). The security
   dimension is split into three `noul`s — authz/injection/exposure,
   crypto/secrets, and misconfiguration — folded back by max-merge (see
   `docs/security-taxonomy.md`). Each carries `inspect`/`focus`/`ignore` hints
   plus true/false criteria with examples and counterexamples. Test context
   rides along for testGap (`changedTests` in changes mode;
   `selectRelatedTests` + `compactTest` in codebase mode). Changes mode also
   sends `file.base` alongside `file.patch` (correctness compares the two);
   codebase mode attaches 1-hop callers/callees (`neighbors`) resolved via the
   heuristic import graph in `adapters/imports.rs`. Output:
   `Record<Dimension, number>`.
2. **Profile** (top 5 by max probability): `choice` file role
   (`changeTypes` for diffs, `fileRoles` for sources) + `score` review
   priority on `reviewPriorityRubric`. Cheap triage aid, not gating.
3. **Locate** (every signal ≥ 0.7; unlimited by default, `--follow-ups N`
   re-imposes a per-dimension budget): `choice` strongest-evidence hunk
   (`parseHunks`, 80-line chunks for new files) or function-aware source
   region (`regions::function_regions`, a tree-sitter-free heuristic splitting
   at column-0 top-level declarations; oversized declarations and
   declaration-free files fall back to uniform windows; screening uses the
   same regions, max-merged). `noMatch` fallback
   + `MIN_LOCATION_CONFIDENCE=0.55` kill weak attributions.
4. **Mechanism**: `choice` over per-dimension `mechanisms` vocabulary.
   `noIssue` kills the finding — the first precision gate.
5. **Severity**: `score` on `severityRubric` (0 none → 3 critical).
6. **Meta-judge** (`meta::judge`): a second, independent skeptical `noul`
   ("does selectedEvidence concretely support this mechanism?"). Below
   `MIN_META_JUDGE_CONFIDENCE` the finding is dropped as an unsupported claim
   — the FP filter.
7. **Route** (severity ≥ 1.5): `choice` over `owners`
   (security/api/runtime/testing/maintainer). Action derives from severity:
   ≥ 2.0 → `request_changes`, else `comment`.
8. **Enrich** (top 8 located findings by severity): `title` / `why` are
   derived deterministically from the classified mechanism; `fix` / `test`
   come from one narrow `choice` call each (`suggestedFix`, `suggestedTest`)
   over curated strategy vocabularies. Jev offers no free-text generation
   (only `noul`/`choice`/`score`), so "generation" is expressed as a
   `choice` whose selected label's description is the suggestion.

## Policy Lives in Code

`src/domain/policy.rs`: `SCREEN_THRESHOLD`, `SEVERITY_MAX`, `ROUTE_SEVERITY`,
`BLOCKING_SEVERITY`, `MIN_LOCATION_CONFIDENCE`, `MAX_PROFILES`, `CONCURRENCY`,
plus `dimensions`, `mechanisms`, rubrics, `owners`, and file patterns. The
model never sets budgets or thresholds; follow-ups are unlimited by default
and `--follow-ups N` re-imposes a cap in code.

## Report Shape

`ReviewReport` (`domain/report.rs`): mode, scope, dimensions, config snapshot,
`screenedFiles`, `contextFiles`, full probability `matrix`, `profiles`,
workflow funnel counts, and `findings` (file, line, dimension, mechanism +
confidences, severity + confidence, owner + confidence, action, the
`evidence` excerpt, and generated `title` / `why` / `fix` / `test`).

The dashboard renders each finding's evidence excerpt plus its generated
`title`, `why`, `fix`, and `test`, with filter/sort/search, per-file focus,
and copy-as-PR-comment.

The same report is also emitted as **SARIF 2.1.0** (`--sarif <path>`, for
GitHub code scanning) and archived per-revision under
`reviews/history/<sha>.json`; the dashboard's History view aggregates those
snapshots into risk-over-time, hotspots, and fix-latency (open vs. resolved).

## Cost Model

Per file: 1 screen call (5–7 questions batched — security is 3 sub-`noul`s).
Per review: +5 profiles max, +1 locate chain per followed signal (unlimited by
default; each up to 4 calls: evidence, mechanism, severity, owner), +1 enrich
call per finding up to the top 8 by severity. Codebase mode multiplies
screening by region count (function-aware regions). No caching yet —
re-scans resend everything. Budgets are static globals; per-dimension
thresholds and value-of-information selection are roadmap items.
