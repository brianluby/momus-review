# Repository tours

`momus tour` builds a local repository tour without API calls. JSON is the default; `--markdown` emits ordered tour stops and a readable directed architecture map. `head`, `basis`, `stops`, `components`, `relationships`, `unknowns` and `partial` make its evidence boundary explicit.

The command reuses the normal local index cache and can write metadata under
`MOMUS_INDEX_DIR` (default `reviews/index`). Set that directory outside the
checkout when cache writes must not affect checkout cleanliness. It reads
working-tree source: dirty or untracked inputs are explicitly disclosed, and
`head` identifies the checkout baseline rather than an immutable tour snapshot.

```sh
momus tour . --exclude 'vendor/**' --exclude 'generated/**' --markdown
momus tour src tests --max-files 250 --max-file-bytes 500000 --max-total-bytes 4000000
```

Entry declarations and conventional package entry paths appear first. Major directories group files and inferred roles using the existing entrypoint/boundary/domain/persistence/infrastructure/utility vocabulary. Each stop includes a source path, one-based line, bounded excerpt and extracted public declarations. Roles inferred from paths are hypotheses rather than runtime responsibility proofs.

The shared RepoIndex still provides heuristic neighbors for review context. Architecture maps use a separate conservative resolver: exact local Rust module and JS/TS relative import paths must resolve uniquely in the visible inventory, and the active top-level import declaration must be supported on one source line. Parent-relative paths preserve their directory identity. Dotted module names such as `user.service` and `config.dev` are preserved; only recognized JS/TS file extensions are stripped. Exact path lookups are indexed once per repository inventory, rather than scanning all files for each import. Ambiguous extensions, conditional/path attributes, opaque comments/multiline strings, unsupported syntax and external imports do not become edges. Maps describe imports rather than calls or deployment boundaries. Python tours expose entrypoints/components but currently report import relationships as unknown.

Gitignore and explicit exclusions apply before reads. Defaults bound eligible files to 500, one file to 1 MB and retained base/current evidence text to 8 MB, counting both copies of changed current text used by inventory and comparison. These are evidence-string limits, not a bound on total process memory or index overhead. CLI upper bounds are 10,000 files, 10 MB per file and 100 MB total. Evidence limits, unavailable files, binaries, symlinks and missing entrypoints become unknowns. Scopes must share one canonical checkout. `partial` identifies actual inventory gaps, missing entrypoint evidence or absence of corroborated internal edges. A complete static inventory with a cited entrypoint and edges can have `partial: false` while retaining the fixed disclosure that dynamic/external relationships remain unknown; this is not a claim of complete runtime architecture. A partial tour can still contain reliable cited relationships.

The lightweight scope counter abstains on a whole file when brace-containing literals or unsupported syntax leave scopes unbalanced; it never retains only an apparently valid prefix of that file's edges. Full-line comments are skipped before scope counting. Opaque syntax guards remain conservative, including backticks in Rust documentation comments; cited maps can omit imports and do not claim full syntax coverage.

The shared index has a separate 2 MB metadata limit. If larger files are admitted by the tour's file-byte setting, or index metadata falls back for unsupported content, the tour reports the fallback count as unknown public-declaration evidence and sets `partial: true`. An empty declaration excerpt for those files does not establish that the file exports nothing.

Secrets are redacted before indexing, signature extraction and output, independently of API redaction settings. Only eligible source/dependency/documentation text is read; dot-env files are not inventory candidates. Source excerpts and Markdown are rendered as text. Exclusions and resource limits can omit crucial architecture context; verify entrypoints and relationships against source before making architectural changes.
