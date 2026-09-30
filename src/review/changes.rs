//! Diff-review strategy: discovery via git diff, judgments via `judgments`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use anyhow::Result;

use crate::adapters::exclude::Exclude;
use crate::adapters::git;
use crate::domain::language::is_test_path;
use crate::domain::policy::Probabilities;
use crate::domain::report::{ChangedFile, FileProfile, Finding, ReviewMode, SourceFile};
use crate::review::judgments;
use crate::review::strategy::{Discovery, ReviewStrategy, Screening, Signal};
use crate::review::typesafe::TypeSafeClient;

pub struct ChangesStrategy {
    client: TypeSafeClient,
    exclude: Exclude,
    /// `--base`: diff against the merge base with this revision, not `HEAD`.
    base: Option<String>,
    index: OnceLock<crate::review::index::RepoIndex>,
    neighbors: OnceLock<HashMap<String, SourceFile>>,
}

impl ChangesStrategy {
    pub fn new(client: TypeSafeClient, exclude: Exclude, base: Option<String>) -> Self {
        Self {
            client,
            exclude,
            base,
            index: OnceLock::new(),
            neighbors: OnceLock::new(),
        }
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
        Ok(Discovery {
            files,
            context_files,
        })
    }

    fn prepass(&self, scopes: &[PathBuf], discovery: &Discovery<ChangedFile>) -> Result<()> {
        // Best-effort index discovery: changed-file review remains available
        // even when a full-tree pre-pass cannot read the repository inventory.
        let repository = git::repository_files(scopes, &self.exclude).unwrap_or_default();
        let (tests, files): (Vec<_>, Vec<_>) =
            repository.into_iter().partition(|f| is_test_path(&f.path));
        let mut index = crate::review::index::RepoIndex::build(
            &files,
            &tests,
            &crate::adapters::index_store::IndexStore::from_env(),
        );
        if files.is_empty() && !discovery.files.is_empty() {
            index.stats.fallbacks += discovery.files.len();
        }
        index.map_changed_tests(&discovery.files, &discovery.context_files);
        let _ = self.neighbors.set(
            files
                .iter()
                .map(|f| {
                    (
                        f.path.clone(),
                        crate::review::codebase_judgments::compact_neighbor(f),
                    )
                })
                .collect(),
        );
        let _ = self.index.set(index);
        Ok(())
    }
    fn index_stats(&self) -> crate::review::index::IndexStats {
        self.index.get().map_or_else(Default::default, |i| i.stats)
    }
    fn neighbor_context(&self, path: &str) -> serde_json::Value {
        let neighbors: Vec<_> = self
            .index
            .get()
            .map(|i| i.neighbors(path))
            .unwrap_or_default()
            .into_iter()
            .take(4)
            .filter_map(|p| self.neighbors.get()?.get(p).cloned())
            .collect();
        serde_json::json!(neighbors)
    }

    async fn screen(
        &self,
        file: &ChangedFile,
        context: &[ChangedFile],
    ) -> Result<Screening<ChangedFile>> {
        judgments::screen_file_indexed(
            &self.client,
            file,
            context,
            self.index.get(),
            self.neighbor_context(&file.path),
        )
        .await
    }

    async fn profile(
        &self,
        file: &ChangedFile,
        probabilities: &Probabilities,
    ) -> Result<FileProfile> {
        judgments::profile_file(&self.client, file, probabilities).await
    }

    async fn locate(&self, signal: &Signal<ChangedFile>) -> Result<Option<Finding>> {
        judgments::locate_signal(&self.client, signal).await
    }

    async fn suggestions(&self, finding: &Finding) -> Result<(Option<String>, Option<String>)> {
        crate::review::explain::enrich_suggestions(&self.client, finding).await
    }
}
