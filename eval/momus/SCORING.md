# Stored evidence and issue scoring

The scorer runs offline. It does not call a model, run fixture commands, train
anything, or infer a detection from a filename or mechanism category. Python
3.9+ and Git are required. Git applies each exact diff to a disposable base
copy; the resulting content-tree identity must equal the declared head.
Paths, symlinks, schemas, JSON types, duplicate keys/identifiers, finite numbers
and source/report/receipt hashes are checked before scoring. A validation
failure exits with status 2 and produces no result.

```sh
python3 eval/momus/evaluate.py validate
python3 -m unittest discover -s eval/momus -p 'test_*.py'
python3 eval/momus/evaluate.py prepare --output /tmp/momus-evidence
```

`prepare` creates a full inventory of missing cases and pending independent
adjudications. It performs no review and refuses a nonempty output directory.
After a separately authorized measured run and independent adjudication:

```sh
python3 eval/momus/evaluate.py score \
  --run /tmp/momus-evidence/run.json \
  --adjudications /tmp/momus-evidence/adjudications.json \
  --split holdout --slice core --output /tmp/momus-evidence/core-results.json
python3 eval/momus/evaluate.py score \
  --run /tmp/momus-evidence/run.json \
  --adjudications /tmp/momus-evidence/adjudications.json \
  --split holdout --slice auxiliary --output /tmp/momus-evidence/auxiliary-results.json
```

`--manifest` overrides the product manifest on all commands. Score defaults to
`--split holdout --slice all`; development controls must be selected explicitly
and their results must remain separate from held-out results. The test API uses
`require_inventory=False` only for the five-case unit corpus under `stored/`.
The CLI always enforces the product minimum inventory. The stored reports,
receipts, adjudications and exact expected result are synthetic scorer tests,
labelled `offline-test`, outside the product corpus. Their numeric assertions
are not product performance measurements; their padded Git identities do not
represent real source reviews.

## Run receipt

`run.json` has exactly `schemaVersion: 1`, `datasetVersion`, `manifestSha256`,
`runId`, `kind` (`measured` or `offline-test`) and `cases`. The manifest hash is
SHA-256 over the exact manifest bytes. Each case entry has exactly:

| Field | Meaning |
|---|---|
| `caseId` | Exact manifest case identifier. Duplicate/unknown cases are errors. |
| `status` | `complete`, `partial`, `failed` or `missing`. |
| `report`, `reportSha256` | Native Momus JSON report path and exact byte SHA-256; both null if absent. |
| `receipt`, `receiptSha256` | Materialization receipt path and exact byte SHA-256; both null if no report. |
| `error` | Null for `complete`; concrete nonempty reason for all other statuses. |
| `abstention` | Null, or `{ "abstained": boolean, "rationale": string }`, recording an actual review response. |

Evidence paths are normalized POSIX paths relative to the run file's directory.
They may not escape that directory or traverse symlinks. Copy materialization
receipts and reports into that evidence directory before referencing them.
Absent manifest cases are explicitly reported as missing and retain their full
issue/case denominators. A missing report cannot establish an abstention.

Materialization receipts have exactly `schemaVersion: 1`, `datasetVersion`,
`manifestSha256`, `caseId`, `source` (`baseSha256`, `headSha256`, `diffSha256`),
`reviewedBase` and `reviewedHead`. Git identities are full lowercase 40-digit
commits. Source hashes must equal the validated manifest, and native report
`reviewedBase`/`reviewedHead` must equal this exact receipt. Hash bindings detect
changed evidence; the trusted materializer establishes the original mapping
from the content trees to Git commits. A handwritten receipt is not proof that
the review received those trees.

## Native review completeness

Run `momus review .` inside each sanitized materialized repository. Scope must
be exactly `.` and native mode must be `changes`. Use committed source review
for the core slice and the dependency caller/API slice. The scorer requires
`reviewedClean` and `reviewedCommitted`, the full five-dimension matrix for every
changed head source file, matching screening counters and complete changed test
context. Rust, Python and JavaScript/TypeScript source/test conventions match
the product's `src/domain/language.rs`; deleted files are not head review
subjects. Source coverage is derived independently from base/head tree bytes,
so a selective report cannot claim a complete case by setting `partial: false`.

Docs uses the supported local `--docs-drift` checker with the exact clean
materialized working tree and receipt-bound base/head identities. Its native
`docsDrift.checks` must be present and nonempty. Because current auxiliary flags
do not support committed-only mode, `reviewedCommitted: false` is permitted only
for this docs protocol. This trusts the clean pinned worktree attestation and
materialization receipt; it does not establish committed-only review semantics.

Dependency defects concern concrete changed APIs and visible caller contracts.
Score the committed source review; an optional local `--upgrade-triage` report
is separate advisory evidence. A generic major-upgrade advisory is not detection
of the labelled API/caller defect. The local upgrade checker may report
unverified intervening releases/downstream usage even when exact target notes
are supplied. Attaching that incomplete artifact to the scored report makes
the report partial and earns zero detection credit.

Native partial reports, skipped requests, budget deferrals, unmerged shards,
dismissed subjects, incomplete follow-ups, absent source verification and
auxiliary unknowns are recorded as incomplete. No finding from an incomplete
case earns issue credit, even if the run receipt declares it complete. Source
annotations still `pending` also earn zero credit. A complete report with any
unadjudicated finding is incomplete for issue scoring.

## Independent finding adjudications

`adjudications.json` has exactly `schemaVersion: 1`, `datasetVersion`,
`manifestSha256`, `runSha256`, `reviewer` and `cases`. `runSha256` is the exact
run receipt hash; update it only after the receipt is final. `reviewer` has
`identity`, `independent: true`, and `method` (`human-source-review` or
`offline-test`). Measured runs reject `offline-test`. The generated draft has
`independent: false` and cannot score until a real independent source review
has happened. Model output, candidate labels and bot comments are not
independent ground truth. Do not copy evaluator labels into model requests.

Each adjudicated case has `caseId`, `reportSha256`, `findings` and `abstention`.
Each finding judgment names the exact zero-based native finding `index`, a
`verdict`, `issueId`, `duplicateOf`, `rationale` and `explanation`:

| Verdict | Required interpretation |
|---|---|
| `matched` | The actual finding identifies the known issue, with source evidence of its trigger, violated contract and consequence. `issueId` names that issue and `duplicateOf` is null. |
| `duplicate` | Another finding already identifies the same root cause. `issueId` is the same issue and `duplicateOf` names an earlier `matched` index. |
| `unrelated` | The finding claims a different defect absent from the exhaustive authored case labels; counted as a false alarm even in defect cases. Both identity fields are null. |
| `unsupported` | The source does not support the finding's concrete defect claim; counted as a false alarm. Both identity fields are null. |
| `advisory` | A source-correct observation or explicit request for more evidence that does not assert a concrete defect. Counted separately; never a detection or a blanket exemption for false defect claims. Both identity fields are null. |

`rationale` has nonempty `trigger`, `contract` and `consequence` strings. The
reviewer must explain how the **actual report text/evidence**, at this index,
establishes or fails the labelled issue, citing the pinned source. Merely
repeating oracle text while the finding says something else is invalid
adjudication. The evaluator checks the binding and structure; the named
independent reviewer is responsible for the semantic judgment. Matching file,
dimension and line is necessary, never sufficient. A claimed match with the
wrong exact source location/dimension or empty finding evidence becomes a
false alarm with an explicit `invalidMatchIndices` entry.

`explanation` has independent boolean assessments `sourceCorrectness`,
`specificity`, `actionability`, `uncertainty`, plus a nonempty `rationale`.
Matched, duplicate and advisory verdicts require source correctness.
Specificity, actionability and uncertainty remain separate from detection:
recognizing a defect does not establish a useful fix or a well-bounded claim.
Quality results retain their exact assessed-finding denominators.

Case `abstention` is null or `{ "appropriate": boolean, "rationale": string }`.
Appropriate abstention needs an actual recorded abstention, no findings, validated
missing-evidence labels, verified source identity and a complete review attempt.
A native report partial only because evidence is unavailable may earn
abstention success while remaining incomplete for detection. Failed, missing,
budget/skipped or incomplete-coverage attempts cannot earn abstention success.
An unexpected abstention on a defect/clean case is explicit and does not claim
a completed review.

## Results

Every selected case is present in output, including missed issues, failures,
missing inputs, withheld matches, unadjudicated indices and invalid matches.
An issue earns at most one detection. Additional findings for it are duplicate
noise even if mistakenly adjudicated as a second match. Unrelated findings in
positive cases remain false alarms. Valid advisories are separately counted.

Issue recall keeps **all labelled issues** in its denominator: incomplete cases
receive zero credit. `fullyEvaluated` and completion counts must accompany that
conservative recall. Clean-case false-alarm and abstention rates are null when
any corresponding attempt is incomplete; the observed counts and full case
denominators remain visible. Zero-denominator rates are also null. The scorer
does not infer true negatives from missing telemetry, produce a precision
claim from unadjudicated findings, or claim benchmark-wide performance from
the stored test reports.
