# Independent source annotation review

Reviewer: `independent-source-review:outcome_evidence`, a separate Codex agent
from the fixture author and scorer author. Reviewed on 2026-10-02. This is an
agent source review, not human sign-off, bot-comment ground truth, blind model
evaluation or evidence of observed production defects.

All 36 annotations are validated against the supplied bounded source contracts:
13 core defect/fixed pairs, two auxiliary defect/fixed pairs, four insufficient
evidence cases and two exposed development controls. The reviewer read the
base/head code, callers, README contracts, issue locations and acceptable-finding
requirements. The clean labels mean the specific annotated defect is absent
within those explicit input contracts; they do not certify arbitrary external
callers, inputs or deployment conditions.

The SHA-256 of reviewed source identities is
`3450733037467f330fd63318dd2dd6315b2ddb6ed8f4e0cdafa8d681170cd3c4`.
Its input is compact sorted-key UTF-8 JSON of cases sorted by `id`, each with
exactly `id`, `baseSha256`, `headSha256`, `diffSha256` copied from the manifest.
Annotation metadata is excluded, so marking the completed review does not
change that identity. Source changes require another review.

## Paired source rationales

Each row covers both `-defect` and `-fixed` cases. File/line references name
the defect head; fixed-control details were separately read and reproduced.
Findings receive issue credit only when they establish the described trigger,
violated contract and consequence, not merely the named file or dimension.

| Pair | Source-grounded defect and acceptable finding | Verified fixed control and boundary |
| --- | --- | --- |
| `rust-invoice` | `src/invoice.rs:3` evaluates `2500 / 10000` first, giving zero discount for a 2000-cent checkout and a 500-cent overcharge. README explicitly requires subtracting the floored discount amount, including 2001 cents yielding 1501. An acceptable finding explains the truncating division order and the incorrect checkout total. | Multiply in `u128` before division, then subtract the floored discount. Products of any `u64` cents and 0..10000 basis points fit `u128`; the quotient fits `u64` and cannot exceed cents. The oracle checks 2000, 2001 and maximum-u64 zero/full discounts, distinguishing this policy from flooring the payable amount. |
| `rust-authority` | `src/auth.rs:3` ORs authenticated permission with the caller's `request_role == admin`. A viewer sending that untrusted display preference gains deletion permission despite the README's sole-authority contract. The finding must distinguish the two authorities and trace the OR branch. | The fixed branch checks only `Actor.can_manage`; the same viewer/admin trigger is denied. This validates the explicit permission function rather than inventing an absent HTTP authentication implementation. |
| `rust-backoff` | `src/retry.rs:2` shifts `1u64` by a restored attempt of 64 before applying `min`. With overflow checks, the function panics instead of returning the README's bounded delay. The finding must name the overflow-before-cap trigger; source does not establish job loss or persistence behavior. | Saturating exponentiation and checked multiplication return at most 10000 for all `u32` attempts, including the reproduced 64 trigger. This review narrowed the consequence to aborted delay calculation instead of unsupported job-loss claims. |
| `rust-wire` | `src/wire.rs:2` replaces the stable `state` key with `status`; the visible health client still searches for `state: ready`. The finding must identify that missing key and client impact, not flag additive `trace` as incompatible. | Retains `state` while adding `trace`; client assertions pass. README limits trace identifiers to lowercase ASCII/digits/hyphens, so arbitrary JSON-string injection is outside this fixture's supplied input contract. |
| `js-tenant` | `notes.js:2` looks up id alone before mutating the title. Tenant red's id 9 request matches tenant blue's row, violating README's tenant-plus-id scope. The finding must connect the lookup to unauthorized mutation. | Matching both tenant and id rejects that request and leaves blue's row unchanged. The fixture exercises the scoped update function, not an unspecified external authorization layer. |
| `js-drain` | `queue.js:3` increments the loop index while `shift()` shrinks queue length. Three jobs produce only two dispatched jobs and leave `c` in the owned queue. The finding must explain that interaction and bounded queue consequence. | `while (queue.length)` returns each queued job in order and empties the queue, verified on the same three-job trigger. |
| `js-pagination` | `catalog.js:3` returns `next` instead of the stable `nextCursor`. The unchanged collector reads `nextCursor` and gets `undefined` instead of 2 for four rows. The finding must tie the removed property to that visible caller. | Returns `nextCursor: next` plus additive `hasMore`; the legacy cursor result and new field both pass the probe. |
| `js-refund-tests` | `tests.js:4` tests only a first refund; removing `ledger.js`'s repeated-key guard leaves that suite green while repeat credits become possible. The explicit public test contract requires repeat-key rejection. A valid finding reports a missing assertion and surviving mutation; the current implementation's guard is correct. | Calls refund twice and checks second result `false` plus unchanged balance. Original tests pass; deleting the guard in a temporary copy is rejected. This is a test-gap label, not a current production double-credit defect. |
| `python-cents` | `prices.py:2` converts `0.29` to binary float before multiplying/truncating, yielding 28 rather than 29 cents through the importer. A valid finding gives that concrete undercount, not a numerical-style complaint. | Decimal multiplication preserves cents for the README's nonnegative, at-most-two-decimal inputs through 9999999.99. Probe also checks zero and the maximum. Unbounded Decimal precision/exponent claims are not made. |
| `python-export` | `exports.py:2` resolves the HTTP-supplied `../private.csv` but never checks containment. The visible handler therefore returns a path outside `/exports`. A valid finding traces query input and distinguishes normalization from confinement. | Resolves root and target, then requires `target.relative_to(root)`, rejecting the traversal with `ValueError` while accepting nested export paths. The provided code selects paths; it does not establish how an absent later file-open implementation handles concurrent filesystem changes. |
| `python-config` | `config.py:2` ignores the still-supported legacy `timeout` key, returning 10 for the unchanged `timeout: 2` sample used by the scheduler. A valid finding names that key/caller/fallback regression. | Explicit `timeout_ms` takes precedence; otherwise the original seconds lookup is preserved. Probe verifies legacy 2 seconds and new 2500 ms as 2.5 seconds. |
| `python-signature-tests` | `tests.py:4` asserts only a valid signature. Replacing the actual correct HMAC comparison with `return True` survives the suite, contrary to the explicit test contract for tampered payloads and mismatched signatures. Valid findings report those omitted rejection assertions, not a bypass in the current verifier. | Retains the correct verifier and adds both negative assertions. Original tests pass; the temporary always-accept mutant is rejected. This is a regression-test gap only. |
| `js-async-lease` | `forward.js:3` returns the send promise inside `try/finally`, so `finally` releases the lease before the send resumes after its microtask yield. An otherwise valid worker send rejects with the released-lease error. A valid finding explains adoption of the returned promise and early `finally` execution. | `return await` delays `finally` until settlement. Probe verifies the successful worker send and that lease release occurs after both success and an independently induced transport rejection. The microtask yield is deterministic; no timing sleeps are used. |
| `docs-client-arity` | `README.md:7` calls source-linked `connect` with one argument after `src/client.rs` adds required retries. The README explicitly identifies the import context. A valid docs finding ties that link/signature to the arity mismatch; the defect produces Rust `E0061`. | The excerpt passes retries and compiles/executes with the same documented import prelude supplied by the harness. The unqualified call lies within the existing supported docs recognizer; the fixture is an auxiliary docs slice. |
| `dependency-codec-api` | Changed `vendor/codec-fixture/index.js:1` removes `encode` and supplies v2 `stringify` while unchanged `app.js:2` still calls `codec.encode`. The exact target notes and package pin identify that migration; the caller raises the expected TypeError. Primary location identifies the changed interface; valid credit also requires the concrete downstream caller and exact version/API evidence. A generic major-version advisory alone is insufficient. | Migrates the caller to `codec.stringify`; the output regression passes. This is an authored local stand-in, not a claim about a registry package. The generic upgrade analyzer can retain its separate downstream uncertainty; committed source review establishes this concrete auxiliary compatibility issue. |

## Insufficient evidence and development controls

| Case | Reviewed abstention/control rationale |
| --- | --- |
| `rust-macro-unknown` | Only the private authorization macro invocation and changed storage call are visible. Macro expansion, storage behavior and the authorization caller contract are absent. The source cannot establish an authorization bypass; request the missing implementation instead. |
| `js-transport-unknown` | The caller changes options from `ack` to `mode: durable`, but no SDK version, source or acknowledgment/durability contract is supplied. Data loss cannot be inferred from option names alone. |
| `python-dependency-unknown` | A private package pin changes 1.7.0 to 1.8.0 while `analyze(text)` remains the caller. Target/intervening notes and resolved source are absent; an API-removal finding would invent evidence. |
| `rust-external-tests-unknown` | The new ratio implementation handles denominator zero, and the separate integration-test repository/coverage inventory is explicitly omitted. Recommend checking that coverage, but do not assert a missing external test. |
| `pr40-utf8-hardening` | The public Git adapter intentionally makes invalid UTF-8 evidence fail instead of replacing bytes and prevents unreadable prior source from being treated as an addition. Diagnostic stderr may remain lossy. This is evidence-integrity hardening, supported by source/error paths and the documented regression test, not an observed defect label. |
| `pr40-auxiliary-identity` | The newly added auxiliary attachment preserves the source review head, marks identity/completeness unverified on mismatch or missing source identity, and exposes unknowns. It cannot fill/overwrite the authoritative head. Its located regression assertions verify those two boundaries; this is intentional hardening. |

The PR #40 controls are development-only. The selected source bytes were
independently compared against local immutable Git objects at base
`d64edff1b47982a408ffca4a551d4d83cdc06740` and head
`4bbcd16312774109ca2c147989a032c4401ed627`. Git-adapter base/head bytes match
exactly. The auxiliary module is correctly absent at base and matches the
head object exactly. README context is authored and distinguished from those
verbatim files. These exposed examples cannot earn held-out acceptance credit
and are not imported into prompt tuning or Jeeves training.

## Offline checks and practical limits

```sh
rtk proxy python3 -B eval/momus/reproduce.py --all
rtk proxy python3 -B eval/momus/evaluate.py validate
```

The reviewer independently reran all 30 known behavioral controls after the
final authored source corrections; every annotated defect failed at its
specific trigger, or retained the specified contract-violating mutant, and
every fixed counterpart passed or killed that mutant. Rust probes explicitly
enable overflow checks; the docs control compiles its exact excerpt with the
documented prelude; JS/Python probes use local runtimes without dependencies
or network. The unpaired cases have no claimed behavioral reproducer.

The reviewer also applied each of the 36 stored diffs to an isolated temporary
copy of its base using `rtk proxy git apply`, then compared every resulting
file's exact bytes/path to the declared head tree. All 36 transforms match.
That check found and led to correction of a missing final-newline marker in
the public Git-adapter development diff; original source bytes were preserved.
No repository Git state was changed by that temporary patch check.

Source annotations were refined where necessary before validation: invoice
rounding explicitly subtracts the floored discount; Python decimal inputs are
bounded; backoff consequences do not claim absent persistence evidence;
documentation uses a supported source-linked call; the dependency location
names the changed interface and requires the actual stale caller as evidence;
test-gap labels distinguish correct current
code from undetected later mutations. All declared defect locations and
acceptable findings are supported by the final source. No bot comments were
used to establish any label.

This establishes offline corpus/oracle validity for its stated boundaries.
It supplies no live model detection, precision, calibration or comparative
performance measurements. Measurement and any private-source publication
remain separately authorized work.
