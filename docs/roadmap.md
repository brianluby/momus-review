# Roadmap

## Where We Are

Strong core loop (cheap screening → focused follow-ups → quiet dashboard),
proven on TS and Rust. Stops at triage signals where users need actionable
findings in their workflow. Biggest ROI is not more dimensions — it's evidence
excerpts, explanations, and meeting reviewers where they work (PRs, CI gates).

## Progress (2026-09-25)

Completed ahead of / from this plan:

- **Rust port shipped** — `review`/`scan`/`dashboard`, a thin `system_one`
  client, hardening, MIT + public repo.
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

Still open from "Now": the full dashboard workbench
(filter/sort/search/file-view/copy-as-PR-comment), SARIF + history trend, and
the P(revert) spike.

## Now (1–2 weeks): make findings actionable

- ✅ Evidence excerpts + generated title / why / suggested fix / suggested
  test per finding (one narrow Jev call each, top 8 by severity).
- Dashboard becomes a workbench: excerpt + why + fix, filter/sort/search,
  file view, copy-as-PR-comment. Stop rendering raw mechanism keys.
- CI contract: `--fail-on-blocking` ✅; SARIF output + `reviews/history/<sha>.json`
  trend (risk over time, hotspots, fix latency) — open.
- ✅ README note for FIND-005 (code is uploaded to the Jev API by design).
- Spike: `P(revert)` / merge-confidence signal per PR.

## Next (2–4 weeks): kill noise, add context

- Meta-judge FP filter: skeptical second Jev pass
  ("does this evidence concretely support the claim?"); disagreement with the
  first pass is the uncertainty signal; kill below bar.
- Base + neighbor context: changes mode sends base file + diff; codebase mode
  attaches 1-hop callers/callees via import graph.
- Per-language mechanisms (Rust `unsafe`/`unwrap`/`panic!`, TS `any`/casts…).
- Finding feedback (thumbs up/down) → per-repo threshold auto-tune +
  suppression list.
- ✅ Function-aware regions (tree-sitter-free heuristic) replacing 80-line
  slicing.
- Dedupe/cluster: Jev `choice` "same root cause?" over adjacent candidates.

## Later (bets)

- Decision-theoretic budgets: value-of-information follow-up selection
  ("which 8 would most change the merge decision?") instead of top-8 by
  probability; per-dimension thresholds; cost/latency meter; result caching.
- Pairwise severity ranking (`choice` A-vs-B + Bradley-Terry) for stable
  "top 3 to fix".
- Multi-hop taint chains via composed narrow calls
  (`choice` source → sanitized? → sink?).
- Counterfactual calibration: "what single fact would exonerate this?" then check.
- Ensembles on demand: re-ask high-stakes screens with varied focus; route to
  human on disagreement.
- GitHub Action with inline comments; secret redaction pre-send.
- ✅ Golden-set eval harness (shipped: `momus-eval` + curated Juice Shop
  ground truth) — precision/recall on labeled vulns.

## Beyond Review (Jev as judgment fabric)

- Merge-confidence engine: `P(revert)`/`P(incident)`/`P(flake)` per PR from
  signals + history; auto-approve routine + low-risk.
- Test planner: `choice` over strategies per gap → emit test stub.
- Spec drift: `compare([code, ticket/PRD])` — correct code, wrong behavior.
- Upgrade/changelog triage: screen dependency diffs for breaking-risk.
- Onboarding tours: extend file-role classification to repo tours + arch maps.
- Docs drift: code-vs-docs `noul` on doc-touched PRs.

## Metrics

- `precision@8` (lived findings / surfaced), action rate (% findings producing
  a fix/comment), time-to-first-action, FP suppression rate, cost-per-review,
  escaped-defect rate on reviewed vs unreviewed PRs.

## Open Question

Showcase (optimize for Jev-pattern novelty) or reviewer-replacement product
(optimize for precision + workflow)? The Now list serves both; Next/Later
diverges.
