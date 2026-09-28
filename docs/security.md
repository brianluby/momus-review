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

- **FIND-005, by design, mitigated (#25)**: full patches/contents are
  transmitted to the TypeSafe API, and `scan` uploads the scoped tree.
  Secret redaction (`domain/redact.rs`) now scrubs every request `state` in
  `TypeSafeClient::system_one`, the single egress point, replacing each
  secret with a typed placeholder (`<redacted:rule>`) and preserving line
  structure. It is pattern-based: provider token formats, PEM keys, JWTs,
  URL userinfo, secret-named assignments, and a high-entropy fallback. A
  secret with none of those shapes (a short password in an unlabeled
  variable, a pure-hex key under a neutral name) still goes out. On by
  default, including for loopback servers; `--no-redact` / `MOMUS_REDACT=off`
  disables it.
- No framing beyond CSP, no CORS headers (loopback + no cross-origin reads
  suffices for now).
- Report file (`reviews/latest.json`, overridable via `MOMUS_REPORT`) is
  world-readable by default. Redaction applies only to what is sent, so
  `evidence` excerpts are verbatim and **can contain a hardcoded secret**
  (keeping them local keeps fingerprints stable). Treat the report as
  sensitive, and redact before publishing any excerpt (PR comments, #27).
- Per-run audit: each report's `redactions` field counts distinct values
  redacted per rule (hashed, never stored). There is still no log of the
  full request bodies.

## Rust Port Must-Preserve List

- `O_NOFOLLOW | O_NONBLOCK` + `is_file()` + containment + identity check on
  every worktree read (via `OpenOptionsExt`).
- `O_EXCL` unique temps + unlink-on-failure + atomic rename.
- Loopback bind, asset allowlist, `Host` check, CSP, `nosniff`/`no-store`.
- Fail-loud I/O: skip only on benign-race errnos.
