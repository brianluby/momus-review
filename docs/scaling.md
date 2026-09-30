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
- `momus review` (diff mode) did not, until #33 (v0.1.3): `screen_file` sent
  **every** changed test file in full with each screen — on a pull request
  that adds all of Juice Shop, ~2 MB (~490k tokens) per request, and every
  screen failed `400 max_tokens_exceeded`
  ([the old run](https://github.com/brianluby/momus-juice-shop-test/pull/1)).
  Both modes now select at most 4 related tests and cap every request's state
  at 96k characters (§1); the same PR reviews end to end — 120 findings,
  1,196 Jev calls, ~52 s of review on a hosted runner
  ([PR #3](https://github.com/brianluby/momus-juice-shop-test/pull/3)).
- A single failed screen used to abort the whole run, and only after every
  screen had been sent; #34 added failure isolation — failed units are
  skipped and recorded, with a circuit breaker (`review/workflow.rs`).
- HTTP requests share an adaptive gate (starts at 3, caps at 16 or
  `MOMUS_CONCURRENCY`), with a local content-addressed answer cache: unchanged requests reuse results.
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
a local cache (`reviews/cache/`; SQLite or one file per key). It stores model IDs and answers only, never request code or question text.
Answers may still contain sensitive data; keep the directory local and out of Git.
`MOMUS_CACHE_DIR` overrides the directory; `--no-cache` bypasses reads and writes
in both modes. `usage.cache.hits` and `usage.cache.misses` accompany call/token totals; cached answers
add no calls or token usage. Corrupt entries and failed writes are misses,
and failures or malformed answers are never stored.

The model reported by a live response determines the stored key. Mutable aliases
such as `jev-latest` resolve anew from live responses each run (initial concurrent
requests may all miss); the alias map is never trusted from disk. Set
`TYPESAFE_DEFAULT_MODEL` to a versioned ID for a fully cached warm run. A model
change observed during a run invalidates subsequent alias lookups.

- A re-scan re-sends only units whose inputs changed; a large repo costs
  about its diff to re-review.
- The cache doubles as a checkpoint: an interrupted scan resumes.
- In CI, the action restores the most recent cache for the base branch and
  saves a unique run key, including after a blocking-findings failure. Seeding the base branch requires a custom trusted job using the same
  `momus-work-v1-<OS>-<branch>-` prefix and exact cache paths below. The shipped
  `scan.yml` uses separate per-shard keys and paths, so it cannot seed this cache. PR caches are
  scoped by GitHub: sibling PRs cannot read each other's caches. The action stores work under
  `RUNNER_TEMP/momus-work/{cache,index}`, outside the reviewed checkout; raw
  reports are excluded.
- Diff mode and scan mode keep their own question sets but share the unit,
  budget, and cache machinery.

### 3. Repo index

Context is precomputed once per scan instead of per request. A pre-pass
builds an index of the tree:

- regions per file (`review/regions.rs`),
- the import graph (`adapters/imports.rs`),
- the source-to-test mapping (today's `select_related_tests`),
- short export signatures per file (declaration lines, no bodies).

Context packs (1) draw from the index in both modes, including bounded
export signatures. `review/index.rs` builds the pre-pass once; scan screens
and evidence location rehydrate source regions from stored line spans, while
diff screens reuse preselected compact changed-test patches and import neighbors.

Metadata lives under `reviews/index/` (`MOMUS_INDEX_DIR` overrides it), keyed
by the Git SHA-1 blob address of the exact bytes reviewed plus the language,
region dialect, and schema version. Dirty content therefore invalidates its
entry without a commit. Only spans, candidate imports, and short declaration
signatures are persisted, not full source bodies. The tree-dependent import
graph and source-to-test maps are resolved anew from the current inventory,
so added/removed/renamed files cannot leave stale links.

`index.computed`, `index.reused`, and `index.fallbacks` report the pre-pass.
Corrupt/missing metadata is recomputed; storage errors never abort a review.
Oversized or NUL-containing inputs fall back to the ordinary region splitter;
Git discovery retains its existing guarded-read exclusions for unreadable and
binary files. Index files are local context metadata and must stay out of Git.

### 4. Shards and one global merge

`momus scan --shard i/N` partitions source paths with SHA-256 (1-based,
N <= 256). Each partial records the complete source/test inventory digest,
expected paths, model/server and configuration. It runs screening, profiles,
location and suppression locally. It defers **all** refinement and enrichment
until `momus merge`, so dedupe, taint, counterfactual, ensemble and pairwise
ranking keep their original global caps. Confidence, history, SARIF and
`github-review` also run only on the merged report. Partial reports cannot be
published. A follow-up cap is rejected on shards because applying it per shard
would change the global selection.

Merge rejects missing/duplicate/incompatible shards, wrongly assigned files,
unaccounted inventory paths, and a checkout whose discovered bytes changed.
Use a pinned `TYPESAFE_DEFAULT_MODEL` for every shard and the merger; mutable
aliases cannot establish the same model before a merge starts. Source/test
context stays global: partitioning changes ownership, not context availability.

The reusable `.github/workflows/scan.yml` has plan/build, scan matrix and merge
jobs. It fixes every target checkout to the same head SHA, builds the tool at
the reusable workflow commit, skips fork PRs/missing secrets, restores per-shard
cache/index data, and uploads **only redacted partials** with one-day retention.
Each matrix job uses a cap of 4; account-wide concurrency is shards times 4.
The merged result produces SARIF/history, and publishes once when called from
a same-repository PR workflow. Dispatch runs produce SARIF without PR posting.
Scopes are whitespace-separated directories, as in the existing review workflow.

```yaml
jobs:
  scan:
    permissions:
      contents: read
      pull-requests: write
    uses: brianluby/momus-review/.github/workflows/scan.yml@<immutable-commit>
    with:
      shards: 4
      paths: "."
      model: jev-1.13.0
    secrets:
      TYPESAFE_API_KEY: ${{ secrets.TYPESAFE_API_KEY }}
```

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
| 0 | #32 ✅ | Measure: model input limit, TypeSafe rate limits, answer stability on repeated identical requests | — |
| 1 | #33 ✅ | Bounded context packs: diff-mode related tests + one per-request budget | 0 |
| 2 | #34 ✅ | Failure isolation: skip and record failed units, circuit breaker | — |
| 3 | #35 ✅ | Release v0.1.1; re-run the Juice Shop example PR | 1, 2 |
| 4 | #36 | Work units + content-addressed result cache (incremental, resumable, CI cache) | 0, 1 |
| 5 | #37 | Repo index feeding the context packs | 1 |
| 6 | #38 | Shards + `momus merge` + Actions matrix | 4 |
| 7 | #39 | Tiered screening, priority order, `--budget` (#11) | 4 |
| 8 | #40 | Adaptive concurrency (429-aware) | 0 |

Steps 1–3 unblock the Juice Shop example; 4 is the step that makes large
repos practical.

## Open questions

- **Input limit — measured 2026-09-29, bisected against `jev-latest`**
  (resolved to `jev-1.13.0`): the largest screen-shaped request that succeeds
  carries ~122.5k characters of state (33,827 reported input tokens,
  state plus the seven screen questions); ~124k characters (~34.2k tokens)
  fails with `400 max_tokens_exceeded`. This matches the documented budgets
  (docs.typesafe.ai/models): 64k tokens per request for state plus all
  questions combined, and 32k tokens for state plus the longest question —
  the 32k budget is the one that binds a screen. Code runs ~3.8–4.2
  chars/token, so the context budget in (1) should cap state at ~100k
  characters (~26k tokens), leaving headroom for questions; the old
  2 MB (~490k-token) requests were ~7x over even the 64k budget. Latency at
  the boundary is ~0.3 s — size costs no wall time, only the cap matters.
- **Rate limits — documented and ramp-tested 2026-09-29.** TypeSafe
  documents 250k tokens/sec and 1,200 requests/min (429 with `retry-after`;
  529 overloaded), and warns the numbers adjust dynamically. A bounded ramp
  (~40 requests: concurrency 4/8/16 with ~2k-char states, plus 8 concurrent
  ~33k-token requests) saw no 429, no 529, and no rate-limit headers —
  bursts of ~6.8k requests/min and ~530k tokens/sec passed unthrottled, so
  short bursts run well above the documented steady-state ceiling.
  Sustained load was not measured (that needs >1.2k requests/min for
  minutes); adaptive concurrency (#40) should assume the documented
  1,200 requests/min and react to the first 429/529 it sees. At the
  measured 0.1–0.5 s per request, today's fixed concurrency of 3 (~600
  requests/min worst case) has headroom; sharding (#38) splits the work but
  every shard draws on the same per-account pool, so N shards still share
  one 1,200-requests/min ceiling.
- **Answer stability — measured 2026-09-29: tight enough to cache.** One
  real screen (a 5.9k-char region of `run_review` at `d8b8fa9^`, the
  collect-all-then-fail bug) sent 6 times sequentially and, from the ramp,
  16 times concurrently: every noul probability spread ≤ 0.02 (stdev
  ≤ 0.009; one question returned exactly 0.02 on all six runs). The
  follow-up file-role choice picked `domain` 5/5 (confidence 0.52–0.57) and
  the review-priority score spread 0.09 (1.84–1.93). Nothing crossed the
  0.7 screen threshold, so no decision flipped. Cached results (#36) can
  replace fresh ones; only probabilities within ~±0.03 of a threshold could
  route differently, and the key should pin the versioned model id
  (`jev-1.13.0`), since `jev-latest` is an alias that silently moves.
- **One question set or two.** Diff mode asks "does this change introduce…",
  scan mode "does this code have…". Sharing machinery is clear; whether a PR
  review should also use scan questions on changed regions is not.
- **Cache invalidation on policy change.** The questions JSON is in the key,
  so any wording change invalidates every result. That is correct, but a
  policy tweak then costs a full re-scan.

## Budget and adaptive gate implementation

`--budget calls=N` is a shared atomic HTTP-attempt cap, including retries,
across every stage. Cache hits are checked first and cost zero attempts.
Budget-exhausted work is recorded instead of tripping the failure breaker.
Priorities combine persisted role/priority profiles, finding hotspots, Git churn
and conservative path role hints; ties use path order. This schedules files,
then their existing stages; it does not claim an optimal value-of-information
planner. Reruns resume cached successful units; failed/uncached units retry.

`--tiered` asks one cheap question for each file whose complete content fits the
context budget. Only probabilities below 0.05 skip full screening; uncertain,
failed, and oversized files retain full screening. Dismissals remain explicit
and the report remains partial. Default scans never use this heuristic. A paired
Juice Shop evaluation must show no loss of baseline finding coverage and curated
security-route coverage before promoting it to default behavior.

Every client clone shares one adaptive gate. Eight healthy responses increase
capacity by one, up to `MOMUS_CONCURRENCY` (default 16). HTTP 429/529 halve the
current capacity and establish a shared cooldown; numeric Retry-After is honored
up to five minutes. Permits are released before retry sleeps and on cancellation.
Transport/other 5xx retries keep the existing bounded backoff. Healthy cached
answers neither consume permits nor inflate the success ramp.
