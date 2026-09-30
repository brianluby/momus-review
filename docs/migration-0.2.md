# Migrating the Rust API to 0.2

The optional robustness features introduce a **Rust source compatibility change**.
The crate version advances from 0.1.4 to 0.2.0, rather than shipping the changed
public struct contracts as another 0.1 patch. Dependencies constrained to the
0.1 line must explicitly opt into 0.2. No release tag or package is published by
this change.

Existing CLI invocations retain their defaults. Stored JSON reports remain
readable: missing new fields default to the original behavior, and the new
test-plan/spec/VOI artifacts are optional. This wire compatibility does not make
exhaustive Rust struct literals source compatible.

## Struct construction

New fields were added to `ConfigSnapshot`, `Finding`, `ReviewReport` and
`ReviewOptions`. Code that enumerates every field must either supply the new
fields or use a default struct update for unspecified values:

```rust
use momus_review::domain::report::{ConfigSnapshot, Finding, ReviewReport};
use momus_review::review::workflow::ReviewOptions;

let config = ConfigSnapshot {
    screen_threshold: 0.7,
    ..Default::default()
};
let finding = Finding {
    file: "src/lib.rs".into(),
    line: 1,
    ..Default::default()
};
let report = ReviewReport {
    scope: ".".into(),
    ..Default::default()
};
let options = ReviewOptions {
    refine: false,
    ..Default::default()
};
```

`ConfigSnapshot.follow_up_strategy` defaults to `Probability` when reading an
older report. `Finding.test_plan`, `ReviewReport.follow_up_plan` and
`ReviewReport.spec_drift` default to `None`. `ReviewOptions` defaults to
probability ordering, no threshold overrides, no test plans and no specs.

## Exhaustive matches and CLI construction

`ReviewStage` adds `TestPlan`; exhaustive matches must handle it, and the stable
serialized key is `testplan`. `Command::Review` and `Command::Scan` add a
`robustness` field. Rust callers constructing these variants can supply
`RobustnessArgs::default()` to retain ordinary review behavior; existing patterns
that enumerate fields can add `..` when they do not inspect the new options.

No new methods are required from existing `FileEntry` implementations:
`test_context` has a default implementation. New optional artifacts use
camelCase JSON fields and do not add a sixth concern dimension or change the
blocking gate.
