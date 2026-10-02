# Roadmap

## Where We Are

Strong core loop (cheap screening → focused follow-ups → quiet dashboard),
and findings now reach reviewers where they work: inline PR comments from a
GitHub Action. Biggest ROI is not more dimensions — it's precision,
calibration, and staying in the reviewer's workflow.

## Release milestones

- **0.9 beta — assembled capabilities:** supported repository docs/dependency
  checks, local tours, separate merge-outcome tooling, the Momus-owned offline
  benchmark suite, native receipt/scorer acceptance fixes and executable Rust
  API contracts. Use findings as advisory evidence with manual source review;
  automatic approval stays disabled by default. This milestone does not claim
  product model accuracy, full language coverage or real-history calibration.
  See [v0.9.0 notes](releases/v0.9.0.md).
- **1.0 — finish non-Jeeves work and stabilize supported contracts:** complete
  the remaining non-Jeeves tickets and verify the supported CLI, report,
  evidence and failure behavior. #17/#51 still require independently sourced
  external outcomes, mature observation windows and chronological held-out
  acceptance; collection machinery and synthetic fixtures do not close that
  gate. #63's comparative evaluation also remains open.
- **1.1 — Jeeves while preserving 1.0 contracts:** pursue the deferred Jeeves
  work (#53/#55/#56/#57) without weakening established CLI/report compatibility,
  source evidence, uncertainty, privacy or approval boundaries. Benchmark
  holdouts remain evaluation-only rather than tuning or training inputs.

## Progress (2026-09-30)

Completed ahead of / from this plan:

- **Rust implementation shipped** — `review`/`scan`/`dashboard`, a thin
  `system_one` client, hardening, MIT + public repo.
- **Golden-set eval harness** (`momus-eval`, from "Later") — category coverage
  + curated known-vulnerable corroboration, run against Juice Shop.
- **Follow-up budget fixed** — unlimited by default (the old top-8 starved real
  findings); `--follow-ups N` opts into a per-dimension cap.
- **Security taxonomy** — OWASP-aligned mechanism vocabulary + crypto/misconfig
  screen steering. See `docs/security-taxonomy.md`.
- **Evidence excerpt + generated title/why/fix/test** — each finding carries
  the selected hunk/region, a human-readable title + why (from the classified
  mechanism), and model-chosen fix/test suggestions (one narrow Jev call each,
  top 8 by severity). Function-aware region splitting (tree-sitter-free
  heuristic) replaces 80-line slicing.
- **Dashboard workbench + CI outputs** — findings are filterable/sortable/
  searchable with per-file focus and copy-as-PR-comment; SARIF 2.1.0 emission
  (`--sarif`) and per-sha `reviews/history/<sha>.json` trend (risk over time,
  hotspots, fix latency) feed CI and the dashboard History view.
- **Noise + context (Now complete; Next underway)** — a meta-judge skeptic
  pass drops unsupported findings; changes mode sends the base file and
  codebase mode attaches 1-hop callers/callees; a spiked `P(revert)` heuristic
  drives a merge-confidence stat.

## Now (1–2 weeks): make findings actionable

- ✅ Evidence excerpts + generated title / why / suggested fix / suggested
  test per finding (one narrow Jev call each, top 8 by severity).
- ✅ Dashboard becomes a workbench: excerpt + why + fix, filter/sort/search,
  file view (group-by-file + per-file focus), copy-as-PR-comment. Raw
  mechanism keys are replaced by generated titles.
- ✅ CI contract: `--fail-on-blocking`; SARIF 2.1.0 output (`--sarif <path>`)
  + `reviews/history/<sha>.json` trend (risk over time, hotspots, fix
  latency).
- ✅ README note for FIND-005 (code is uploaded to the Jev API by design).
- ✅ Spike: `P(revert)` signal per PR — an uncalibrated heuristic
  (`review/merge_confidence.rs`) now drives a `pRevert` field + dashboard
  stat; real calibration is the merge-confidence engine (#17 / Beyond Review).

Now is complete.

## Next (2–4 weeks): kill noise, add context

- ✅ Meta-judge FP filter: a second skeptical Jev pass ("does this evidence
  concretely support the claim?"); below `MIN_META_JUDGE_CONFIDENCE` the
  finding is dropped.
- ✅ Base + neighbor context: changes mode sends base file + diff; codebase
  mode attaches 1-hop callers/callees via a heuristic import graph
  (`adapters/imports.rs`).
- ✅ Per-language mechanisms: the 20-language consensus set is discovered,
   its test conventions and test-body markers are language-aware, and each
   language's own footguns (Rust `unsafe`/`unwrap`/`panic!`, TS `any`/casts,
   Go ignored `error`, C memory safety, shell word splitting, …) join the
   mechanism `choice` vocabulary. See `docs/language-support.md`.
- ✅ Finding feedback (thumbs up/down) → per-repo threshold auto-tune +
  suppression list (`reviews/feedback.json`, `domain/feedback.rs`).
- ✅ Function-aware regions (tree-sitter-free heuristic) replacing 80-line
  slicing.
- ✅ Dedupe/cluster: Jev `choice` "same root cause?" over adjacent candidates
  (`review/refine/dedupe.rs`).

## Later (bets)

- Whole-repo scale (epic #31): bounded per-request context, a
  content-addressed result cache (incremental, resumable), a repo index,
  sharded scans with `momus merge`, tiered screening under a budget. Design
  in `docs/scaling.md`.
- Decision-theoretic budgets: value-of-information follow-up selection
  ("which 8 would most change the merge decision?") instead of top-8 by
  probability; per-dimension thresholds; cost/latency meter; result caching.
  Implemented as opt-in heuristic priorities and explicit threshold overrides
  (#11); calibration remains future work. See [Review robustness](review-robustness.md).
- ✅ Pairwise severity ranking (`choice` A-vs-B + Bradley-Terry) for stable
  "top 3 to fix".
- ✅ Multi-hop taint chains via composed narrow calls
  (`choice` source → sanitized? → sink?).
- ✅ Counterfactual calibration: "what single fact would exonerate this?" then check.
- ✅ Ensembles on demand: re-ask high-stakes screens with varied focus; route to
  human on disagreement.

The five refinement judgments run as one post-locate stage
(`review/refine`, `--no-refine` to skip); see `docs/jev-pipeline.md`.
- ✅ Secret redaction pre-send (#25): typed placeholders, per-rule counts in
  the report, `--no-redact` to opt out.
- ✅ GitHub Action with inline comments: `momus review --base` (#26), the
  `momus github-review` publisher (#27), and the composite action + prebuilt
  release binaries (#28). See README "CI".
- ✅ Golden-set eval harness (shipped: `momus-eval` + curated Juice Shop
  ground truth) — security-category coverage and curated lower-bound corroboration.
- ✅ Offline Momus-owned benchmark suite (#54): 13 authored core defect/fixed
  pairs, two docs/dependency pairs, four insufficient-evidence cases and two
  exposed PR #40 development controls; exact source identities, independently
  reviewed annotations, offline behavioral probes and issue-level scoring.
  [Product evaluation](../eval/momus/README.md) remains separately scheduled;
  its results template is blank.

## Beyond Review (Jev as judgment fabric)

- Merge-confidence engine (#17): implemented separate outcome history, chronological held-out evaluation, explicit unknowns and opt-in fail-closed approval. Implementation child #52; real repository outcome/calibration acceptance remains open in #51. Synthetic fixtures establish machinery only. See [merge confidence](merge-confidence.md).
- Prospective outcome collection (#60): frozen trusted premerge receipts, exact
  merged-head joins, separately sourced observations and availability-aware
  history export are implemented offline. [Source inventory and collection protocol](outcome-collection.md)
  record candidate archived reports and the missing mature independent telemetry;
  #17/#51 remain open, with no real-data calibration acceptance.
- Test planner (#18): opt-in `choice` over strategies per gap → unfinished test
  scaffold and observable assertion guidance; no source writes or execution.
- Spec drift (#19): opt-in comparison with supplied local requirements, exact
  evidence and skeptical confirmation; advisory, not full requirement coverage.
- ✅ Upgrade/changelog triage (#20): opt-in Cargo/npm manifest/lock comparisons with matched local changelogs, evidence advisories and explicit unsupported/missing-evidence unknowns. See [upgrade triage](upgrades.md).
- ✅ Onboarding tours (#21): local bounded/redacted index-backed tours, role/path components and corroborated unique static import maps. Unsupported relationships remain unknown. See [tours](tours.md).
- ✅ Docs drift (#22): opt-in deterministic supported Rust public-interface/example comparisons, current/base source and documentation references, standard finding/suppression flow. Broader semantic and other-ecosystem checks remain explicit unknowns. See [docs drift](docs-drift.md).

## Metrics

- `precision@8` (lived findings / surfaced), action rate (% findings producing
  a fix/comment), time-to-first-action, FP suppression rate, cost-per-review,
  escaped-defect rate on reviewed vs unreviewed PRs.

## Open Question

Showcase (optimize for Jev-pattern novelty) or reviewer-replacement product
(optimize for precision + workflow)? The Now list serves both; Next/Later
diverges.

Epic #48 release verification is complete. SLSA L3 remains deferred.
