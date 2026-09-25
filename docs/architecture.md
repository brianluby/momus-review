# Architecture

## Prototype (TypeScript, `jev-review` fork)

```
src/
  domain/      config.ts, types.ts, patch.ts   policy, report shapes, diff parsing
  adapters/    git.ts, repository-files.ts      change and complete-source discovery
               report-store.ts                  atomic report save/load
  review/      changes.ts, codebase.ts           mode strategies
               judgments.ts, codebase-judgments.ts  Jev calls per mode
               workflow.ts                      staged orchestration (Strategy pattern)
  cli/         review-*.ts, save-*.ts            explicit entry points
  dashboard/   server.ts, public/               loopback HTTP + static client
scripts/check-dependencies.ts                   layer enforcement
```

Layers depend downward only: `{ cli, dashboard } -> review -> adapters -> domain`.
`npm run check` (tsc + `check-dependencies.ts` + client syntax check) fails on
upward imports, cli↔dashboard imports, or cycles.

Key invariants:

- `review/workflow.ts:runReview` owns concurrency (`CONCURRENCY=3` via
  `mapLimit`), thresholding (`SCREEN_THRESHOLD=0.7`), ranking, and report
  assembly. Modes own only `discover`/`screen`/`profile`/`locate`.
- Adapters return only validated content: `readRepoFile` gates on
  `O_NOFOLLOW | O_NONBLOCK` + `fstat.isFile()` + `realpath` containment +
  post-read dev/ino/size identity (`git.ts`, `repository-files.ts`).
- Report writes are atomic: unique `pid.uuid.tmp` with `wx` (`O_EXCL`),
  retry on `EEXIST`, unlink-on-failure, `rename` into place (`report-store.ts`).
- Dashboard binds `127.0.0.1`, serves 3 allowlisted assets + `/api/review`,
  validates `PORT` (1–65535, fallback 4317), enforces `Host` allowlist
  (trimmed/lowercased, else 403), sends `nosniff` + `no-store` + CSP with
  `frame-ancestors 'none'` (`dashboard/server.ts`).
- Client (`dashboard/public/app.js`) renders all untrusted text through text
  nodes; no `innerHTML`. Report validation (`isReviewReport`) is deliberately
  loose for forward-compat; the client null-tolerates every field.

## Target (Rust, `momus-review`)

```
src/
  domain/      policy, report shapes, diff parsing (serde structs)
  adapters/    git discovery, file guards, report store
  review/      strategies + judgments (jev_sdk) + orchestration
  cli/         clap subcommands (review, scan, dashboard, check)
  dashboard/   axum server + embedded public/ assets
```

Mapping:

| TS today | Rust target |
|---|---|
| `TypeSafeClient().systemOne(...)` | `jev_sdk::TypeSafeClient::from_env()?.system_one(...).await` |
| `mapLimit(CONCURRENCY)` | `tokio` + `Semaphore` / `buffer_unordered` |
| `execFileSync("git", …)` | `std::process::Command` (no shell), later `gix` |
| `O_NOFOLLOW`/`fstat` guards | `std::os::unix::fs::OpenOptionsExt`, same checks |
| `node:http` + `handle()` | `axum` router + middleware (Host check, CSP) |
| `check-dependencies.ts` | Deleted; `pub(crate)` + module tree enforces layers |
| Loose `isReviewReport` | `serde` with `#[serde(default)]` for tolerant reads |

`public/` (app.js, index.html, style.css) is ported from the prototype with
the empty-state command hints updated to the `momus` binary; served via
`include_str!`. Same curl-verified behavior: good host 200, foreign host 403,
CSP header present.

## Decisions

- Orchestration stays in code; Jev only answers bounded typed questions.
  Thresholds and budgets are policy (`domain`), never model output.
- Local-first: no auth, no multi-tenancy, no remote surface. The only network
  calls are Jev API evaluations and they carry only the scoped evidence.
- Boring over clever: flat modules, explicit strategies, no plugins until
  the core funnel earns them.
