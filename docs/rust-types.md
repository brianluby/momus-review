# Rust Types & Traits (`domain/`, `review/`)

The concrete `serde` structs, policy data, and the `ReviewStrategy` trait as
they ship in the Rust port. Every symbol maps to a TypeScript symbol in the
prototype (`src/domain/*.ts`, `src/review/*.ts`); the mapping is noted so the
port stays mechanical and reviewable.

## Key decision: a thin HTTP client, not `jev_sdk`

`jev-sdk` 0.1.0 (crates.io) could not express the prototype's judgment
questions. Its question types are string-only:

- `NoulCriteria { yes: Option<String>, no: Option<String> }`
- `Choice { criteria: IndexMap<String, Option<String>> }`
- `Score { criteria: Vec<String> }`

…but the prototype sends *structured JSON* in `instructions`/`criteria` — e.g.
`{ what, examples }`, `not_for`, `inspect`/`focus`/`ignore`/`compare`/`caution`
on the instruction object:

```js
noul(
  { question: "…", inspect: "file.patch", focus: "…", ignore: ["…"] },
  { true: { what: "…", examples: ["…"] }, false: { what: "…", not_for: "…" } },
)
```

This is exactly the gap `docs/rust-port.md` Step 0 anticipated ("if anything
is missing, the underlying API is plain HTTP — fill gaps directly"). So the
port speaks the wire format directly in `src/review/typesafe.rs` — a thin
`POST /v1/systemone` client that sends/parses arbitrary JSON and matches the
TypeScript SDK's request shape byte-for-byte. `jev-sdk` is **not** a
dependency; its only reference value was the response answer shapes
(`NoulAnswer.noul`, `ChoiceAnswer.choice`/`confidence`, `ScoreAnswer.score`
/`confidence`), which `typesafe.rs` reproduces as accessors.

## 1. `domain/policy.rs` — pure data (mirrors `config.ts`)

```rust
#[derive(..., Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]   // TestGap → "testGap"
pub enum Dimension { Correctness, Security, Reliability, Compatibility, TestGap }

pub const DIMENSIONS: [Dimension; 5] = [/* … */];
pub type Probabilities = BTreeMap<Dimension, f64>;

// `dimension_metadata()` returns Vec<DimensionMeta>; label/short are owned
// String — &'static str would make DimensionMeta non-Deserialize (a report
// must re-read an older file).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DimensionMeta { pub key: Dimension, pub label: String, pub short: String }

// Thresholds/budgets/patterns consts: SCREEN_THRESHOLD, SEVERITY_MAX,
// ROUTE_SEVERITY, BLOCKING_SEVERITY, MIN_LOCATION_CONFIDENCE, MAX_FOLLOW_UPS,
// MAX_PROFILES, CONCURRENCY.

// The two regex literals become LazyLock statics (Regex isn't const-safe).
static SOURCE_FILE: LazyLock<Regex> = LazyLock::new(/* … */);
static TEST_FILE: LazyLock<Regex> = LazyLock::new(/* … */);
pub fn source_file() -> &'static Regex { &SOURCE_FILE }

// mechanisms(d: Dimension) -> &'static [(&str, &str)]   (order preserved, noIssue last)
// REVIEW_PRIORITY_RUBRIC: [&str; 4], SEVERITY_RUBRIC: [&str; 4], OWNERS: [(&str, &str); 5]
```

`Dimension` also carries `key()` (`"testGap"`, …) and `definition()` (the
`dimensions` record string used in `locate` state).

## 2. `domain/report.rs` — serialized shapes (mirrors `types.ts`)

Serde-only. Fields are `snake_case` in Rust, `camelCase` on the wire via
`#[serde(rename_all = "camelCase")]`; `Action` is `snake_case`.

```rust
#[serde(rename_all = "lowercase")]
pub enum ReviewMode { Changes, Codebase }

pub struct ChangedFile { pub path: String, pub patch: String }
pub struct SourceFile  { pub path: String, pub content: String }

#[serde(rename_all = "camelCase")]
pub struct Hunk { pub id: String, pub start_line: usize, pub patch: String }  // → startLine

pub struct FileProfile { /* file, category, category_confidence, review_priority, … */ }
#[serde(rename_all = "snake_case")]
pub enum Action { Comment, RequestChanges }
pub struct Finding { /* file, line, dimension, probability, location_confidence,
                        mechanism, mechanism_confidence, severity, severity_confidence,
                        owner: Option<String>, owner_confidence: Option<f64>, action */ }

#[serde(default)]
pub struct MatrixRow { pub file: String, #[serde(flatten)] pub probabilities: BTreeMap<Dimension, f64> }

pub struct ConfigSnapshot { screen_threshold, severity_max, max_follow_ups, max_profiles }
pub struct WorkflowCounts { screened_cells, threshold_signals, profiled_files, followed_signals, located_findings, routed_findings }

#[serde(default, rename_all = "camelCase")]
pub struct ReviewReport { /* mode, scope, dimensions, config, screened_files,
                             context_files, matrix, followed_signals, profiles,
                             workflow, findings */ }
```

`#[serde(default)]` on the report reproduces the prototype's loose
`isReviewReport`: an older report still deserializes. `#[serde(flatten)]` on
`MatrixRow` reproduces `Record<Dimension, number>` flattened onto the row.

## 3. `domain/patch.rs` — diff helpers (mirrors `patch.ts`)

```rust
pub fn parse_hunks(patch: &str) -> Vec<Hunk>;       // @@ +N parsing, start-line tracking
pub fn patch_for_new_file(source: &str) -> String;  // all-additions, 80-line chunks
```

Pure string functions; no async, I/O, or SDK.

## 4. `review/` — the `ReviewStrategy` trait (mirrors `workflow.ts`)

```rust
pub trait FileEntry: Debug + Clone + Serialize {
    fn path(&self) -> &str;
}
// impl FileEntry for ChangedFile, SourceFile

pub struct Discovery<F> { pub files: Vec<F>, pub context_files: Vec<F> }
pub struct Screening<F> { pub file: F, pub probabilities: Probabilities }
pub struct Signal<F> { pub file: F, pub dimension: Dimension, pub probability: f64 }

#[allow(async_fn_in_trait)]   // single-task polling via buffered; no Send needed
pub trait ReviewStrategy: Send + Sync {
    type File: FileEntry + Send + Sync + 'static;
    fn mode(&self) -> ReviewMode;
    fn subject(&self) -> &'static str;
    fn context_label(&self) -> &'static str;
    fn discover(&self, scope: &Path) -> Result<Discovery<Self::File>>;
    async fn screen(&self, file: &Self::File, context: &[Self::File]) -> Result<Screening<Self::File>>;
    async fn profile(&self, file: &Self::File, probabilities: &Probabilities) -> Result<FileProfile>;
    async fn locate(&self, signal: &Signal<Self::File>) -> Result<Option<Finding>>;
}
```

Both modes use the same concrete type for `File` and `Context`, so the two
TS type params collapse to one associated type. Two strategy structs
(`ChangesStrategy`, `CodebaseStrategy`) own a `TypeSafeClient` and delegate to
`review/judgments.rs` / `review/codebase_judgments.rs` respectively.

`review/workflow.rs::run_review` is generic over `S: ReviewStrategy`, and owns
concurrency (`futures` `buffered(CONCURRENCY)` — ordered, bounded
concurrency), thresholding (`SCREEN_THRESHOLD`), ranking, follow-up budget
(`MAX_FOLLOW_UPS`), and report assembly.

## Type-level decisions

| # | Decision | Rationale |
|---|---|---|
| T1 | `Dimension` = closed enum, `#[serde(rename_all="camelCase")]` | Exhaustive `match`; wire key `testGap` |
| T2 | Collapse `<File, Context>` → one `type File` | Both modes use identical types for the two roles |
| T3 | `async fn` trait + `buffered` (no `tokio::spawn`) | Ordered concurrency; futures needn't be `Send` |
| T4 | Strategy owns `TypeSafeClient` | One client per run, one mode |
| T5 | `#[serde(default)]` + `#[serde(flatten)]` | Reproduces loose `isReviewReport` + flattened matrix |
| T6 | `LazyLock<Regex>` statics | `Regex` isn't `const`-safe; std |
| T7 | `Action` `snake_case`, report `camelCase` | Wire value `request_changes`, `screenedFiles` |
| T8 | `DimensionMeta` owns `String` label/short | `&'static str` is not `Deserialize` |

## Verification

The wire shape is pinned by `domain/report.rs` unit tests
(`wire_shape_matches_prototype`, `tolerant_read_of_partial_report`). End-to-end:
`momus review` and `momus scan` both run against a live TypeSafe key and
produce a report the dashboard deserializes; the dashboard enforces the Host
allowlist (403), CSP, `nosniff`, and `no-store` exactly as the prototype did.