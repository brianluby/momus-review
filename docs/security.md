# Security

Momus is local-first: loopback dashboard, no auth, no remote surface. The
threat model is (1) malicious content in a scanned repo attacking the
reviewer, (2) accidental disclosure of local secrets to the Jev API, and (3)
in CI, a pull request trying to abuse the review workflow.

## Hardening (FIND-001–004)

FIND-001–004 come from a security review of the `jev-review` design (the
same issues were reported and fixed upstream in
[devagrawal09/jev-review#10](https://github.com/devagrawal09/jev-review/pull/10)).
Momus implements each fix itself:

- **FIND-001, symlink escape (Medium)**: `read_repo_file`
  (`adapters/git.rs`) opens every worktree file with
  `O_NOFOLLOW | O_NONBLOCK`, requires a regular file, checks `canonicalize`
  containment in the repo root, and compares dev/ino/size before and after
  the read. Symlinks, FIFOs, special files, and repo escapes are skipped;
  a file swapped mid-read fails with `File changed during read`. Only benign
  races (`ELOOP`/`ENOENT`/`ENXIO`/`ENODEV`) skip; any other I/O error (e.g.
  `EACCES`) fails the run loudly. Tracked-file diffs come from git objects
  and were never at risk.
- **FIND-002, predictable temp file (Low)**: `write_atomic`
  (`adapters/report_store.rs`) writes a unique temp file opened with
  `O_EXCL` (`create_new`), retries on collision, removes it on failure, and
  renames it into place. A planted symlink is never followed and failed
  writes leave no litter.
- **FIND-003, unvalidated port (Info)**: `--port` is a `u16` in 1–65535
  (default 4317); clap rejects anything else.
- **FIND-004, framing/Host (Info)**: `Host` allowlist (`127.0.0.1:port`,
  `localhost:port`, trimmed and lowercased, else 403) and a CSP with
  `frame-ancestors 'none'` (`dashboard/mod.rs`).

## Verified Non-Issues

- No secrets in the repo: `.env`/`.env.*` are gitignored (except
  `.env.example`), and `reviews/` (reports, feedback, history) is ignored.
- No XSS sinks: the dashboard client renders untrusted text through text
  nodes only, never `innerHTML`.
- Git runs through `std::process::Command` with argument vectors: no shell,
  no interpolation. A `--base` revision starting with `-` is refused before
  it reaches git.
- Dashboard: exact-match asset routes (no traversal), `nosniff` +
  `no-store` + `default-src 'self'`, generic errors without stacks. The one
  write endpoint (`POST /api/feedback`) requires `Content-Type:
  application/json` (a browser must preflight, which is never approved) and
  refuses a browser `Origin` other than the dashboard's own.

## CI (GitHub Action)

The action (`action.yml`) and `momus github-review` run on untrusted pull
request content with a write token, so:

- **Permissions**: `contents: read` and `pull-requests: write`;
  `security-events: write` only with `sarif: true`. Nothing else.
- **Fork PRs are skipped**: secrets are not available to `pull_request`
  workflows from forks, so the action exits with a notice instead of
  failing. It also refuses every event but `pull_request`.
- **`pull_request_target` is not supported, and not recommended**: it runs
  with the base repository's secrets and a write token while checking out
  untrusted fork code. `version: source` would then build (and run
  `build.rs` from) the fork's code with that token; even a prebuilt binary
  would send fork code to the Jev API on the repository's key. Keep fork PRs
  unreviewed, or review them after a maintainer pushes the branch.
- **No script injection**: every input and event field reaches the action's
  scripts through `env`, never `${{ }}` inside `run`.
- **Binary integrity**: the downloaded release archive is checked against
  its `.sha256`; a mismatch fails the step (no silent fallback). Third-party
  actions are pinned to commit SHAs.
- **Report stays off the log**: `momus review`'s stdout (the report, with
  unredacted `evidence`) goes to `/dev/null`; the report file stays in
  `RUNNER_TEMP`.
- **Published text is redacted**: comment bodies and the summary go through
  `domain::redact`, the same rules as outgoing requests. Raw evidence is
  never published.
- **Markers are trusted only from bots**: fingerprint and topic markers
  count as "already posted", and the summary is found and edited, only in
  bot-authored comments. Both are deterministic, so otherwise a PR author
  could pre-post a finding's marker to suppress it.
- **Many repositories**: the caller passes only `TYPESAFE_API_KEY` to the
  reusable workflow, never `secrets: inherit`, and skips fork pull requests
  at the job. `scripts/rollout.sh` pins the caller to the latest release's
  commit SHA, and that one pin covers every piece that handles the secret:
  the workflow file, the action (checked out from the workflow's own commit,
  `job.workflow_sha`), and the binary (`version: auto` downloads the release
  tagged at that commit, or builds the commit when no release is tagged
  there). Upgrades arrive as Dependabot pull requests, so each new momus
  version is a change you review in each repository. `--float` pins to the
  moving `v0` tag instead: every repository then runs whatever this
  repository releases next. The release workflow moves `v0` only for the
  newest `v0.x.y`, never for a backport or pre-release. `scripts/rollout.sh`
  hands the key to `gh` on stdin, never on a command line.
- **Same-repo PRs**: anyone who can push a branch can change `action.yml` or
  the code `version: source` builds, and so can reach the step's secrets.
  That is already true of push access; the dogfood workflow accepts it.

## Residual Risks

- **FIND-005, by design, mitigated (#25)**: full patches/contents are
  transmitted to the TypeSafe API, and `scan` uploads the scoped tree; in CI
  the action sends the PR's diff. Secret redaction (`domain/redact.rs`)
  scrubs every request `state` in `TypeSafeClient::system_one`, the single
  egress point, replacing each secret with a typed placeholder
  (`<redacted:rule>`) and preserving line structure. It is pattern-based:
  provider token formats, PEM keys, JWTs, URL userinfo, secret-named
  assignments, and a high-entropy fallback. A secret with none of those
  shapes (a short password in an unlabeled variable, a pure-hex key under a
  neutral name) still goes out. On by default, including for loopback
  servers; `--no-redact` / `MOMUS_REDACT=off` disables it.
- No framing beyond CSP, no CORS headers (loopback + no cross-origin reads
  suffices for now).
- The report file (`reviews/latest.json`, overridable via `MOMUS_REPORT`) is
  world-readable by default. Redaction applies only to what is sent, so
  `evidence` excerpts are verbatim and **can contain a hardcoded secret**
  (keeping them local keeps fingerprints stable). Treat the report as
  sensitive.
- Per-run audit: each report's `redactions` field counts distinct values
  redacted per rule (hashed, never stored). There is still no log of the
  full request bodies.
- Dependency auditing (`cargo audit`/`cargo deny`) is not yet in CI.

## Invariants to keep

- `O_NOFOLLOW | O_NONBLOCK` + `is_file()` + containment + identity check on
  every worktree read (via `OpenOptionsExt`).
- `O_EXCL` unique temps + unlink-on-failure + atomic rename.
- Loopback bind, asset allowlist, `Host` check, CSP, `nosniff`/`no-store`,
  same-origin feedback writes.
- Fail-loud I/O: skip only on benign-race errnos.
- One egress point to the Jev API, redacted; publish only redacted text.
