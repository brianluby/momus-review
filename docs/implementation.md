# Implementation Notes

`momus-review` crate, `momus` binary: a from-scratch Rust implementation.
The staged-funnel design was informed by the `jev-review` project (see
README "Design Lineage"); no code was taken from it. This page records the
build decisions and what is still open.

## A thin HTTP client, not an SDK

The Jev API is plain HTTP, so momus speaks the `POST /v1/systemone` wire
format directly through a thin client (`src/review/typesafe.rs`): judgment
questions go out as structured JSON, and `choice`/`noul`/`score` answers come
back through small accessors. The `jev_sdk` crate (0.1.0) could not express
those questions: its criteria are string-only, while momus sends structured
criteria (`{ what, examples }`, `not_for`) and instruction hints
(`inspect`/`focus`/`ignore`/`compare`/`caution`). It is not a dependency;
details in `docs/rust-types.md`.

Retries (transient `429`/`529`/`5xx` and connection/timeout errors, with
backoff), timeouts, usage metering, and secret redaction all live in that one
client, the single egress point.

## How it was built

1. **Domain first**: `policy.rs`, `report.rs`, `patch.rs` as pure `serde`
   types and functions, with wire-shape unit tests.
2. **Bottom-up**: `domain → adapters → review → cli/dashboard`, each layer
   tested before the next, and the whole run checked against the live API:
   `momus review` and `momus scan` produce reports the dashboard reads.
3. **Dashboard**: an axum router serving embedded `public/` assets plus the
   `/api/*` endpoints, with Host check and CSP as middleware (good host 200,
   foreign host 403, CSP present).
4. **Hardening**: the invariants in `docs/security.md` (`OpenOptionsExt`
   flags, `O_EXCL` temps, fail-loud errnos) are implemented and tested.
5. **CI**: `--base` PR-range review (#26), the `github-review` publisher
   (#27), and the Action plus release binaries (#28).

## Status of idiomatic upgrades

- `thiserror` error types replacing string errors: *pending* (`anyhow`
  throughout; `ApiStatusError` is the one typed error, for GitHub 422s).
- `gix` instead of shelling out to git: *pending* (`Command`, no shell).
- Retry policy: *done* (see above). Per-run cost metering: *done* (`usage`
  in every report: successful calls plus server-reported token totals).
  Latency metering: *pending*.
- `clap` subcommands: *done*: `momus review [paths…]` (with `--base`),
  `momus scan [paths…]`, `momus github-review`, `momus dashboard`, plus the
  `momus-eval` golden-set harness.

## Risks

- Prompt-state serialization: the nested `state` each judgment sends is
  built by hand with `json!`; wire-shape tests pin the report, not every
  request.
- `public/app.js` duplicates `THRESHOLD`/`SEVERITY_MAX` from
  `domain/policy.rs`; keep the duplication explicit, or serve the values
  from one source.
