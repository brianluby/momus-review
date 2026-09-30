# Optional review robustness features

These options ship on the 0.2 line because the public Rust struct contracts have
new fields. See [Rust API migration](migration-0.2.md). CLI defaults and older
stored JSON reports remain compatible.

Review and unsharded scan support three independent options:

```bash
momus review --base origin/main --test-plans --spec docs/requirements.md \
  --follow-up-strategy voi --follow-ups 8 --budget calls=100 \
  --threshold security=0.65 --threshold testGap=0.8

momus scan src --spec docs/requirements.md --test-plans
```

All three use the ordinary System One client's secret redaction, result cache,
retry policy and atomic HTTP-attempt budget. Default reviews add none of the new
test-planner or spec judgments and keep probability-based follow-up ordering.
No generated scaffold is applied or executed. Local reports contain verbatim
evidence; sanitized artifacts and PR publication redact it.

## Test planner (#18)

`--test-plans` selects up to eight supported test-gap findings after refinement
and suppression, in final report order. It supplies the finding's intact evidence,
bounded source context, related existing/changed test snippets and import neighbors.
One closed `choice` selects a mechanism-compatible strategy: unit regression,
branch table, injected failure, boundary table, property invariant or component
integration. A missing, unknown or less-than-0.7-confidence choice produces no
plan. A generic/uncertain mechanism cannot produce a specific test scaffold.

`findings[].testPlan` carries strategy/confidence, file/line, scenario, required
observable assertion, scaffold language, `stub`, `incomplete: true`, required work
and context-drop counts. Rust, Python, JavaScript, TypeScript and Go have static,
deliberately failing native scaffolds. Other source languages receive a plain-text
plan. They contain placeholders for repository-specific setup, calls and assertions,
so they provide **no regression coverage until completed**. Repository text never
becomes executable scaffold content. Context clipping is explicit; clipped finding
evidence causes abstention. Plans are visible in the dashboard and PR comments,
including the sticky summary for findings previously posted inline.

## Spec drift (#19)

Repeat `--spec PATH` to supply authoritative local UTF-8 requirement documents.
Input must be regular files, not symlinks, directories or FIFOs: at most eight
files, 64 KiB total and 256 nonempty lines. Empty, binary and excessive input fails
before API work. Each nonempty line is an evidence candidate with original line
numbers. Metadata retains path, byte count and a SHA-256 content hash. All supplied
lines are compared together, so a selected line never replaces qualifying context.

Each discovered source file is checked independently, including files whose
ordinary risk screen was deferred. A typed comparison chooses `drift`, `matches`,
`notApplicable` or `uncertain`. Reporting a contradiction requires at least 0.85
confidence in the assessment and actual requirement/source selections, followed
by a second skeptical probability of at least 0.85. Quotes are taken from the
original candidates; the model cannot invent evidence text. Diff evidence contains
only current added/context lines, never removed lines or the base implementation.
Deletion-only diffs abstain.

`specDrift.documents` contains input provenance; `specDrift.checks` contains file,
status/confidence, reason and exact `spec`/`source` path/line/text evidence for drift.
The first call compares and selects; a second call runs only for a confident
contradiction candidate. Missing context, unsupported answers and insufficient
confidence become `uncertain`; budget/service errors become `deferred`. Both make
the overall report partial. Complete code and specification context must fit the
configured context budget, otherwise the check abstains without a request.

These results are **advisory**, shown in JSON, the dashboard and the PR summary.
Requested spec comparisons run after core locating and before optional refinement,
fix/test suggestions and scaffolds, so optional output cannot consume their budget first.
They do not add a sixth dimension, affect P(revert), or change `--fail-on-blocking`.
At most one contradiction is reported per file. `matches` means only that visible
relevant behavior appears consistent; it does not establish full requirement
coverage or compliance. Cross-file behavior may require human review. There is no
automatic ticket/URL fetching. Changing requirement text changes its cache keys.

## VOI follow-up selection (#11)

`--follow-up-strategy voi` uses a deterministic heuristic over signals already
eligible under their dimension thresholds:

```text
uncertainty = 4 * p * (1 - p)
score = impact_weight * p * (1 + uncertainty) / 5
```

The impact weights are security 3, correctness 2, reliability 2, compatibility 1.5,
test gap 1. Five is a nominal upper estimate for ordinary locate judgments,
including possible routing; early abstention/cache hits can lower cost and retries
can raise it. Likely concerns retain value even at low uncertainty. These weights
are policy choices, not calibrated probabilities or demonstrated cost savings.

With `--follow-ups N`, the highest-ranked available signal in each dimension gets
a slot when the cap permits, then remaining slots fill by score. Ties use file path
and dimension. With no follow-up cap, every eligible signal is retained. A cap that
omits eligible signals marks the report partial in both ranking modes.
`followUpPlan.candidates` audits probability, threshold, impact, uncertainty,
estimated calls, score and selected status. The dashboard and PR summary show the
selection; actual call/token/cache/wall-time meters continue to report real usage.

With a finite `--budget calls=N`, VOI omits optional file profiles and runs locate
pipelines sequentially to finish evidence for the highest-priority signal before
spending on the next. Screens still precede follow-ups; a very small budget may
be exhausted by screening alone. The hard atomic cap counts actual HTTP attempts
including retries. Nominal estimates never reject cached work: a pinned-model warm
rerun with `calls=0` can replay all cached judgments. There is no inferred dollar
price or latency guarantee.

Repeat `--threshold DIMENSION=P` to override eligibility after reviewer-feedback
tuning. Dimensions are correctness, security, reliability, compatibility and
testGap; P must be finite in [0,1]. Duplicate overrides are errors. The applied
thresholds and ranking policy are saved in `config`. Overrides deliberately change
which signals get investigated; lowering a threshold can increase calls and noise.

## Boundaries

`--test-plans`, `--spec` and VOI reject `--shard` rather than silently applying a
global option independently to shards. Existing sharded workflows remain unchanged.
Specs also reject experimental `--tiered`, since dismissed files would escape
requirement checks. These options can be used independently or together in review
and ordinary scan. Budget-limited runs explicitly disclose unfinished work; a
follow-up cap must be raised or removed to investigate omitted signals.

## Validation

Run `cargo test --all-targets --all-features`,
`cargo clippy --all-targets --all-features -- -D warnings`, and
`node --test tests/dashboard-robustness.cjs`. The HTTP tests use deterministic
loopback servers and child-process configuration, including combined-feature cold
runs, pinned-model zero-call cache replay, requirement edits, confidence abstention,
budget priority and deferred coverage. Dashboard regressions verify threshold
overrides, incomplete-review disclosure and inert source/spec text.

These checks establish integration behavior. They do not measure hosted-model
precision/recall, generated test coverage, or actual savings from the VOI heuristic.
