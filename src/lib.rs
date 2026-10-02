//! Momus Review: staged code and security review with explicit evidence limits.
//!
//! The CLI assembles repository inputs, runs review judgments and publishes a
//! [`domain::report::ReviewReport`]. The library also exposes the underlying adapters,
//! data structures and analyses. Calling those APIs directly requires honoring
//! their input contracts; constructing or deserializing report data does not
//! establish that a review was complete or that its provenance was verified.
//!
//! Start with these contracts when embedding or extending the review pipeline:
//!
//! - [`adapters::git`] distinguishes working-tree reads from pinned Git evidence.
//! - [`domain::patch`] defines diff anchors; [`domain::repository`] distinguishes
//!   removed files, empty text and unavailable baselines.
//! - [`domain::report`] exposes findings together with skipped, deferred and
//!   incomplete evidence. An empty finding list alone does not mean a clean run.
//! - [`domain::redact`] describes best-effort redaction; [`adapters::cache`]
//!   describes caller-owned cache storage and input binding.
//! - [`review::budget`] counts reserved attempts, including unsuccessful ones.
//! - [`review::merge_confidence`] keeps heuristic scores separate from observed
//!   outcomes. [`adapters::approval`] applies an opt-in eligibility policy before
//!   posting an approval; it does not merge a pull request.
//!
//! The crate does not establish calibrated outcome probabilities merely by
//! producing a report. Empirical estimates require validated history and the
//! support checks documented by the outcome APIs.

pub mod adapters;
pub mod cli;
pub mod dashboard;
pub mod domain;
pub mod review;
