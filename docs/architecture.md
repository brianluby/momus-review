# Architecture

```
src/
  domain/      policy, report shapes, diff parsing, redaction, feedback,
               github_review (inline-vs-summary planning + comment text)
  adapters/    git discovery + guarded reads, exclude globs, report store,
               SARIF, import graph, github (REST client + PR context)
  review/      strategies + judgments (thin TypeSafe client), orchestration,
               refinement, publish (github-review orchestration)
  cli/         clap subcommands (review, scan, github-review, dashboard)
  dashboard/   axum server + embedded public/ assets
  bin/         momus-eval (golden-set evaluation harness)
action.yml     composite GitHub Action (download/verify binary, review, publish)
.github/       release.yml (prebuilt binaries), momus.yml (dogfood on PRs)
```

Layers depend downward only: `{ cli, dashboard } -> review -> adapters -> domain`.
The module tree and `pub(crate)` visibility keep it that way; `domain` does no
I/O.

## Key invariants

- `review/workflow.rs::run_review` owns concurrency (`futures`
  `buffered(CONCURRENCY)`: bounded, order-preserving), thresholding
  (`SCREEN_THRESHOLD = 0.7`, feedback-tuned per dimension), follow-up
  selection, refinement, and report assembly. Strategies (`ChangesStrategy`,
  `CodebaseStrategy`) own only `discover`/`screen`/`profile`/`locate`.
- The one egress point to the Jev API is `TypeSafeClient::system_one`
  (`review/typesafe.rs`): a thin `POST /v1/systemone` client with retry and
  backoff, where every request `state` is redacted (`domain/redact.rs`).
- Adapters return only validated content: `read_repo_file` (`adapters/git.rs`)
  opens with `O_NOFOLLOW | O_NONBLOCK`, requires a regular file, checks
  `canonicalize` containment in the repo, and re-checks dev/ino/size after the
  read. Git runs through `std::process::Command` (no shell), and a `--base`
  revision that looks like an option is refused before it reaches git.
- Report writes are atomic (`adapters/report_store.rs::write_atomic`): a
  unique `O_EXCL` temp file, retried on collision, removed on failure, then
  renamed into place.
- The dashboard binds `127.0.0.1`, serves three embedded assets plus the
  `/api/review`, `/api/history`, and `/api/feedback` endpoints, enforces a
  `Host` allowlist (trimmed, lowercased, else 403), refuses cross-origin
  feedback writes, and sends `nosniff`, `no-store`, and a CSP with
  `frame-ancestors 'none'` (`dashboard/mod.rs`).
- The client (`dashboard/public/app.js`) renders untrusted text through text
  nodes only, never `innerHTML`. Reports deserialize with `#[serde(default)]`,
  so a report saved by an older version stays viewable.
- `momus github-review` (`review/publish.rs`) anchors inline comments only on
  lines of the PR's own diff (`domain/github_review.rs::commentable_lines`),
  redacts everything it publishes, and trusts fingerprint and summary markers
  only in bot-authored comments. The HTTP calls sit behind the `GitHubApi`
  trait (`adapters/github.rs`) so tests use a fake.

## Decisions

- Orchestration stays in code; Jev only answers bounded typed questions.
  Thresholds and budgets are policy (`domain`), never model output.
- Local-first: no auth, no multi-tenancy, no remote surface. The only network
  calls are Jev API evaluations (scoped evidence, secrets redacted) and, in CI,
  the GitHub calls `github-review` makes.
- Publishing lives in a Rust subcommand, not in Action scripts, so it is
  tested like the rest of the code; the Action only downloads, runs, and
  passes inputs through.
- Boring over clever: flat modules, explicit strategies, no plugins until
  the core funnel earns them.
