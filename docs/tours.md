# Repository tours

`momus tour` builds a local repository tour without API calls. JSON is the default; `--markdown` emits ordered tour stops and a readable directed architecture map. `head`, `basis`, `stops`, `components`, `relationships`, `unknowns` and `partial` make its evidence boundary explicit.

```sh
momus tour . --exclude 'vendor/**' --exclude 'generated/**' --markdown
momus tour src tests --max-files 250 --max-file-bytes 500000 --max-total-bytes 4000000
```

Entry declarations and conventional package entry paths appear first. Major directories group files and inferred roles using the existing entrypoint/boundary/domain/persistence/infrastructure/utility vocabulary. Each stop includes a source path, one-based line, bounded excerpt and extracted public declarations. Roles inferred from paths are hypotheses rather than runtime responsibility proofs.

The shared RepoIndex still provides heuristic neighbors for review context. Architecture maps use a separate conservative resolver: exact local Rust module and JS/TS relative import paths must resolve uniquely in the visible inventory, and the active top-level import declaration must be supported on one source line. Parent-relative paths preserve their directory identity. Ambiguous extensions, conditional/path attributes, opaque comments/multiline strings, unsupported syntax and external imports do not become edges. Maps describe imports rather than calls or deployment boundaries. Python tours expose entrypoints/components but currently report import relationships as unknown.

Gitignore and explicit exclusions apply before reads. Defaults bound eligible files to 500, one file to 1 MB and total base/current evidence to 8 MB. CLI upper bounds are 10,000 files, 10 MB per file and 100 MB total. Evidence limits, unavailable files, binaries, symlinks and missing entrypoints become unknowns. Scopes must share one canonical checkout. The map always discloses that dynamic/external relationships are unknown; partial does not mean every cited relationship is suspect.

Secrets are redacted before indexing, signature extraction and output, independently of API redaction settings. Only eligible source/dependency/documentation text is read; dot-env files are not inventory candidates. Source excerpts and Markdown are rendered as text. Exclusions and resource limits can omit crucial architecture context; verify entrypoints and relationships against source before making architectural changes.
