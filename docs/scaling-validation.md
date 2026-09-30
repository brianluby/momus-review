# Scaling bundle validation

Veans #36, #37, #38, #39, #40, #42, #47. Local evaluation on 2026-09-29.

## Public benchmark

OWASP Juice Shop commit `1618a611b173b4bf114028e6e02549950606e29d`,
tree `339d8833f7111ee1f4ca91fe568d63cb973f2274`, clean tracked checkout.
Pinned model `jev-1.13.0`, TypeSafe API, concurrency cap 12. Exclude
`frontend/src/assets/**`, `data/static/codefixes/**`,
`data/static/contractABIs.ts`; generated `reviews/**` excluded on reruns.
A 5,000-attempt budget was set on each scan and was never exhausted.

| Run | Files screened | Final findings | HTTP calls | Cache hits | Category coverage | Curated routes |
|---|---:|---:|---:|---:|---:|---:|
| Cold monolithic | 300 | 77 | 2,108 | 0 | 6/16 | 13/23 |
| Tiered, using baseline cache | 300 | 77 | 300 | 2,108 | 6/16 | 13/23 |
| Warm monolithic | 300 | 77 | 0 | 2,108 | 6/16 | 13/23 |
| Four shards + global merge, using same cache | 300 | 77 | 15 | 2,108 | 6/16 | 13/23 |

The sharded merge's complete finding array and `pRevert` exactly matched the
warm monolithic report. No missing or additional finding identities. All
refinement/enrichment executes globally, so top-K caps are not multiplied by
shard count. The 15 extra calls are per-shard profiles, which are not gating;
the merged report retains the global top five profiles.

Tier 0 made 300 cheap calls and dismissed **zero** files at the conservative
0.05 cutoff. It preserved all baseline findings but demonstrated no screening
savings on this target. Keep `--tiered` opt-in; this is paired equivalence on
one benchmark, not evidence that arbitrary dismissed files are safe. The
category metric is coverage, and the curated-route metric is an incomplete
lower bound; neither is total vulnerability recall. Baseline contained no
`other` findings, so the catch-all explanation regression is covered by the
unit vocabulary tests rather than this benchmark's findings.

Cold baseline took 42,781 ms, used 5,538,218 input / 212,910 output tokens,
and trimmed no context. Warm monolithic took 631 ms with zero HTTP calls.
These are local measurements, not promised hosted performance.

## Automated checks

- Rust all-target/all-feature tests, strict Clippy, binary build.
- CLI cap zero/one/ten attempts, with metering and cached resume.
- Five-shard partition/merge equality and dirty-checkout rejection.
- Missing, duplicate and incompatible shard rejection.
- Concurrent atomic reservations and 429 retries charged to the budget.
- Adaptive capacity reduction, cooldown, bounded healthy ramp and cancellation.
- Cold/warm index request equality, cache-only reruns, dirty-file invalidation.
- Per-dimension fix vocabularies and catch-all explanation behavior.
- Report-derived stderr metrics and CI artifact redaction.
- Scoped Rust formatting and `git diff --check`.
- `actionlint` for the matrix workflow, ignoring only its outdated schema's
  two `job.workflow_repository` / `job.workflow_sha` errors. Both fields are
  documented by GitHub and pin the called workflow's own tool source.

Whole-repository formatting has pre-existing failures in untouched source.
New hosted matrix execution remains a post-publication check; this local
validation does not claim it already ran on GitHub.
