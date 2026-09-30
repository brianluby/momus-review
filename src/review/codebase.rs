//! Codebase-scan strategy: discovery via git ls-files, judgments via
//! `codebase_judgments`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use anyhow::Result;

use crate::adapters::exclude::Exclude;
use crate::adapters::git;
use crate::adapters::index_store::IndexStore;
use crate::domain::language::is_test_path;
use crate::domain::policy::Probabilities;
use crate::domain::report::{FileProfile, Finding, ReviewMode, SourceFile};
use crate::review::codebase_judgments;
use crate::review::index::{IndexStats, RepoIndex};
use crate::review::strategy::{Discovery, ReviewStrategy, Screening, Signal};
use crate::review::typesafe::TypeSafeClient;

/// Cap on 1-hop import neighbors carried as screening context.
const MAX_NEIGHBORS: usize = 4;

pub struct CodebaseStrategy {
    client: TypeSafeClient,
    exclude: Exclude,
    index: OnceLock<RepoIndex>,
    file_map: OnceLock<HashMap<String, SourceFile>>,
}

impl CodebaseStrategy {
    /// Configure codebase discovery and a shared client with a lazy index pre-pass.
    pub fn new(client: TypeSafeClient, exclude: Exclude) -> Self {
        Self {
            client,
            exclude,
            index: OnceLock::new(),
            file_map: OnceLock::new(),
        }
    }

    /// Resolves `path`'s 1-hop import neighbors to compact `SourceFile`s
    /// (capped at `MAX_NEIGHBORS`, missing files skipped).
    fn neighbor_files(&self, path: &str) -> Vec<SourceFile> {
        let (Some(graph), Some(file_map)) = (self.index.get(), self.file_map.get()) else {
            return Vec::new();
        };
        graph
            .neighbors(path)
            .into_iter()
            .filter_map(|p| file_map.get(p).cloned())
            .take(MAX_NEIGHBORS)
            .collect()
    }
}

impl ReviewStrategy for CodebaseStrategy {
    type File = SourceFile;

    /// Return the shared request client used by judgments and global refinement.
    fn client(&self) -> &TypeSafeClient {
        &self.client
    }

    /// Return bounded one-hop source dependencies/importers for refinement.
    fn neighbor_context(&self, path: &str) -> serde_json::Value {
        let neighbors: Vec<SourceFile> = self
            .neighbor_files(path)
            .iter()
            .map(codebase_judgments::compact_neighbor)
            .collect();
        serde_json::json!(neighbors)
    }

    /// Identify the review mode represented by this strategy.
    fn mode(&self) -> ReviewMode {
        ReviewMode::Codebase
    }

    /// Name the review subjects for progress output.
    fn subject(&self) -> &'static str {
        "codebase source"
    }

    /// Name the test context accompanying the review subjects.
    fn context_label(&self) -> &'static str {
        "repository test"
    }

    /// Discover review subjects and test-context files after exclusions.
    fn discover(&self, scopes: &[PathBuf]) -> Result<Discovery<SourceFile>> {
        let repository = git::repository_files(scopes, &self.exclude)?;
        let mut files = Vec::new();
        let mut context_files = Vec::new();
        for f in repository {
            if is_test_path(&f.path) {
                context_files.push(f);
            } else {
                files.push(f);
            }
        }
        // Store only a compact excerpt per neighbor, not the full file contents
        // (which `discover` already returns for screening).
        let _ = self.file_map.set(
            files
                .iter()
                .map(|f| (f.path.clone(), codebase_judgments::compact_neighbor(f)))
                .collect(),
        );
        Ok(Discovery {
            files,
            context_files,
        })
    }

    /// Build reusable source metadata and fresh tree-dependent context before screening.
    fn prepass(&self, _scopes: &[PathBuf], discovery: &Discovery<SourceFile>) -> Result<()> {
        let _ = self.index.set(RepoIndex::build(
            &discovery.files,
            &discovery.context_files,
            &IndexStore::from_env(),
        ));
        Ok(())
    }
    /// Snapshot computed, reused and fallback metadata counts from the pre-pass.
    fn index_stats(&self) -> IndexStats {
        self.index
            .get()
            .map_or_else(IndexStats::default, |i| i.stats)
    }

    /// Screen this file using indexed metadata and bounded related-test context.
    async fn screen(
        &self,
        file: &SourceFile,
        context: &[SourceFile],
    ) -> Result<Screening<SourceFile>> {
        let neighbors = self.neighbor_files(&file.path);
        codebase_judgments::screen_source_file_indexed(
            &self.client,
            file,
            context,
            &neighbors,
            self.index.get(),
        )
        .await
    }

    /// Classify file role and review priority without gating findings.
    async fn profile(
        &self,
        file: &SourceFile,
        probabilities: &Probabilities,
    ) -> Result<FileProfile> {
        codebase_judgments::profile_source_file(&self.client, file, probabilities).await
    }

    /// Locate concrete evidence for a threshold signal, or return no finding.
    async fn locate(&self, signal: &Signal<SourceFile>) -> Result<Option<Finding>> {
        let neighbors = self.neighbor_files(&signal.file.path);
        codebase_judgments::locate_source_signal_indexed(
            &self.client,
            signal,
            &neighbors,
            self.index.get(),
        )
        .await
    }

    /// Choose supported fix/test strategies for the located finding.
    async fn suggestions(&self, finding: &Finding) -> Result<(Option<String>, Option<String>)> {
        crate::review::explain::enrich_suggestions(&self.client, finding).await
    }
}
