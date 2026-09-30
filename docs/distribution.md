# Distribution

Goal: one binary with no runtime, a real install path, and CI gating on pull
requests.

## Install

- **Prebuilt binaries**: every `v*` release from v0.3.0 on carries, per
  target, `momus-<target>.tar.gz` plus `.sha256`, a CycloneDX 1.5 SBOM
  (`momus-<target>.cdx.json`) and signed provenance/SBOM attestation
  bundles for `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, and
  `aarch64-apple-darwin` (`.github/workflows/release.yml`). Linux builds
  link against Ubuntu 22.04's glibc so they run on older hosts too.
  Verify a download with `scripts/verify-release.sh` (docs/slsa.md);
  releases before v0.3.0 have checksums only.
- **From source**: `cargo install --locked --git https://github.com/brianluby/momus-review`
  (binary `momus`); `cargo run -q --bin momus --` for development. The
  exact toolchain is pinned by `rust-toolchain.toml`.
- No runtime beyond the binary: `momus review ~/repos/anything` just works.
- Config comes from the environment (`TYPESAFE_API_KEY`, …; see README
  "Configuration"). The report defaults to `./reviews/latest.json` in the
  working directory (`MOMUS_REPORT` overrides it), never the install
  directory.

## CI

- **GitHub Action** (`action.yml` at the repo root): on `pull_request`,
  downloads the release binary for the runner, verifies its checksum and
  (for v0.3.0+ releases, by default) its provenance and SBOM attestations
  against the selected tag's source commit, runs `momus review --base <PR
  base sha>`, optionally uploads SARIF to code
  scanning, then `momus github-review` posts inline comments and a sticky
  summary. With no prebuilt binary (or `version: source`) it builds the
  action's own checkout with `cargo install --locked`. Usage: README "CI".
- **Any CI**: `momus review --base origin/main --fail-on-blocking` is the
  exit-code contract; `--sarif <path>` writes SARIF 2.1.0 for other
  uploaders.
- **Many repositories**: `.github/workflows/review.yml` is a reusable
  workflow (`workflow_call`); each repository carries only the caller in
  `examples/momus.yml`, pinned to a release commit SHA (`version: auto`
  pins the binary to it too), and passes the `TYPESAFE_API_KEY` secret
  explicitly. `scripts/rollout.sh` sets the secret and opens the pull
  request that adds the pinned caller and, where there is none, a
  github-actions Dependabot config that proposes upgrades, for a list of
  repositories (dry run by default; `--update` refreshes open rollout PRs). Personal accounts have no account-wide
  Actions secrets, so the script sets one per repository; in an
  organization, an org secret shared with selected repositories replaces
  that step.
- This repository reviews its own pull requests with the action
  (`.github/workflows/momus.yml`, `version: source`, advisory only).

## Releasing

1. Bump `version` in `Cargo.toml` (and `Cargo.lock`), merge to `main`.
2. Rehearse: `workflow_dispatch` the release workflow on `main`. It runs
   the full chain — audit, all three targets, SBOM generation and
   validation, Apple signing/notarization, attestations and consumer
   verification — without publishing or moving tags. A release is cut only
   after a passing rehearsal and review.
3. Tag and push: `git tag v0.3.0 && git push origin v0.3.0`. The workflow
   refuses a tag that does not match `Cargo.toml`, refuses to overwrite an
   existing release, requires immutable releases, verifies the draft as a
   consumer before publishing, and moves the major tag (`v0`) only after
   verified publication. The full per-release inventory is in docs/slsa.md.

## Package names

`momus-review` was free on crates.io and npm at check time (2026-09-25). The
product ships as a Rust binary; publishing the crate (`cargo install
momus-review`, `cargo binstall`) is a later step.
