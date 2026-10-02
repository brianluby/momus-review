# Authored corpus inventory

Dataset `momus-owned-v1` contains 34 authored holdout cases: 13 core
defect/fixed pairs, four bounded-evidence abstention cases, and two additional
auxiliary pairs. These are authored evaluation examples, not observed
production defects. The integrating agent may append exposed PR #40 controls
with `split: development`; those controls never enter the held-out denominator.

The source of truth for each independent review is its `annotation` field in
`manifest.json`. Authorship and successful reproduction alone leave that field
pending. A reviewer other than the corpus author must inspect the source,
contracts, locations, paired controls and abstention limits before changing it
to validated. Bot comments are not annotation evidence.

| Pair | Language | Core dimension | Contract and concrete trigger |
| --- | --- | --- | --- |
| `rust-invoice` | Rust | correctness | A 2500 basis-point discount on 2000 cents must produce 1500, rather than truncate the fractional discount to zero. |
| `rust-authority` | Rust | security | An authenticated viewer cannot gain deletion authority by supplying the request's display role `admin`. |
| `rust-backoff` | Rust | reliability | A restored retry counter of 64 must produce the capped delay without a shift panic. |
| `rust-wire` | Rust | compatibility | Adding trace metadata must retain the existing `state` JSON key consumed by the health caller. |
| `js-tenant` | JavaScript | security | Tenant red cannot update id 9 belonging only to tenant blue. |
| `js-drain` | JavaScript | reliability | Draining a three-job mutable queue must dispatch all three and leave no job behind. |
| `js-async-lease` | JavaScript | reliability | A lease remains active until the send promise resolves or rejects; `finally` must run after awaited completion. |
| `js-pagination` | JavaScript | compatibility | Adding `hasMore` must retain `nextCursor`, which the unchanged collector reads. |
| `js-refund-tests` | JavaScript | testGap | Removing the repeat-key guard must fail the regression suite; acceptance-only tests let duplicate credit survive. |
| `python-cents` | Python | correctness | Importing decimal `0.29` must produce exactly 29 cents, rather than 28 after float truncation. |
| `python-export` | Python | security | Nested downloads must reject `../private.csv` because normalization alone does not confine a path. |
| `python-config` | Python | compatibility | Existing `timeout: 2` settings must remain two seconds when `timeout_ms` is absent. |
| `python-signature-tests` | Python | testGap | An always-accept verifier mutation must fail negative signature tests; valid-signature-only assertions miss it. |

| Auxiliary pair | Supported slice | Concrete contract |
| --- | --- | --- |
| `docs-client-arity` | Source-linked Rust docs examples | The quickstart must pass the newly required retries argument to the current public function. |
| `dependency-codec-api` | npm package metadata plus exact local release notes | The authored v2 dependency removes `encode`; the visible caller must migrate to `stringify`. |

Auxiliary scoring remains separate. In the dependency fixed control, the major
version upgrade is still visible while the caller has migrated. A generic
major-version review advisory is not evidence that the caller remains broken;
issue-level credit requires the exact downstream trigger, contract and
consequence. The dependency is a local authored stand-in and requires no
registry installation, third-party license inference or network access.

| Insufficient-evidence case | Dimension | Evidence required before a concrete finding |
| --- | --- | --- |
| `rust-macro-unknown` | security | The missing private authorization macro expansion and storage caller contract. |
| `js-transport-unknown` | reliability | The external SDK's actual version and acknowledgment/durability semantics. |
| `python-dependency-unknown` | compatibility | Exact dependency source and target/intervening release notes. |
| `rust-external-tests-unknown` | testGap | The excluded external integration-test repository and coverage inventory. |

Every pair shares a base snapshot and has distinct defect and fixed heads with
nonempty base/head diffs. The manifest binds each tree and exact diff to SHA-256.
Each concrete issue records a current source location, trigger, violated
contract, consequence, acceptable finding, fix and separate explanation
criteria. Labels, this inventory and adjudications are outside the materialized
review workspace and must never enter prompts, prompt tuning or any training
dataset.

Run the offline behavioral controls from the repository root:

```sh
rtk proxy python3 -B eval/momus/reproduce.py --all
rtk proxy python3 -B eval/momus/reproduce.py --case rust-invoice-defect --case rust-invoice-fixed
```

The controls use local `rustc`, Node.js and Python's standard library. Rust
source is compiled directly without Cargo dependencies. Test-gap controls
first verify the correct implementation, then mutate a temporary copy: the
defect tests allow the contract-violating mutation and the fixed tests reject
it. Documentation controls compile the exact README excerpt with its documented
caller import supplied by the harness. No probe
executes a command taken from a manifest, downloads a dependency, calls a model
or measures product detection performance.

`generate.py` deterministically reconstructs authored source and content
identities. It preserves validated annotations only when the case's source
identity and oracle metadata remain unchanged, and preserves separately added development
controls. Re-running it after modifying fixtures discards those source edits;
corpus changes require updating the generator and a new independent review.
