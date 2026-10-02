# Momus-owned product benchmarks

Dataset `momus-owned-v1` is an offline, source-reviewed evaluation corpus beside
the existing [Juice Shop evaluator](../README.md). No live model campaign has
been run and no product performance results are published. The
[results template](results-template.md) remains blank.

The manifest contains 36 cases:

| Slice | Defect/fixed pairs | Insufficient evidence | Total |
|---|---|---|---|
| Authored core holdout | 13 pairs / 26 cases | 4 cases | 30 |
| Authored docs/dependency holdout | 2 pairs / 4 cases | — | 4 |
| Public PR #40 development controls | — | — | 2 |

Core cases span Rust, JavaScript and Python and all five Momus dimensions:
correctness, security, reliability, compatibility and test gaps. They include
integer rounding, server-side authority, restored retry counters, stable wire
keys, tenant boundaries, queue draining, async lease lifetime, pagination,
decimal amounts, path containment, configuration compatibility, and meaningful
idempotency/signature rejection tests. Test-gap defects describe missing
regression protection in correct current code; mutation probes demonstrate the
gap without claiming a current authorization bypass.

The four deliberately insufficient inputs omit a generated authorization
implementation, external transport semantics, resolved dependency/release
evidence, or an external test inventory. Their acceptable behavior is explicit
uncertainty and a request for the missing evidence.

Every paired case has realistic callers/tests and a repository contract,
nonempty base/head changes, exact tree/diff SHA-256 identities, issue-level
trigger/contract/consequence/locations and acceptable explanation/fix criteria.
Both buggy and fixed heads change from the baseline. The
[annotation inventory](ANNOTATIONS.md) and [independent source review](annotation-review.md)
record validation and limits. Independent agent source review and reproducers
validate authored annotations; bot comments are not ground truth, and no human
or production-outcome validation is implied.

## Offline validation

Python uses only its standard library. Behavioral probes also require Node.js
and `rustc`; Cargo fixtures require no downloaded dependencies. These commands
call no model or remote service:

```sh
rtk proxy python3 -B eval/momus/evaluate.py validate
rtk proxy python3 -B eval/momus/test_evaluate.py
rtk proxy python3 -B eval/momus/test_materialize.py
rtk proxy python3 -B eval/momus/reproduce.py --all
rtk cargo test --locked --offline --test product_fixtures
```

`reproduce.py` dispatches only code-owned known fixture probes and never executes
an arbitrary manifest command. Expected assertion/compile errors establish the
defect, while fixed controls must pass. Test-gap probes require the unmutated
tests to pass, then show the contract-violating mutant survives the deficient
suite and is rejected by its fixed pair. The runner fails on absent tools,
unexpected failure modes or a failing fixed control.

Regenerate authored bytes with `rtk proxy python3 -B eval/momus/generate.py`.
Unchanged annotations are retained; changed source or oracle metadata invalidates
prior validation. `development_controls.py` copies public source files verbatim
from pinned PR #40 merge/base Git objects, and adds authored README contract
context. Its refresh requires those exact objects in the local repository.
Refreshing those exposed examples never promotes them to holdout cases.

## Scoring and boundaries

See [the scoring contract](SCORING.md) for exact run/adjudication schemas.
Raw Momus reports stay in their native shape. A separate independent
adjudication binds each exact report hash and finding index to a known issue,
with source-grounded trigger, violated contract and consequence. Source path,
line and dimension also must match. Same-file/category/line agreement alone
earns no detection credit. Each issue can earn credit once; duplicates,
unrelated findings, source-correct non-defect advisories and unsupported claims
are counted separately. Explanation source correctness, specificity,
actionability and uncertainty are assessed independently.

Missing, failed, budget-deferred, skipped, unmerged-shard and partial reviews
retain full scheduled denominators and zero detection credit. Explicit,
appropriate abstentions are accounted for separately from defect detection;
silence is not a verified abstention or clean pass. Incomplete clean-case
coverage cannot produce a clean false-alarm-rate claim. Scorer test reports are
fabricated mechanics fixtures and must never populate a product-results report.

Docs comparisons use Momus's supported source-linked unqualified Rust calls
and fixed one-line public signatures. The local docs analyzer verifies the
bad arity and the corrected control without inference. Dependency cases use an
authored, locally vendored API stand-in and exact target release notes. Momus's
upgrade analyzer emits a legitimate major-version advisory for both heads and
retains its target-release/downstream uncertainty. That advisory alone is never
credited as the caller defect. The concrete caller is evaluated in a separate
committed source review; optional upgrade artifacts remain separate evidence.

All 34 authored holdout cases are evaluation-only and may not be used for
prompt tuning, threshold selection or any Jeeves training/adaptation data.
Labels, adjudications, mutation probes and annotation receipts are evaluator
inputs and must stay outside model review inputs. Two public PR #40 controls
are already exposed development/demo material; their intentional UTF-8 and
auxiliary-identity hardening changes never count as held-out evidence. Dataset
v1 contains no private-source examples. Publishing private examples requires
separate authorization and a separately versioned, reviewed corpus.

## Prepare a future scheduled evaluation

Preparation is offline. The following generates a blank inventory, then creates
one clean Git workspace containing only source inputs. Receipts stay outside
that workspace. All output paths must be new; existing work is preserved.

```sh
rtk proxy python3 -B eval/momus/evaluate.py prepare --output /tmp/momus-run
rtk proxy python3 -B eval/momus/materialize.py \
  --case rust-invoice-defect --output /tmp/momus-input-invoice \
  --receipt /tmp/momus-run/receipts/rust-invoice-defect.json
```

The receipt contains exact `reviewedBase`/`reviewedHead` Git identities and
manifest/source hashes. Materialization is deterministic and copies neither
labels nor evaluator metadata into the Git tree. Independent adjudication must
verify those receipts and the actual review scope. Hashes bind consistency;
they do not authenticate an externally supplied report or source assertion.

The following is a documented future run, **not authorization to run a model
campaign now**. Replace the model and base placeholders with immutable verified
identities, pin the Momus binary, record its checksum/configuration and create a
separate workspace/report/receipt for every scheduled case. Preserve failures
and partial output rather than retrying selectively to improve results.

```sh
rtk proxy sh -c 'cd /tmp/momus-input-invoice && exec env \
  TYPESAFE_DEFAULT_MODEL=PINNED_MODEL_REVISION \
  MOMUS_REPORT=/tmp/momus-run/reports/rust-invoice-defect.json \
  MOMUS_INDEX_DIR=/tmp/momus-run/index \
  momus review . --base RECEIPT_BASE_COMMIT \
    --committed-only --no-cache --budget calls=100'
```

Use the same committed source-review command for the dependency caller slice.
For the docs slice, replace `--committed-only` with `--docs-drift --allow-empty`
in its own clean exact-head workspace; Momus currently rejects their
combination. Do not combine `--upgrade-triage` with the scored source review.
Any optional upgrade comparison belongs to a separately retained advisory
artifact, and its unknowns remain explicit.

Fill the prepared run with original report/receipt paths and SHA-256 hashes.
An independent human source reviewer must adjudicate measured findings and
abstentions and bind the finalized run hash. Score core and auxiliary results
separately:

```sh
rtk proxy python3 -B eval/momus/evaluate.py score \
  --run /tmp/momus-run/run.json --adjudications /tmp/momus-run/adjudications.json \
  --split holdout --slice core --output /tmp/momus-run/core-score.json
rtk proxy python3 -B eval/momus/evaluate.py score \
  --run /tmp/momus-run/run.json --adjudications /tmp/momus-run/adjudications.json \
  --split holdout --slice auxiliary --output /tmp/momus-run/auxiliary-score.json
```

The prepared draft has no reviewed cases and asserts no independent review;
scoring rejects it until that evidence is supplied. Copy genuine observations
and exact denominators into the blank template only after the separately
scheduled campaign and adjudication. Authored-case results cannot establish
observed production incidents, general repository precision or calibrated risk.
