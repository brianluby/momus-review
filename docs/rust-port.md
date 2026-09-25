# Rust Port Plan

Target: `momus-review` crate, `momus` binary. The TS prototype is ~900 lines
of real logic (`review/` 660 + `adapters/` 250 + `domain/`/`cli/`) plus a
1,300-line dashboard frontend that ports byte-identical. Realistic: 2–4
focused days, mostly mechanical. Blocker removed: [`jev_sdk` exists on
crates.io](https://docs.rs/jev_sdk) with `Choice`/`Noul`/`Score`/
`TypeSafeClient` mirroring the TS SDK.

## Why It Maps Cleanly

- `client.systemOne({state, questions})` → `client.system_one(state, questions).await`
  with `response.choice()/noul()/score()` accessors.
- `jev_sdk` adds free upgrades: `ClientBuilder` (timeouts, custom `reqwest`
  client), `RetryPolicy` (transient 429/529 retried with backoff by default).
- Layer rules become compiler-enforced: `pub(crate)` + module tree replaces
  `scripts/check-dependencies.ts` (delete it).
- Report validation becomes `serde` structs with `#[serde(default)]` for the
  same tolerant reads `isReviewReport` provides today.

## Step 0: SDK Parity Spike (30 min, do first)

`jev_sdk` is young (~7 days on docs.rs at check time). Build one `screenFile`
equivalent and compare JSON wire format against the TS client. Must confirm:

- `inspect`, `focus`, `ignore`, `compare`, `caution`, `fallback`, `not_for`
- `noul` true/false criteria with `examples`
- `choice` option descriptions, `score` rubrics
- Answer shapes: `choice`+`confidence`, `noul` probability, `score`+`confidence`

If anything is missing, the underlying API is plain HTTP — fill gaps directly.

## Steps

1. **Wire-format lock**: capture a TS `systemOne` request/response pair per
   judgment type (screen, profile, evidence, mechanism, severity, route) as
   fixtures under `tests/fixtures/`.
2. **Scaffold**: `cargo init`, `tokio + axum + serde + serde_json + clap +
   jev_sdk`, `domain/` types first (`policy.rs`, `report.rs`, `patch.rs`).
3. **Port bottom-up**: `domain → adapters → review → cli/dashboard`, running
   fixtures against `jev_sdk` to confirm parity at each judgment.
4. **Dashboard**: axum router serving embedded `public/` + `/api/review`;
   Host-check + CSP as middleware; re-run the same curl checks (good host 200,
   foreign host 403, CSP present).
5. **Harden**: port the `docs/security.md` must-preserve list exactly
   (`OpenOptionsExt` flags, `O_EXCL` temps, fail-loud errnos).

## Then, Idiomatic Upgrades (only after parity)

- `thiserror` error types replacing string errors.
- `gix` instead of shelling out to git (or keep `Command` — no shell either way).
- Retry policy tuning + cost/latency metering per run.
- `clap` subcommands: `momus review [path]`, `momus scan [path]`,
  `momus dashboard`, `momus check`.

## Risks

- SDK option parity (answered by the spike).
- Prompt-state serialization: TS sends rich nested `state`; Rust needs matching
  `Serialize` structs — mechanical but fiddly, hence fixtures first.
- `public/app.js` `THRESHOLD`/`SEVERITY_MAX` mirrors of `domain/config.ts` —
  keep the duplication explicit and tested, or generate from one source.
