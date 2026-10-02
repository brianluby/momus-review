# Merge confidence and automatic approval

Momus distinguishes three outcomes: a **revert** undoes the merged change, an
**incident** is an operational incident attributed to that change, and a **flake**
is an intermittent test failure attributed to it. They are separate labels,
windows, models and evaluations. An incident need not cause a revert; a passing
test run does not establish absence of later flakes.

The existing `pRevert` report field remains a hand-weighted review heuristic for
compatibility. `mergeConfidence.heuristicScore` labels it explicitly as
uncalibrated. A low or zero heuristic is **not** evidence of low merge risk.
Every outcome probability is `null` when historical evidence is absent or
insufficient. Momus does not derive a single misleading complement called
"merge confidence" from three different outcomes.

## Supply an auditable outcome history

`OutcomeHistory` JSON uses camelCase and rejects unknown fields. Supply one
repository's history with a source description in `provenance`, an explicit
`synthetic` flag, required `heuristicVersion: 1`, Unix-second `asOf` and `trainingCutoff`, and separate positive
`windows` (`revertSeconds`, `incidentSeconds`, `flakeSeconds`). The default Rust
windows are 30 days, 30 days and 7 days; history files must state their windows.
Use the same outcome definitions, surveillance process, review pipeline and
heuristic formula for all records. Record changes to these definitions in the
provenance and start a separate history when comparability is lost.
`HEURISTIC_VERSION` pins the formula, severity scale and fixed bins. Histories
missing that field or naming another producer version are rejected before
fitting. `mergeConfidence.heuristicVersion` identifies the current producer;
its separate `version` identifies the report format. An old saved summary with
no producer field reads as version zero, meaning unverified provenance, and
is never reused to authorize approval. Do not relabel incompatible scores with
a newer version number.

Each record names its unique merged `head`, `mergedAt`, `scoreRecordedAt` and
`heuristicScore`. Freeze the score before the merge; recomputing scores with
later knowledge creates hindsight leakage. Each of `revert`, `incident` and
`flake` is either `null` (unknown) or an object:

```json
{
  "occurred": false,
  "observedAt": 1780000000,
  "evidence": "surveillance export identifying this head and complete observation window"
}
```

For a positive outcome, `observedAt` is the attributed event time and must fall
within the outcome window. For a negative outcome, it is the end of verified
surveillance and must cover the entire window. Every supplied label requires a
nonempty evidence reference. An absent incident record, an issue search with no
results, a missed monitoring interval or unavailable test telemetry remains
unknown. Momus validates times, windows, uniqueness, bounds and evidence presence;
it cannot verify a caller's external surveillance or causal attribution.
Nonsynthetic history requires full, nonzero, lowercase hexadecimal Git head
identities of 40 or 64 characters. Abbreviated, uppercase and padded aliases
are rejected, so a candidate's own record cannot bypass the independence
check under another spelling. Synthetic demonstrations may use opaque IDs.

## Reproducible evaluation

Momus bins the frozen heuristic into `[0,.25)`, `[.25,.5)`, `[.5,.75)` and
`[.75,1]`. Each bin's prediction is the training event rate with Laplace
smoothing `(events + 1) / (samples + 2)`. The model never tunes boundaries or
thresholds from the held-out outcomes. Sorting input records does not change
the result.

Records merged before `trainingCutoff` train an outcome only if its whole
window matured by the cutoff and its label was available by then. Records
merged at or after the cutoff are held out, and contribute only once their
window matured by `asOf`. Unknown, immature or unavailable labels are counted
explicitly. Positive early events are also held to the maturity rule to avoid
selectively including events while excluding censored negatives.

For each outcome the report includes training and held-out sample/event counts,
Brier score, a constant training-rate baseline's Brier score, and expected
calibration error weighted across held-out score bins. Sparse bins use the
training baseline for evaluation. A probability requires at least 40 training
labels, 20 held-out labels, both outcome classes in training, and 20 training
plus 20 held-out labels in the candidate's score bin. These are minimum
engineering guardrails; they do not establish statistical or real-world
calibration. Poor held-out metrics remain visible even when an empirical
estimate can be calculated.

The candidate estimate also includes separate Wilson 95% binomial upper bounds
for its training and held-out bins, together with the candidate bin's held-out
calibration error. Each interval describes sampling uncertainty in its own observed bin,
without accounting for distribution shift, surveillance bias, causal
misattribution or dependence between changes. `evaluatedEmpirical` means an
empirical estimate with held-out evaluation, **not** certified calibration.
Use real repository data and prospective monitoring before relying on it.
The evaluation also records the candidate bin's latest usable training and
held-out `observedAt` timestamps and its latest mature held-out `mergedAt`.
These refer only to records actually admitted to that outcome's chronological
fit/evaluation, excluding unsupported, unknown and immature labels.

## Explicit, fail-closed automatic approval

Automatic approval is disabled by default. An enabled `ApprovalPolicy` must
name at least one required check. Defaults cap each outcome's upper uncertainty
bound (both training and held-out) at 0.1, pooled and candidate-bin held-out calibration error at 0.1, finding severity below 1.0,
check age at one hour and history age at 30 days. The bin predictor must perform
at least as well as its held-out constant baseline. Supply stricter thresholds
to match the repository's actual risk tolerance. Invalid thresholds are errors.
The built-in blocking severity (2.0) always rejects approval, even if a policy
uses a more permissive finding threshold or a stored finding has an incorrect
comment action.

Approval requires all three supported, nonsynthetic outcome estimates and fresh
current-head evidence. `CheckEvidence` names `repository`, `head`,
`reviewedHead`, `assessedAt`, `reviewComplete`, `evidenceComplete`,
`supportedInputs`, `auxiliaryUnknowns`, and a `checks` array. A named check
includes `name`, exact `head`, `status`, `completedAt`, and a nonempty evidence
reference. Only `passed` is accepted. Required missing checks, any supplied
failed/pending/unknown checks, duplicate names, stale results, future
timestamps and results for other heads reject eligibility.
Every `CheckEvidence` JSON field is required, including an explicitly supplied
`auxiliaryUnknowns` array. An omitted completeness assertion is an input error,
never an assertion that nothing is unknown. The Rust `Default` constructor
remains available for constructing incomplete, ineligible evidence internally.

A recent export `asOf` is insufficient to make old telemetry current. For each
outcome and the candidate's score bin, approval also requires the latest usable
held-out observation within `maxHistoryAgeSeconds`, and the latest mature
held-out merge within that age plus the outcome window. The window allowance
permits ordinary 30-day outcomes to mature before evaluation. The latest usable
training observation must fall within the same age-plus-window allowance,
because training labels precede the cutoff and held-out windows mature after
it. Refreshing a negative-surveillance timestamp on an ancient merge cannot
refresh its old score cohort. These recency checks establish minimum current
evidence support; they do not establish a sufficient recent sample size or
guarantee safety against distribution shift.

The report's `reviewedHead`, the evidence's `reviewedHead` and current `head`
must match exactly. The candidate cannot be in its own training/evaluation
history. Nonsynthetic approval also requires a canonical full lowercase
candidate head. The history must belong to the same repository and be no later than
the assessment. An old report without a recorded head cannot be approved.
The report must also record an actual `reviewedBase` merge-base identity,
`reviewedClean: true`, and `reviewedCommitted: true`. Run
`momus review --base origin/main --committed-only` to read the diff and repository
context from immutable Git objects at the bound head, preventing transient
working-tree or untracked context from being attributed to different committed
contents. Publication verifies the current live PR base and head. Ordinary
working-tree reviews remain useful but cannot authorize automatic approval.

Incomplete, skipped, deferred, truncated or sharded reviews, omitted follow-up
signals, missing file coverage, high severity or request-changes findings,
human-judgment findings, unsupported inputs and unknown auxiliary evidence
all reject automatic approval. The integrating caller must verify full
changed-file coverage and auxiliary evidence before setting completeness
booleans; those assertions are a trust boundary. In particular, missing
changelogs, unsupported manifests, unprovable documentation drift or incomplete
repository context must be listed in `auxiliaryUnknowns`.
Saved review reports retain tolerant legacy deserialization, including
workflow counters. They are not authenticated completeness evidence: privileged
publication requires a report produced by the trusted committed reviewer and
fresh provider inventory/check verification. Legacy missing head/verification
fields reject approval; externally authored reports must not be promoted to
trusted review artifacts merely because they deserialize.
All five review dimensions must be present for every uniquely identified matrix
file. Selectively omitting a dimension prevents automatic approval even when
the requested subset completed successfully.
Suppressed findings also prevent approval because their original risk cannot
be established from the remaining findings. Supplied specification checks must
be explicitly inapplicable or match with located source and requirement
evidence; drift, uncertainty, deferred checks and unsupported matches reject
approval. The existing advisory specification analyzer does not attach evidence
to `matches`, so those results currently require human approval.

`assess` computes eligibility and reasons without publishing an approval.
Publication must separately verify live repository/head/check identity and
require the user's explicit opt-in policy. Evidence files are assertions,
not an authenticated replacement for live provider checks.

## Synthetic reproducibility fixture

Regenerate committed JSON deterministically with:

```sh
python3 examples/merge-confidence/generate.py
momus confidence \
  --report examples/merge-confidence/routine-report.json \
  --history examples/merge-confidence/synthetic-history.json \
  --policy examples/merge-confidence/opt-in-policy.json \
  --checks examples/merge-confidence/synthetic-checks.json
```

The fixture has 60 training and 40 held-out records with fabricated outcomes
and check receipts. Its estimates are `syntheticDemonstration`, and approval
is rejected despite a routine report and enabled policy. This verifies
serialization, evaluation and policy mechanics only. Unit tests additionally
exercise the observed-source contract to cover eligible routine changes, but
their fabricated labels are not real calibration evidence.

The remaining external requirement for real-world validation is a provenance
backed repository-specific export of premerge scores and separately attributed
revert, incident and flake outcomes, with mature positive/negative surveillance
windows and a chronological holdout covering the candidate score bins. Collect
those records prospectively, publish the measured held-out results and monitor
drift before describing the engine as calibrated on real outcomes.

The [prospective collection/export protocol](outcome-collection.md) now provides
offline frozen-report capture and separately sourced outcome validation. It
preserves label availability separately from event/surveillance time and
withholds labels unavailable at their chronological boundary. Its audited
repository source inventory records available candidate premerge reports and
the exact external-evidence gaps; collection machinery does not close #17/#51.

## Trusted GitHub publishing setup

Use a clean checkout of the exact PR head and pin the binary/action source. Keep review cache, index and reports outside that checkout (or ignored):

```sh
export MOMUS_CACHE_DIR="$RUNNER_TEMP/momus-cache"
export MOMUS_INDEX_DIR="$RUNNER_TEMP/momus-index"
export MOMUS_REPORT="$RUNNER_TEMP/momus-report.json"
momus review --base origin/main --committed-only
momus github-review --report "$MOMUS_REPORT" \
  --auto-approve-policy /trusted/approval-policy.json \
  --history /trusted/outcome-history.json
```

The publishing step needs `contents: read`, `checks: read`, `statuses: read` and `pull-requests: write`. Run it after the required checks complete; an in-progress check, including a still-running review job, prevents approval. Policy and history must come from maintainer-controlled storage or the trusted base branch, and the report must come from the trusted reviewer. PR-authored JSON evidence is not an authorization mechanism. `confidence --checks` is an offline assessment interface; the publisher obtains live evidence itself.

The publisher compares exact report/PR head and merge base, complete changed-file counts and reviewed matrix paths. It rejects unavailable/binary patches, more-than-100 check inventories, unsupported/non-source inputs, absent checks and any non-success result. Required named checks must exist, and all returned check/status results must succeed. Duplicate/ambiguous names reject rather than selecting one provider. It recomputes eligibility before publication and again immediately before an exact-commit approval, verifying the head and base again. Provider changes after the final API reads remain subject to GitHub branch protection; these REST calls do not form an atomic transaction.

Committed-only review reads diffs and source/test context from the pinned Git tree. It currently rejects combination with worktree-based `--upgrade-triage`/`--docs-drift`; dependency/docs/configuration-only changes cannot pass the publisher's source-coverage gate. Dirty or untracked checkout inputs, mixed repository scopes, suppressed findings, specification drift and uncertain spec matches reject eligibility. Automatic approval is intentionally limited to routine fully screened source changes with sufficient real outcome history.

Committed source context is limited to 10,000 files, 10 MB per file and 100 MB
of source text. Tree sizes are checked before a batched immutable blob read.
Unavailable, binary, non-UTF-8 or nonregular source context fails the committed
review before API screening rather than silently presenting incomplete context.
