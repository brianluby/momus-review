# Security

Momus is local-first: loopback dashboard, no auth, no remote surface. The
threat model is (1) malicious content in a scanned repo attacking the
reviewer, and (2) accidental disclosure of local secrets to the Jev API.

## Hardening Shipped (FIND-001–004 + review follow-ups)

Upstream PR: `devagrawal09/jev-review#10` (from `brianluby:upstream-pr1`).
All verified with smoke tests; `npm run check` passes.

- **FIND-001, symlink escape (Medium)**: `readRepoFile` in both adapters gates
  every read on `O_NOFOLLOW | O_NONBLOCK` + `fstat.isFile()` + `realpath`
  containment + post-read dev/ino/size identity vs the open fd. Symlinks,
  FIFOs, special files, and repo escapes resolve to skip; ancestor swaps
  mid-read throw `File changed during read`. Only benign races
  (`ELOOP`/`ENOENT`/`ENXIO`/`ENODEV`) skip — unexpected I/O (e.g. `EACCES`)
  rethrows so scans fail loudly. Tracked-file diffs were never at risk
  (content comes from git objects).
- **FIND-002, predictable temp file (Low)**: `saveReport` uses unique
  `pid.uuid.tmp` with `wx` (`O_EXCL`), 10-attempt retry on `EEXIST`, and
  unlinks the owned temp on write/rename failure. A planted symlink is never
  followed; failed writes leave no litter.
- **FIND-003, unvalidated PORT (Info)**: integer 1–65535, fallback 4317.
- **FIND-004, framing/Host (Info)**: `Host` allowlist (`127.0.0.1:port`,
  `localhost:port`, trimmed + lowercased, else 403); CSP gains
  `frame-ancestors 'none'`.
- Bot-review extras: `O_NONBLOCK` in both adapters (FIFO-swap races hang
  neither mode — git never lists untracked FIFOs but the swap window between
  listing and open is real); Host normalization accepts mixed-case/padded
  loopback hosts.

## Verified Non-Issues

- `npm audit`: 0 vulns; `@typesafe-ai/sdk` has zero transitive deps.
- No secrets in repo: only `TYPESAFE_API_KEY=` placeholder in `.env.example`;
  `.env`/`.env.*` gitignored, `!.env.example` exception. No `.env`/`reviews/`
  artifacts committed.
- No XSS sinks: client uses text nodes only; `setAttribute` + numeric-derived
  styles.
- Git invoked via `execFileSync("git", ["-C", cwd, …])` — no shell, no
  interpolation.
- Dashboard: exact-match asset allowlist (no traversal), GET/HEAD-only (405),
  `nosniff` + `no-store` + `default-src 'self'`, generic errors without stacks.

## Residual Risks

- **FIND-005, by design**: full patches/contents are transmitted to the TypeSafe
  API. Key handling is correct (env file, gitignored), but users must know that
  `scan` uploads the scoped tree. Needs a README note + eventual secret
  redaction pre-send.
- No framing beyond CSP, no CORS headers (loopback + no cross-origin reads
  suffices for now).
- Report file (`reviews/latest.json`, overridable via `REVIEW_FILE`) is
  world-readable by default; contains code-derived findings, not secrets —
  but treat it as sensitive.
- No audit log of what was sent to the API per run.

## Rust Port Must-Preserve List

- `O_NOFOLLOW | O_NONBLOCK` + `is_file()` + containment + identity check on
  every worktree read (via `OpenOptionsExt`).
- `O_EXCL` unique temps + unlink-on-failure + atomic rename.
- Loopback bind, asset allowlist, `Host` check, CSP, `nosniff`/`no-store`.
- Fail-loud I/O: skip only on benign-race errnos.
