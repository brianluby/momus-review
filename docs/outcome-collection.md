# Prospective repository outcome collection

`eval/outcomes/collect.py` supplies the feasible collection and export work for
Veans #51, under merge-confidence parent #17. It does not change the estimator
or approval gates delivered by #52. It calls no model or remote service. The
engine remains an uncalibrated heuristic plus a separately evaluated empirical
model; this collector does not establish real calibration acceptance.

## Source inventory, 2026-10-02

This audit inspected `brianluby/momus-review` at source revision
`4bbcd16312774109ca2c147989a032c4401ed627`, repository files outside the excluded
Jeeves research, and read-only GitHub PR/artifact/issue APIs. It did not inspect
private incident systems, operational monitoring, or unpublished test telemetry.

| Source | Observed inventory | Evidence limit |
| --- | --- | --- |
| GitHub merged PR inventory | 40 PRs, merged between 2026-09-25T19:11:49Z and 2026-10-01T00:53:58Z | Identifies PR heads, merge commits and merge times; does not supply outcome negatives or source attribution. |
| GitHub Actions artifact inventory | 124 artifacts over two pages; 21 named `momus-report` | Candidate archived review evidence, requiring inspection and qualification per report; artifact count is not a qualifying-score count. |
| Latest report artifact | Artifact `11133984977`, run `36798087615`, created 2026-10-01T00:51:06Z at PR #40 head `216c860c81429776182f1020d3db7cad4b7346de` | Archived before PR #40 merged at 00:53:58Z. The report contains a version-1 heuristic score and exact reviewed head/base, but `reviewedCommitted` is `false`. |
| Current dogfood workflow | `.github/workflows/momus.yml` checks out the PR head and runs `uses: ./` with `version: source` | Producer and workflow are controlled by the PR. The archival timestamp alone does not establish a maintainer-controlled committed-review pipeline. |
| Repository history fixture | `examples/merge-confidence/synthetic-history.json`, explicitly synthetic | Exercises serialization/estimation only; cannot train or validate real acceptance. |
| Revert/incident/flake discovery | Read-only issue search `revert OR incident OR flake` returned no issues; no independently supplied incident/flake surveillance export found in the inspected repository | Search absence is unknown telemetry. It supplies no negative labels, causal attribution, or complete surveillance windows. |

The latest archive was actually downloaded and inspected, not inferred from its
name. SHA-256 of the archive was
`0bd277997b6586e9968579383cd506da78bc4bf96c9b36ff625146ac20857a1a`, matching
the artifact API digest. SHA-256 of its `report.sanitized.json` was
`0ed65f0623469f22357a0ebf03b0b10a0b1d2bb8fd98aea8c8e3568dbb51a567`.
The archive contains one report. Its actual source-bound score is useful audit
evidence; no committed-review qualification or mature outcome label is inferred.
The other 20 candidate report archives were inventoried, not individually
qualified. Existing archived bytes must be assessed on their own provenance;
rerunning reviews now would introduce hindsight.

At the refreshed inventory time, 2026-10-02T07:44:38Z, all 40 merged changes
were less than seven days old. None could yet supply a mature default seven-day
flake window or thirty-day revert/incident window. Even with fully qualified
frozen scores and telemetry, 40 total merges cannot supply the engine's disjoint
minimum 40 training plus 20 held-out records for each outcome. This is a
population/window blocker in addition to the missing qualified telemetry.

Reproduce the source discovery read-only:

```sh
rtk gh pr list --repo brianluby/momus-review --state merged --limit 100 \
  --json number,headRefOid,mergeCommit,mergedAt
rtk gh api --paginate \
  'repos/brianluby/momus-review/actions/artifacts?per_page=100' \
  --jq '{total:.total_count,returned:(.artifacts|length),reports:[.artifacts[]|select(.name == "momus-report")|{id,created_at,digest,workflow_run}]}'
rtk gh issue list --repo brianluby/momus-review --state all --limit 100 \
  --search 'revert OR incident OR flake' --json number,title,url,state
```

Primary source identities: [PR #40](https://github.com/brianluby/momus-review/pull/40),
[review run](https://github.com/brianluby/momus-review/actions/runs/36798087615),
[artifact metadata](https://api.github.com/repos/brianluby/momus-review/actions/artifacts/11133984977),
and [audited workflow](https://github.com/brianluby/momus-review/blob/4bbcd16312774109ca2c147989a032c4401ed627/.github/workflows/momus.yml).
Inventories and retention change; refresh them before making a later acceptance
decision. No real-data evaluation was performed by this audit.

## Register one immutable protocol

Before collecting scores, choose one repository, fixed committed-review
pipeline, heuristic producer version, outcome definitions, telemetry sources,
separate windows and a chronological training cutoff. Timestamp and archive
that protocol in maintainer-controlled storage before the cutoff. Keep source
and receipt bytes; changing the protocol starts a new cohort. Do not select a
cutoff after looking at held-out labels or change definitions to improve scores.

The protocol is an evidence envelope with exactly `payload` and `receipt`.
Its payload has this shape (example values are explanatory, not real evidence):

```json
{
  "schemaVersion": 1,
  "repository": "owner/repository",
  "heuristicVersion": 1,
  "pipelineIdentity": "maintainer-controlled immutable binary/workflow/source identity",
  "provenance": "registered cohort, source owners and surveillance/adjudication process",
  "protocolAuthority": "maintainer-protocol-archive",
  "scoreAuthority": "trusted-review-archive",
  "mergeAuthority": "github-verified-pr-export",
  "trainingCutoff": 1800000000,
  "windows": {
    "revert": {
      "seconds": 2592000,
      "definitionId": "attributed-revert-v1",
      "definition": "A full or partial undo attributed to this exact merged PR, adjudicated from commit and PR evidence.",
      "authority": "verified-revert-ledger"
    },
    "incident": {
      "seconds": 2592000,
      "definitionId": "attributed-incident-v1",
      "definition": "An operational incident attributed to this exact change using incident-owner evidence.",
      "authority": "verified-incident-ledger"
    },
    "flake": {
      "seconds": 604800,
      "definitionId": "attributed-flake-v1",
      "definition": "An intermittent test failure attributed to this exact change using retained test-run and adjudication evidence.",
      "authority": "verified-flake-ledger"
    }
  }
}
```

Specify inclusion/exclusion rules, partial reverts, severity thresholds, causal
attribution, test rerun semantics, monitoring gaps, population coverage and
adjudication ownership in the registered definition/process. Retain the detailed
policy at the protocol's provenance reference. A revert does not imply an
incident or flake. A passing test run and a search without matches do not imply
complete surveillance. Every outcome's telemetry authority must differ from
the score source; different definitions always have separate identities.

## Receipt contract and trust boundary

Each receipt has exactly these required fields:

```json
{
  "authority": "registered source authority",
  "uri": "immutable source/version reference",
  "sha256": "64 lowercase hexadecimal characters",
  "recordedAt": 1800000001,
  "producer": "immutable producer identity",
  "verification": "reference to the maintainer's provider/signature/source verification audit"
}
```

The verifier must independently establish the timestamp, source, producer and
payload digest from the trusted archive/provider, including exact attribution
version and the first time the label was available. `verification` records that
audit; a nonempty string does not perform it. The script checks consistency and
digests, not authenticity, signatures or external surveillance. It does not
trust an artifact merely because JSON fields claim an authority. Keep the
protocol and receipts in maintainer-controlled storage outside PR authors'
control. A receipt created today cannot assert that a newly reconstructed score
or newly attributed label was available last month.

For a raw review report, `sha256` hashes its original UTF-8 bytes exactly.
For every envelope, it hashes canonical UTF-8 JSON of `payload`: sorted keys,
compact separators, unescaped Unicode, no newline and no nonfinite numbers.
`collect.canonical` and `collect.digest` implement that encoding. Normalize
provider evidence in the trusted collector, archive the normalized bytes, and
bind them with a provider/maintainer verification receipt. Keep the original
upstream API/event evidence in the verification audit. Do not confuse the
digest of a zip archive with the digest of the enclosed report.

## Freeze before merge

Use an existing trusted report from a clean committed-object review. Capture
requires `mode: changes`, `partial: false`, canonical `reviewedHead` and
`reviewedBase`, `reviewedClean: true`, `reviewedCommitted: true`, and an already
emitted version-1 `mergeConfidence.heuristicScore` consistent with `pRevert`.
Known native `workflow.droppedContextChars` and `droppedContextItems` counters,
when present, must be zero integers: Momus can record context clipping while
leaving `partial` false. Existing stored reports need not supply optional
counters their producer did not emit. It never calculates or revises the score.
The report receipt must identify the
protocol's exact `pipelineIdentity` and score authority.

Independently verify the current open PR and supply an envelope whose payload
contains exactly `repository`, integer `pullRequest`, exact `head`, reviewed
merge-base `base`, `state: OPEN`, and `mergedAt: null`. Its receipt uses the merge
authority, is at least as recent as the score receipt, and binds that normalized
source snapshot. The base identifies the actual reviewed merge base; the live
target branch tip is not necessarily that same Git object.

```sh
rtk proxy python3 eval/outcomes/collect.py capture \
  --protocol /trusted/protocol.json \
  --report /trusted/report.json --report-receipt /trusted/report-receipt.json \
  --open-pr /trusted/open-pr.json --output /trusted/frozen-draft.json
```

The command writes a new draft containing the exact report bytes, bound source
receipts, extracted score, identities, protocol digest and current local capture
time. Archive that draft before merge and create a separate receipt envelope
over its canonical payload, using the registered score authority and pipeline.
That external receipt is necessary: the draft's local clock is insufficient
evidence. `captures.json` is an array of these externally receipted drafts.
Both the report-source time, local capture time, and externally archived
capture time must be strictly earlier than merge. Equal-second ordering is
ambiguous and rejected. No command overwrites an existing capture or export.

```sh
rtk proxy python3 eval/outcomes/collect.py validate \
  --protocol /trusted/protocol.json --captures /trusted/captures.json
```

Use a preregistered selection rule for repeated reviews. The exported cohort
must contain exactly one eligible capture per PR and exact merged head; remove
superseded revisions from the selected input while retaining them in the
immutable archive and inclusion/exclusion audit. The script rejects ambiguous
duplicates and never picks a favorable score retrospectively.

Raw reports can contain private source context. Store them in the controlled
evidence archive; do not commit or publish them without the required source
authorization. The committed unit-test inputs are explicitly fabricated and
are not substitute outcome history.

## Join merges and supply independent observations

`merges.json` is an array of receipt envelopes. Each payload has exactly
`repository`, integer `pullRequest`, `head` (the exact reviewed PR revision),
`mergeCommit` (the provider's merge/squash commit), and Unix-second `mergedAt`.
The receipt uses the protocol's merge authority and cannot precede merge.
Join by repository, PR identity and full head; short SHAs, case aliases,
duplicate heads/merge commits, another revision and duplicate PRs are errors.
Missing scores exclude merges with an audit reason; unmerged captures remain
in the audit. Missing merge inventory must be investigated independently: an
export cannot establish completeness of a source the operator did not supply.

The engine record's `head` remains the scored PR head incorporated through the
merge, preserving its candidate-independence comparison. The separate merge
commit is retained in the audit. Squash/merge commits must never replace the
reviewed identity when joining scores or attributing labels.

`observations.json` is an array of independent source envelopes. Each payload
has exactly these fields:

```json
{
  "repository": "owner/repository",
  "pullRequest": 123,
  "head": "full lowercase reviewed PR head",
  "kind": "incident",
  "definitionId": "attributed-incident-v1",
  "occurred": false,
  "observedAt": 1802592000,
  "availableAt": 1802592600,
  "coverageStart": 1800000000,
  "coverageEnd": 1802592000,
  "coverageComplete": true,
  "attribution": "independently verified uninterrupted surveillance, population coverage and absence of attributed events"
}
```

For positive events, `observedAt` is the attributed event time inside that
outcome's own window; `coverageStart`, `coverageEnd` and `coverageComplete`
must be `null`. For negatives, coverage must start no later than merge,
extend through the complete window, and explicitly assert
`coverageComplete: true`; `observedAt` equals `coverageEnd`. Evidence owners
must verify continuity and monitoring coverage before making that assertion.
Unavailable instrumentation, missing intervals and unverified attribution
remain omitted observations, never negative labels. Conflicting/duplicate
observations require upstream adjudication, not last-write-wins selection.

`availableAt` is when the exact attributed label first became available in
trusted immutable source evidence; it equals that envelope receipt's
`recordedAt` and cannot precede `observedAt`. Incident occurrence, later
attribution, and surveillance completion are different events. Never backdate
availability to the incident's event time or the end of the window.

## Export without chronological leakage

```sh
rtk proxy python3 eval/outcomes/collect.py export \
  --protocol /trusted/protocol.json --captures /trusted/captures.json \
  --merges /trusted/merges.json --observations /trusted/observations.json \
  --as-of 1806000000 --output /trusted/outcome-history.json \
  --audit /trusted/outcome-availability-audit.json
```

The output uses the engine's strict `OutcomeHistory` schema with
`heuristicVersion: 1`, `synthetic: false`, repository, source provenance,
registered windows/cutoff and joined records. Every missing label is `null`.
An empty or sparse export remains valid collection evidence, not sufficient
calibration support. The supplied `asOf` must follow the cutoff and cannot be
in the future. Merges whose receipt was not available by `asOf` are excluded.

The current engine's `ObservedOutcome` has event/surveillance `observedAt` and
does not represent separate label availability. Consequently this collector
withholds a pre-cutoff merge's label unless both its entire window matured and
the trusted label was available by `trainingCutoff`. For held-out merges
(including ones exactly at the cutoff), both must hold by `asOf`. Early
positive events also wait for window maturity. An event before cutoff attributed
after cutoff cannot train the predictor. The collector exports its label as
`null` and retains both clocks and the precise withholding reason in the
separate audit. It does not overload `observedAt`, change the estimator, or
silently drop withheld records from the population.

Validate the exported history with the existing offline interface:

```sh
rtk proxy momus confidence --report /trusted/current-report.json \
  --history /trusted/outcome-history.json
rtk proxy python3 -m unittest discover -s eval/outcomes -p 'test_*.py'
```

Keep collection-validation runs distinct from real held-out evaluation. The
33 deterministic tests cover source/producer binding, edited and hindsight
captures, explicit unknown telemetry, separate windows, complete negative
surveillance, event-versus-availability chronology, maturity, identity joins,
duplicates/conflicts, malformed inputs, overflow and deterministic export.
They prove mechanics using fabricated inputs and publish no real performance
numbers.

## Remaining acceptance requirements

#17/#51 stay open. The precise external requirements are:

1. A registered maintainer-controlled committed-review collection pipeline,
   trusted immutable premerge report/capture timestamps and exact head/base
   identity. The 21 discovered archived reports require individual qualification;
   the inspected PR #40 report fails the committed-object capture requirement.
2. Authoritative, independently attributed revert, incident and flake evidence
   from separate registered telemetry processes, including complete verified
   negative surveillance and actual label-availability times. Such exports
   were not supplied in this audit; GitHub search/CI success cannot replace them.
3. Mature windows and the preregistered chronological holdout, with sufficient
   outcome classes and score-bin support. The engine requires at least 40
   training labels, 20 held-out labels, both training outcome classes, and 20
   training plus 20 held-out labels in the candidate's bin, separately for
   every outcome. These minima do not establish calibration by themselves.
4. Measured real-data held-out results, uncertainty/baseline comparisons,
   inclusion/unknown/withholding counts, source-owner validation and drift
   monitoring before any empirical acceptance claim. Automatic approval
   additionally needs all existing fail-closed identity/completeness/freshness
   gates; this exporter grants no approval.

Prospective collection/export machinery is independently deliverable. Local
checks, synthetic tests or a merged collector PR do not satisfy those external
evidence requirements or authorize closing #17/#51.
