//! Diff-review strategy: discovery via git diff, judgments via `judgments`.
//! Mirrors `review/changes.ts`.

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
}

impl ChangesStrategy {
    pub fn new(exclude: Exclude) -> Result<Self> {
        Ok(Self { client: TypeSafeClient::from_env()?, exclude })
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

    fn discover(&self, scopes: &[PathBuf]) -> Result<Discovery<ChangedFile>> {
        let changed = git::changed_files(scopes, &self.exclude)?;
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