//! Diff-review strategy: discovery via git diff, judgments via `judgments`.

use std::path::PathBuf;

use anyhow::Result;

use crate::adapters::exclude::Exclude;
use crate::adapters::git;
use crate::domain::language::is_test_path;
use crate::domain::policy::Probabilities;
use crate::domain::report::{ChangedFile, FileProfile, Finding, ReviewMode};
use crate::review::judgments;
use crate::review::strategy::{Discovery, ReviewStrategy, Screening, Signal};
use crate::review::typesafe::TypeSafeClient;

pub struct ChangesStrategy {
    client: TypeSafeClient,
    exclude: Exclude,
    /// `--base`: diff against the merge base with this revision, not `HEAD`.
    base: Option<String>,
}

impl ChangesStrategy {
    pub fn new(client: TypeSafeClient, exclude: Exclude, base: Option<String>) -> Self {
        Self { client, exclude, base }
    }
}

impl ReviewStrategy for ChangesStrategy {
    type File = ChangedFile;

    fn mode(&self) -> ReviewMode {
        ReviewMode::Changes
    }

    fn subject(&self) -> &'static str {
        "changed source"
    }

    fn context_label(&self) -> &'static str {
        "changed test"
    }

    fn client(&self) -> &TypeSafeClient {
        &self.client
    }

    fn discover(&self, scopes: &[PathBuf]) -> Result<Discovery<ChangedFile>> {
        let changed = match &self.base {
            Some(base) => git::changed_files_since(scopes, &self.exclude, base)?,
            None => git::changed_files(scopes, &self.exclude)?,
        };
        let mut files = Vec::new();
        let mut context_files = Vec::new();
        for f in changed {
            if is_test_path(&f.path) {
                context_files.push(f);
            } else {
                files.push(f);
            }
        }
        Ok(Discovery { files, context_files })
    }

    async fn screen(
        &self,
        file: &ChangedFile,
        context: &[ChangedFile],
    ) -> Result<Screening<ChangedFile>> {
        judgments::screen_file(&self.client, file, context).await
    }

    async fn profile(&self, file: &ChangedFile, probabilities: &Probabilities) -> Result<FileProfile> {
        judgments::profile_file(&self.client, file, probabilities).await
    }

    async fn locate(&self, signal: &Signal<ChangedFile>) -> Result<Option<Finding>> {
        judgments::locate_signal(&self.client, signal).await
    }

    async fn suggestions(&self, finding: &Finding) -> Result<(Option<String>, Option<String>)> {
        crate::review::explain::enrich_suggestions(&self.client, finding).await
    }
}