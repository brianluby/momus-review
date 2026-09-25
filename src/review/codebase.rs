//! Codebase-scan strategy: discovery via git ls-files, judgments via
//! `codebase_judgments`. Mirrors `review/codebase.ts`.

use std::path::PathBuf;

use anyhow::Result;

use crate::adapters::exclude::Exclude;
use crate::adapters::git;
use crate::domain::policy::{Probabilities, test_file};
use crate::domain::report::{FileProfile, Finding, ReviewMode, SourceFile};
use crate::review::codebase_judgments;
use crate::review::strategy::{Discovery, ReviewStrategy, Screening, Signal};
use crate::review::typesafe::TypeSafeClient;

pub struct CodebaseStrategy {
    client: TypeSafeClient,
    exclude: Exclude,
}

impl CodebaseStrategy {
    pub fn new(exclude: Exclude) -> Result<Self> {
        Ok(Self { client: TypeSafeClient::from_env()?, exclude })
    }
}

impl ReviewStrategy for CodebaseStrategy {
    type File = SourceFile;

    fn mode(&self) -> ReviewMode {
        ReviewMode::Codebase
    }

    fn subject(&self) -> &'static str {
        "codebase source"
    }

    fn context_label(&self) -> &'static str {
        "repository test"
    }

    fn discover(&self, scopes: &[PathBuf]) -> Result<Discovery<SourceFile>> {
        let repository = git::repository_files(scopes, &self.exclude)?;
        let mut files = Vec::new();
        let mut context_files = Vec::new();
        for f in repository {
            if test_file().is_match(&f.path) {
                context_files.push(f);
            } else {
                files.push(f);
            }
        }
        Ok(Discovery { files, context_files })
    }

    async fn screen(
        &self,
        file: &SourceFile,
        context: &[SourceFile],
    ) -> Result<Screening<SourceFile>> {
        codebase_judgments::screen_source_file(&self.client, file, context).await
    }

    async fn profile(&self, file: &SourceFile, probabilities: &Probabilities) -> Result<FileProfile> {
        codebase_judgments::profile_source_file(&self.client, file, probabilities).await
    }

    async fn locate(&self, signal: &Signal<SourceFile>) -> Result<Option<Finding>> {
        codebase_judgments::locate_source_signal(&self.client, signal).await
    }
}