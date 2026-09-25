//! The per-mode strategy abstraction and shared workflow types. Mirrors the
//! `Strategy<File, Context>` structural type in `review/workflow.ts`.

use std::path::Path;

use anyhow::Result;

use crate::domain::policy::{Dimension, Probabilities};
use crate::domain::report::{ChangedFile, FileProfile, Finding, ReviewMode, SourceFile};

/// A file payload discovered by a mode. Mirrors `File extends { path }`.
pub trait FileEntry: std::fmt::Debug + Clone + serde::Serialize {
    fn path(&self) -> &str;
}

impl FileEntry for ChangedFile {
    fn path(&self) -> &str {
        &self.path
    }
}

impl FileEntry for SourceFile {
    fn path(&self) -> &str {
        &self.path
    }
}

/// The files a mode discovers, split into review subjects and test context.
/// Both roles use the same concrete type in each mode, hence one type param.
pub struct Discovery<F> {
    pub files: Vec<F>,
    pub context_files: Vec<F>,
}

/// A screening result: a file plus its per-dimension probability matrix.
pub struct Screening<F> {
    pub file: F,
    pub probabilities: Probabilities,
}

/// A single dimension's probability for a file, above threshold → follow-up.
#[derive(Debug, Clone)]
pub struct Signal<F> {
    pub file: F,
    pub dimension: Dimension,
    pub probability: f64,
}

/// A review mode. Owns discovery and judgments; `workflow` owns concurrency,
/// thresholds, ranking, and report assembly.
///
/// `async fn` in the trait is deliberate: futures are driven concurrently in
/// a single task via `buffered` (not `tokio::spawn`), so no `Send` bound is
/// required on the futures.
#[allow(async_fn_in_trait)]
pub trait ReviewStrategy: Send + Sync {
    type File: FileEntry + Send + Sync + 'static;

    fn mode(&self) -> ReviewMode;
    fn subject(&self) -> &'static str;
    fn context_label(&self) -> &'static str;

    fn discover(&self, scope: &Path) -> Result<Discovery<Self::File>>;
    async fn screen(&self, file: &Self::File, context: &[Self::File])
        -> Result<Screening<Self::File>>;
    async fn profile(&self, file: &Self::File, probabilities: &Probabilities)
        -> Result<FileProfile>;
    async fn locate(&self, signal: &Signal<Self::File>) -> Result<Option<Finding>>;
}