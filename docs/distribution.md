# Distribution

Goal: one binary with no runtime, a real install path, and CI gating on pull
requests.

## Install

- **Prebuilt binaries**: every `v*` release carries
  `momus-<target>.tar.gz` plus a `.sha256` for `x86_64-unknown-linux-gnu`,
  `aarch64-unknown-linux-gnu`, and `aarch64-apple-darwin`
  (`.github/workflows/release.yml`). Linux builds link against Ubuntu
  22.04's glibc so they run on older hosts too.
- **From source**: `cargo install --locked --git https://github.com/brianluby/momus-review`
  (binary `momus`); `cargo run -q --bin momus --` for development.
- No runtime beyond the binary: `momus review ~/repos/anything` just works.
- Config comes from the environment (`TYPESAFE_API_KEY`, …; see README
  "Configuration"). The report defaults to `./reviews/latest.json` in the
  working directory (`MOMUS_REPORT` overrides it), never the install
  directory.

## CI

- **GitHub Action** (`action.yml` at the repo root): on `pull_request`,
  downloads the release binary for the runner and verifies its checksum,
  runs `momus review --base <PR base sha>`, optionally uploads SARIF to code
  scanning, then `momus github-review` posts inline comments and a sticky
  summary. With no prebuilt binary (or `version: source`) it builds the
  action's own checkout with `cargo install --locked`. Usage: README "CI".
- **Any CI**: `momus review --base origin/main --fail-on-blocking` is the
  exit-code contract; `--sarif <path>` writes SARIF 2.1.0 for other
  uploaders.
- This repository reviews its own pull requests with the action
  (`.github/workflows/momus.yml`, `version: source`, advisory only).

## Releasing

1. Bump `version` in `Cargo.toml` (and `Cargo.lock`), merge to `main`.
2. Tag and push: `git tag v0.1.0 && git push origin v0.1.0`. The workflow
   refuses a tag that does not match `Cargo.toml`, runs the tests on every
   target, and creates the release with generated notes.
3. `workflow_dispatch` on the release workflow builds the same archives as
   run artifacts without publishing anything, for a dry run.

## Package names

`momus-review` was free on crates.io and npm at check time (2026-09-25). The
product ships as a Rust binary; publishing the crate (`cargo install
momus-review`, `cargo binstall`) is a later step.
