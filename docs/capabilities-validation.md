# Remaining review capabilities: delivery evidence

Scope: Veans project 15 #20 upgrade/changelog triage, #21 onboarding tours, #22 docs drift and implementation child #52 of #17 merge confidence. Epic #48 is complete; SLSA L3 is unchanged and deferred.

## Acceptance and limitations

- Upgrade triage supports Cargo/npm manifests and locks with exact matched local changelog evidence. Ordinary/major/pre-1 changes, downgrades, removals, additions, schema migrations and unknown/unsupported evidence have regression fixtures. Other ecosystem and unavailable compatibility evidence remains unknown.
- Docs drift supports evidenced structural Rust public-function/example comparisons, including documentation-only changes, removed/renamed interfaces and stale argument counts. Broader semantic claims and unsupported/ambiguous syntax remain unknown. Findings use existing suppression/publication behavior.
- Tours use bounded source discovery, redaction, indexed signatures, heuristic roles, path components and uniquely resolved active static import edges. Representative Rust/JS layouts and Python entrypoints are tested; Python/dynamic/external import relationships remain unknown.
- Merge outcomes have distinct labels/windows and deterministic chronological evaluation. Missing evidence produces null estimates. Approval is opt-in and requires an immutable committed review plus fresh complete live GitHub evidence. Synthetic and fabricated test contracts exercise the machinery only.
- #17 and child #51 remain open until real provenance-backed repository outcome data, mature observation windows and candidate-bin chronological held-out results support calibration. Missing incident/flake telemetry cannot be labeled as negative.

## Local validation

- `RUSTC_WRAPPER= cargo test --offline --locked --all-targets --all-features --quiet`: 315 tests passed (258 library, 1 evaluation binary, 17 CLI, 23 Git, 11 publication and 5 robustness integration tests). Loopback fixture servers required execution outside the sandbox.
- `RUSTC_WRAPPER= cargo clippy --offline --locked --all-targets --all-features -- -D warnings`: passed.
- `RUSTC_WRAPPER= cargo build --offline --locked --release`: passed.
- Scoped `rustfmt --check --edition 2024 --config skip_children=true` on changed Rust files and `git diff --check`: passed. Unrelated baseline formatter churn was byte-verified and removed.
- Installer: 12 tests; publication/workflow: 15 tests; SBOM: 10 tests with pinned jsonschema 4.25.0; release verifier: 16 cases; rollout fixtures: passed.
- Actual composite-action argument test: all eight independent opt-in/current-head combinations, runner shell semantics and literal shell-injection-shaped arguments passed, including system Bash 3.2. Dashboard: 11 rendering regressions passed, including serializer-shaped evidence, unknown outcomes, untrusted evidence as text and list limits.
- Shellcheck and Actionlint passed. Synthetic JSON regeneration matched committed bytes exactly.

Hosted exact-head checks, external review dispositions and merged commit evidence are recorded in the PR and Veans closeout comments. This document does not claim hosted validation before those results exist.

## Review remediation

Copilot's version-rendering finding and CodeRabbit's seven actionable findings
plus one performance nitpick were validated and fixed. Regressions cover exact
dotted import identity, indexed resolution, unreadable binary baselines,
conditional attributes, meaningful tour completeness and historical-only
removal citations. Detailed replies and resolution evidence live on PR #40.

Local OCR reviewed the first implementation commit
`7a6b4690e1802a50bd2b29e7da3eb2073eb14fa4` in session
`1c08a612-d34f-44bc-9d5d-a47a1365db93`: 34 selected items completed, zero
failed or waived, with 84 candidate comments. Three model timeouts and malformed
context-search requests recovered. This was a candidate-discovery run, not a
clean review of the later remediation commit. Its source-validated dispositions
are recorded in [review-dispositions.md](review-dispositions.md).
