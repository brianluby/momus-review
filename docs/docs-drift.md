# Documentation drift detection

Enable local documentation checks with `momus review --docs-drift`. The checks
compare the reviewed change and the bounded current repository inventory. They
make no network requests and execute neither source code nor documentation
examples.

The optional `docsDrift` report records `checks` and `unknowns`. Its findings
join the report's ordinary top-level advisory `findings`. Each check identifies its documentation path and one-based
line. Supporting source evidence identifies its path, line, and `current` or
`base` revision. Base evidence describes the previous interface, never the
current implementation. Evidence excerpts are secret-redacted before entering
this artifact.

## Supported comparisons

The initial deterministic recognizer supports complete Markdown Rust fenced
examples and top-level Rust `pub fn` declarations whose signature fits on one
line. Simple fixed positional parameter lists support argument-count checks.
It does not evaluate argument types, return values, behavioral contracts,
feature selection, or the success of an actual example compilation.

Crate-level or item-level `cfg(...)` gates keep the interface conditional and
produce unknown evidence. Benign `cfg_attr` payloads such as
`feature(doc_cfg)`, `no_std` and `doc(cfg(...))` still allow structural
comparisons. A `cfg_attr` that applies `cfg(...)`, including through nested
`cfg_attr`, remains conditional. Multiline attributes are handled; malformed
or excessively nested conditional attributes remain unknown.

An example must establish its source identity: use an exact crate-qualified
call such as `crate::api::connect()`, or reference the source file from the document
with a Markdown link or exact backticked path and use an unqualified call.
Relative source references resolve against the
document's directory. `src/lib.rs`, `src/api.rs`, and `src/api/mod.rs` correspond
to `crate`, `crate::api`, and `crate::api` respectively. Qualified calls to other
crates are unknown. Example-local bindings and imports that could shadow an
unqualified function also cause abstention.

For example, `docs/api.md` can contain:

````markdown
[Implementation](../src/api.rs)

```rust
connect();
```
````

If `src/api.rs:12` currently declares `pub fn connect(address: &str)`, the
check reports the precise example line and declaration line, the expected and
actual argument counts, and an actionable suggestion to update the example or
contract. An equal count produces `consistent` for that comparison alone;
it does not establish documentation correctness or completeness.

Documentation-only changes compare their current examples with available
current source declarations. Source changes also check unchanged current
documentation, allowing an interface change to identify examples that were
missed in the same change. Deleted documentation is excluded from current
examples.

A removed or renamed function requires its exact prior public declaration in
the change's base content, an associated current example, and a changed file
that no longer contains the old identifier. Another visible Rust reference,
declaration, move, wildcard re-export or opaque source-generating macro makes
replacement compatibility unknown. A
removal finding describes the changed file and the previous declaration, with
a suggestion to check replacement exports outside the supplied inventory.
Absence from the repository inventory by itself never creates a finding.

## Unknowns and findings

Unsupported ecosystems, conditional declarations, generic or multiline
signatures, ambiguous source identities, complex argument syntax, malformed
fences, missing current documentation, and unavailable interface evidence stay
explicit in `unknowns`. Prose, inline examples, shell commands, classes,
methods, macros, Rust type compatibility and generated documentation are
outside the initial recognizer. Excluded files and inventory limits must also
be recorded by the review's repository loader; a bounded inventory is not a
complete API graph.

Unsupported Rust declaration diagnostics are scoped to source paths referenced
by the checked documents or their crate-qualified example calls. Unrelated
generic, multiline or conditional functions do not make a supported example's
comparison incomplete. Every unresolved example call still records its own
unknown; the scoped diagnostics do not establish coverage of undocumented APIs.
Current Rust files are lexed once to index exact identifier tokens and opaque
exports, so repeated stale calls reuse that evidence without rescanning the
repository. Comments, string literals and longer identifiers cannot establish
a replacement reference.

Findings use the existing correctness dimension and comment action. They pass
through the review's ordinary fingerprint and suppression behavior, so an
accepted suppression is consistent with other review findings. Suppression
does not erase the supporting check or turn unknown evidence into proof of
compatibility. These structural checks use deterministic match confidence;
the numbers are not calibrated probabilities of deployment failure.

The unit fixtures cover documentation-only mismatch, code changes with
unchanged examples, removed and renamed functions, file moves and re-exports,
relative links and module-qualified calls, valid argument counts, comments
and string literals, nested declarations, unsupported syntax and ecosystems,
conditional interfaces, shadowed names, deleted documents, incomplete fences,
repeated stale calls, unrelated unsupported declarations, and secret redaction.
To validate a reported mismatch fully, compile the
corrected example against the intended crate and build configuration.
