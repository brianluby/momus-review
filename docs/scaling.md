# Scaling: whole-repo reviews

Goal: `momus` can review an entire large repository, not just a pull
request or a Juice Shop-sized codebase, at a cost and wall time that grow
with what changed rather than with the repo, without one bad file sinking
the run.

## Where we are

- `momus scan` already bounds its context: at most 4 related tests per file
  (`MAX_RELATED_TESTS`), each trimmed to 1,800 characters, 1-hop neighbors
  trimmed to 40 lines, and files split into function-aware regions. It
  scanned all of Juice Shop (299 files, 1,495 screen cells, 357 followed
  signals) in about 46 seconds (README "Validation").
- `momus review` (diff mode) does not. `screen_file` sends **every** changed
  test file in full with each screen. On a pull request that adds all of
  Juice Shop (300 source files, 253 test files) that is about 2 MB, roughly
  490k tokens, per request, and every screen failed with
  `400 max_tokens_exceeded`
  ([example PR](https://github.com/brianluby/momus-juice-shop-test/pull/1)).
- A single failed screen aborts the whole run, and only after every screen
  has been sent: `run_review` collects all results before surfacing the
  first error (`review/workflow.rs`).
- Everything runs in one process at a fixed concurrency (3, or
  `MOMUS_CONCURRENCY`), with no caching: a re-scan resends every request.
- Each Jev `system_one` call is stateless, so context cannot be "stored" in
  the model between calls; it has to be rebuilt, compactly, per request.

## Design: a review is a build

Treat a review like a build system (ccache, Bazel) instead of one long
pipeline: small units of work, content-addressed results, incremental
re-runs, and shards that merge.

### 1. Bounded context packs

Every request's state is assembled from one **context budget** (a character
cap per request, sized from the model's measured input limit). Sources are
added in priority order, each trimmed to its own share: the unit under
review, then its base (diff mode), related tests, neighbors, export
signatures. Whatever does not fit is dropped and counted, never sent.

Diff mode uses the same related-test selection as scan mode. This closes
the bug class, not just the test-file instance: no single context source
can grow a request without limit again.

### 2. Work units and a result cache

A **unit** is one request: a screen of one (file, region), or one follow-up
call (evidence, mechanism, severity, meta-judge, owner, refinement). Its
**key** is a hash of everything that determines the answer:

```
key = sha256(base_url, model, questions JSON, redacted state JSON)
```

The key covers the policy (the question text), the code actually sent (after
redaction, so the setting is part of the key), and the model. Results live in
a local cache (`reviews/cache/`; SQLite or one file per key). It stores keys
and answers only, never code, so it is not sensitive the way the report is.

- A re-scan re-sends only units whose inputs changed; a large repo costs
  about its diff to re-review.
- The cache doubles as a checkpoint: an interrupted scan resumes.
- In CI, `actions/cache` keyed on the base branch warms a PR's run from
  `main`'s scan.
- Diff mode and scan mode keep their own question sets but share the unit,
  budget, and cache machinery.

### 3. Repo index

Context is precomputed once per scan instead of per request. A pre-pass
builds an index of the tree:

- regions per file (`review/regions.rs`),
- the import graph (`adapters/imports.rs`),
- the source-to-test mapping (today's `select_related_tests`),
- short export signatures per file (declaration lines, no bodies).

Context packs (1) draw from the index. The index is local and keyed by blob
id, so it too is incremental.

### 4. Shards and merge

A planner writes the unit list; `momus scan --shard i/N` runs one slice,
assigned by a stable hash of the file path (so each shard's cache stays
warm), and writes a partial report. `momus merge` combines the partial
reports and runs the stages that need the whole picture: cross-file dedupe,
pairwise ranking, `pRevert`, history, SARIF, and `github-review`. In CI that
is an Actions matrix of N scan jobs followed by one merge job.

Per-finding refinement (taint, counterfactual, ensemble) can run in the
shard; their top-K caps become per-shard, which changes semantics slightly
and needs an eval check.

### 5. Tiered screening under a budget

- **Tier 0**: one cheap question per file ("any concern worth a closer
  look?"). Only files it flags get the full per-dimension, per-region
  screens. Adopt only if `momus-eval` shows no recall loss on Juice Shop.
- **Priority order**: file role and review priority, churn (`git log`), and
  past hotspots (`reviews/history`) order the unit list.
- **`--budget calls=N`**: spend the budget on the highest-priority units
  first; the rest stay queued and resume from the cache later. This is the
  value-of-information work in #11.

### 6. Failure isolation and throughput

- A unit that fails with a non-retryable 4xx is skipped, recorded in the
  report (`skippedFiles` with the reason), and listed in the summary comment.
- A circuit breaker aborts early when the first units all fail, instead of
  sending every request.
- Concurrency adapts: back off on `429`, ramp up while responses are
  healthy, capped by a configured maximum.

## Plan

Epic #31 in the project tracker (Vikunja, not GitHub); each step is a ticket.

| step | ticket | work | depends on |
|---|---|---|---|
| 0 | #32 | Measure: model input limit, TypeSafe rate limits, answer stability on repeated identical requests | — |
| 1 | #33 | Bounded context packs: diff-mode related tests + one per-request budget | 0 |
| 2 | #34 | Failure isolation: skip and record failed units, circuit breaker | — |
| 3 | #35 | Release v0.1.1; re-run the Juice Shop example PR | 1, 2 |
| 4 | #36 | Work units + content-addressed result cache (incremental, resumable, CI cache) | 0, 1 |
| 5 | #37 | Repo index feeding the context packs | 1 |
| 6 | #38 | Shards + `momus merge` + Actions matrix | 4 |
| 7 | #39 | Tiered screening, priority order, `--budget` (#11) | 4 |
| 8 | #40 | Adaptive concurrency (429-aware) | 0 |

Steps 1–3 unblock the Juice Shop example; 4 is the step that makes large
repos practical.

## Open questions

- **Input limit.** We only know it is below ~490k tokens; the budget in (1)
  should be derived from a measured value, with margin.
- **Rate limits.** They bound useful parallelism and so the shard count.
- **Answer stability.** Caching assumes identical input gives an equivalent
  answer. Jev is calibrated, but repeated identical requests should be
  measured before cached results replace fresh ones.
- **One question set or two.** Diff mode asks "does this change introduce…",
  scan mode "does this code have…". Sharing machinery is clear; whether a PR
  review should also use scan questions on changed regions is not.
- **Cache invalidation on policy change.** The questions JSON is in the key,
  so any wording change invalidates every result. That is correct, but a
  policy tweak then costs a full re-scan.
