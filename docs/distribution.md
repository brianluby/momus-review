# Distribution

Goal: run without installing, `uvx`-style — plus a real install path and CI
gating. Prototype gaps that block this, and the fixes.

## Prototype Gaps (TS)

- `package.json`: `private: true`, no `bin` entry, unpublished.
- Entrypoints (`src/cli/*.ts`) have no shebang, take `process.argv[2]`, and
  rely on `node --env-file=.env` — a `bin` can't pass node flags through a
  plain shebang.
- Auth/config assumes a clone: `.env` in cwd for `TYPESAFE_API_KEY`; report
  defaults to `<repo>/reviews/latest.json` relative to `import.meta.dirname`
  (under `npx`, an ephemeral cache dir — `REVIEW_FILE` override exists but is
  undocumented for this use).
- Runtime needs Node ≥ 24 on the machine (TS type-stripping, `engines` field);
  `npx` uses whatever node is active.

## TS Stopgap (if needed before the Rust port)

Add a `bin` (single dispatcher over the four modes, `#!/usr/bin/env node`),
read `TYPESAFE_API_KEY` straight from env instead of requiring `--env-file`,
publish to npm (or run via git spec: `npx github:<user>/jev-review`), default
`REVIEW_FILE` to cwd:

```bash
TYPESAFE_API_KEY=xxx npx momus-review -- /path/to/repo
```

## Rust Target

- `cargo install momus-review` — primary install; binary `momus`.
- `cargo run -q -p momus-review --` equivalents for dev; consider `cargo binstall`
  + prebuilt release binaries (GitHub releases) so users skip compile times.
- No runtime beyond the binary: `momus review ~/repos/anything` just works.
- Config: `TYPESAFE_API_KEY` from env (no `--env-file` dance); `MOMUS_REPORT`
  (successor to `REVIEW_FILE`) defaulting to `./momus-report.json` or
  `./reviews/latest.json` in cwd — never in the install dir.
- CI: `--fail-on-blocking` exit code + SARIF output (see `roadmap.md`);
  GitHub Action wrapper as a later bet.

## npm Note

`momus-review` was free on npm at check time (2026-09-25), alongside crates.io.
If a JS distribution ever matters (e.g. JS API consumers), claim it early to
squat the name — even if the product ships as a Rust binary.
