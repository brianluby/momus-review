# Rust Types & Traits (`domain/`, `review/`)

The optional robustness artifacts introduce new public fields on the 0.2 line.
Older JSON reports still load, while exhaustive Rust literals/matches need the
changes described in [Migrating the Rust API to 0.2](migration-0.2.md).

The concrete `serde` structs, policy data, and the `ReviewStrategy` trait as
they ship, plus the `github-review` types.

## Key decision: a thin HTTP client, not `jev_sdk`

`jev-sdk` 0.1.0 (crates.io) could not express momus's judgment questions.
Its question types are string-only:

- `NoulCriteria { yes: Option<String>, no: Option<String> }`
- `Choice { criteria: IndexMap<String, Option<String>> }`
- `Score { criteria: Vec<String> }`

…but momus sends *structured JSON* in `instructions`/`criteria` — e.g.
`{ what, examples }`, `not_for`, `inspect`/`focus`/`ignore`/`compare`/`caution`
on the instruction object:

```rust
noul(
    json!({ "question": "…", "inspect": "file.patch", "focus": "…", "ignore": ["…"] }),
    json!({ "true": { "what": "…", "examples": ["…"] }, "false": { "what": "…", "not_for": "…" } }),
)
```

The underlying API is plain HTTP, so momus speaks the wire format directly in
`src/review/typesafe.rs`: a thin `POST /v1/systemone` client that sends and
parses arbitrary JSON. `jev-sdk` is **not** a dependency; its only reference
value was the response answer shapes
(`NoulAnswer.noul`, `ChoiceAnswer.choice`/`confidence`, `ScoreAnswer.score`
/`confidence`), which `typesafe.rs` reproduces as accessors.

## 1. `domain/policy.rs` — pure data

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

// Thresholds/budgets consts: SCREEN_THRESHOLD, SEVERITY_MAX,
// ROUTE_SEVERITY, BLOCKING_SEVERITY, MIN_LOCATION_CONFIDENCE, MAX_FOLLOW_UPS,
// MAX_PROFILES, CONCURRENCY.

// mechanisms(d: Dimension) -> &'static [(&str, &str)]   (order preserved, noIssue last)
// mechanisms_for(d: Dimension, lang: Option<Language>) -> Vec<(&str, &str)>
//   — the generic entries plus that language's own mechanisms, spliced ahead
//   of the `other` sentinel; LANGUAGE_MECHANISMS holds the per-language rows.
// REVIEW_PRIORITY_RUBRIC: [&str; 4], SEVERITY_RUBRIC: [&str; 4], OWNERS: [(&str, &str); 5]
```

File patterns are no longer regexes here: `domain/language.rs` owns them as a
single `SPECS` table (extensions, test-name conventions, test-body markers)
behind `Language::from_path`, `is_source_path`, and `is_test_path`. See
`docs/language-support.md`.

`Dimension` also carries `key()` (`"testGap"`, …) and `definition()` (the
`dimensions` record string used in `locate` state).

## 2. `domain/report.rs` — serialized shapes

Serde-only. Fields are `snake_case` in Rust, `camelCase` on the wire via
`#[serde(rename_all = "camelCase")]`; `Action` is `snake_case`.

```rust
#[serde(rename_all = "lowercase")]
pub enum ReviewMode { Changes, Codebase }

pub struct ChangedFile { pub path: String, pub patch: String, pub base: String }  // base: pre-change content
pub struct SourceFile  { pub path: String, pub content: String }

#[serde(rename_all = "camelCase")]
pub struct Hunk { pub id: String, pub start_line: usize, pub patch: String }  // → startLine

pub struct FileProfile { /* file, category, category_confidence, review_priority, … */ }
#[serde(rename_all = "snake_case")]
pub enum Action { Comment, RequestChanges }
pub struct Finding { /* file, line, dimension, probability, location_confidence,
                        mechanism, mechanism_confidence, severity, severity_confidence,
                        owner: Option<String>, owner_confidence: Option<f64>, action,
                        evidence: String (the selected hunk/region excerpt),
                        title/why/fix/test: Option<String> (generated actionability;
                        title/why deterministic from mechanism, fix/test a
                        model-choice; see review/explain.rs) */ }

#[serde(default)]
pub struct MatrixRow { pub file: String, #[serde(flatten)] pub probabilities: BTreeMap<Dimension, f64> }

pub struct ConfigSnapshot { screen_threshold, severity_max, max_follow_ups, max_profiles }
pub struct WorkflowCounts { screened_cells, threshold_signals, profiled_files, followed_signals, located_findings, routed_findings }

#[serde(default, rename_all = "camelCase")]
pub struct ReviewReport { /* mode, scope, dimensions, config, screened_files,
                             context_files, matrix, followed_signals, profiles,
                             workflow, findings */ }
```

`#[serde(default)]` on the report keeps reads tolerant: a report saved by an
older version still deserializes. `#[serde(flatten)]` on `MatrixRow` puts the
per-dimension probabilities directly on the row (`{ "file", "testGap", … }`).

## 3. `domain/patch.rs` — diff helpers

```rust
pub fn parse_hunks(patch: &str) -> Vec<Hunk>;       // @@ +N parsing, start-line tracking
pub fn first_added_line(hunk: &Hunk) -> usize;      // where a finding in the hunk points
pub fn patch_for_new_file(source: &str) -> String;  // all-additions, 80-line chunks
```

Pure string functions; no async, I/O, or SDK.

## 4. `review/` — the `ReviewStrategy` trait

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
    async fn suggestions(&self, finding: &Finding) -> Result<(Option<String>, Option<String>)>;
}
```

Both modes use the same concrete type for the reviewed files and their
context, so one associated type serves both roles. Two strategy structs
(`ChangesStrategy`, `CodebaseStrategy`) own a `TypeSafeClient` and delegate to
`review/judgments.rs` / `review/codebase_judgments.rs` respectively. Two
shared modules back the actionability and region work: `review/explain.rs`
(the `title`/`why`/`fix`/`test` vocabularies and one-call enrichment, exposing
`MAX_ENRICH = 8`) and `review/regions.rs` (`function_regions`, the
tree-sitter-free column-0 declaration splitter used by codebase mode).

`review/workflow.rs::run_review` is generic over `S: ReviewStrategy`, and owns
concurrency (`futures` `buffered(CONCURRENCY)` — ordered, bounded
concurrency), thresholding (`SCREEN_THRESHOLD`), ranking, follow-up budget
(`MAX_FOLLOW_UPS`), and report assembly.

**Follow-ups are unlimited by default:** every
signal at/above `SCREEN_THRESHOLD` gets a follow-up. `--follow-ups N` opts back
into a budget; when capped, selection is per-dimension (each dimension with a
signal gets a slot before global probability fills the rest), so a saturated
cheap dimension (`testGap`) can't starve security/correctness. An earlier
global top-8 budget starved real findings (0 on an intentionally vulnerable
codebase that yields 99 under the current default). See
`select_follow_ups` + its unit tests.

## 5. `github-review` — publishing to a pull request

```rust
// domain/github_review.rs — pure planning and text, no I/O
pub struct PrFile { pub filename: String, pub patch: Option<String> }   // GET /pulls/{n}/files
pub fn commentable_lines(patch: &str) -> BTreeSet<usize>;               // RIGHT-side added + context lines
pub struct InlineComment { pub path: String, pub line: usize, pub side: &'static str, pub body: String }
pub enum SummaryReason { NotADiffReview, OutsideDiff, OverCap, Rejected }
pub struct Plan<'a> { pub inline: Vec<(&'a Finding, InlineComment)>,
                      pub summary_only: Vec<(&'a Finding, SummaryReason)>, pub already_posted: usize }
pub fn plan<'a>(report: &'a ReviewReport, files: &[PrFile], posted: &HashSet<String>, max_inline: usize) -> Plan<'a>;
pub fn comment_body(finding: &Finding) -> String;                      // redacted + <!-- momus:fp=… -->
pub fn summary_body(report: &ReviewReport, plan: &Plan, head_sha: &str) -> String;  // starts with SUMMARY_MARKER

// adapters/github.rs — the REST calls, behind a trait so tests use a fake
pub struct PullRequest { pub repository: String, pub number: u64, pub head_sha: String }  // from the Actions event
pub trait GitHubApi {
    fn pull_files(&self, pr: &PullRequest) -> impl Future<Output = Result<Vec<PrFile>>> + Send;
    fn review_comments(&self, pr: &PullRequest) -> impl Future<Output = Result<Vec<Comment>>> + Send;
    fn issue_comments(&self, pr: &PullRequest) -> impl Future<Output = Result<Vec<Comment>>> + Send;
    fn create_review(&self, pr: &PullRequest, review: &NewReview) -> impl Future<Output = Result<()>> + Send;
    fn create_issue_comment(&self, pr: &PullRequest, body: &str) -> impl Future<Output = Result<()>> + Send;
    fn update_issue_comment(&self, pr: &PullRequest, id: u64, body: &str) -> impl Future<Output = Result<()>> + Send;
}
pub struct ApiStatusError { pub status: u16, pub message: String, pub errors: Vec<String> }
pub fn is_unresolvable_anchor(error: &anyhow::Error) -> bool;          // the one 422 that demotes to the summary

// review/publish.rs
pub async fn publish<A: GitHubApi>(api: &A, pr: &PullRequest, report: &ReviewReport,
                                   options: &PublishOptions) -> Result<PublishOutcome>;
```

The trait returns `impl Future + Send` rather than using `async fn`, so a
public trait can promise `Send` futures; implementations still write
`async fn`.

## Type-level decisions

| # | Decision | Rationale |
|---|---|---|
| T1 | `Dimension` = closed enum, `#[serde(rename_all="camelCase")]` | Exhaustive `match`; wire key `testGap` |
| T2 | One `type File` for reviewed and context files | Both modes use identical types for the two roles |
| T3 | `async fn` trait + `buffered` (no `tokio::spawn`) | Ordered concurrency; futures needn't be `Send` |
| T4 | Strategy owns `TypeSafeClient` | One client per run, one mode |
| T5 | `#[serde(default)]` + `#[serde(flatten)]` | Tolerant reads of older reports + flattened matrix rows |
| T6 | `LazyLock<Regex>` statics | `Regex` isn't `const`-safe; std |
| T7 | `Action` `snake_case`, report `camelCase` | Wire value `request_changes`, `screenedFiles` |
| T8 | `DimensionMeta` owns `String` label/short | `&'static str` is not `Deserialize` |

## Verification

The wire shape is pinned by `domain/report.rs` unit tests
(`wire_shape_is_pinned`, `tolerant_read_of_partial_report`). End-to-end:
`momus review` and `momus scan` both run against a live TypeSafe key and
produce a report the dashboard deserializes; the dashboard enforces the Host
allowlist (403), CSP, `nosniff`, and `no-store`.
