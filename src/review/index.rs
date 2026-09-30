//! Incremental pre-pass: persist blob-derived metadata and resolve tree-dependent
//! graph edges and test maps afresh, so renames/additions/deletions never use stale paths.
use super::{
    context,
    regions::{self, Region, RegionSpan},
};
use crate::adapters::{
    imports::{ImportGraph, extract_imports},
    index_store::IndexStore,
};
use crate::domain::{
    language::Language,
    report::{ChangedFile, SourceFile},
};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::collections::BTreeMap;

pub use crate::domain::report::IndexStats;

#[derive(Serialize, Deserialize)]
struct BlobEntry {
    blob: String,
    screen: Vec<RegionSpan>,
    locate: Vec<RegionSpan>,
    imports: Vec<String>,
    signatures: String,
}

pub struct RepoIndex {
    entries: BTreeMap<String, BlobEntry>,
    graph: ImportGraph,
    tests: BTreeMap<String, Vec<SourceFile>>,
    changed_tests: BTreeMap<String, Vec<ChangedFile>>,
    pub stats: IndexStats,
}

impl RepoIndex {
    /// Reuse valid blob metadata and rebuild path-dependent graph/test mappings for this tree.
    pub fn build(files: &[SourceFile], tests: &[SourceFile], store: &IndexStore) -> Self {
        let mut entries = BTreeMap::new();
        let mut stats = IndexStats::default();
        for file in files.iter().chain(tests) {
            // Guarded discovery already omits unreadable/binary files. Avoid
            // persisting oversized metadata and retain ordinary per-request fallback.
            if file.content.len() > 2_000_000 || file.content.contains('\0') {
                stats.fallbacks += 1;
                continue;
            }
            let blob = blob_id(&file.content);
            // Language affects extraction; the same bytes under a new extension
            // must not inherit metadata from the old language.
            let key = format!(
                "v1-{blob}-{}-{}",
                file.path.ends_with(".rs"),
                serde_json::to_string(&Language::from_path(&file.path))
                    .expect("language JSON")
                    .replace('"', "")
            );
            let cached = store.read::<BlobEntry>(&key).filter(|e| {
                e.blob == blob
                    && regions::materialize_regions(&file.content, &e.screen, 160).is_some()
                    && regions::materialize_regions(&file.content, &e.locate, 80).is_some()
            });
            let entry = match cached {
                Some(e) => {
                    stats.reused += 1;
                    e
                }
                None => {
                    stats.computed += 1;
                    let e = BlobEntry {
                        blob,
                        screen: regions::region_spans(&file.content, &file.path, 160),
                        locate: regions::region_spans(&file.content, &file.path, 80),
                        imports: extract_imports(&file.path, &file.content),
                        signatures: regions::export_signatures(&file.content, &file.path),
                    };
                    store.write(&key, &e);
                    e
                }
            };
            entries.insert(file.path.clone(), entry);
        }
        let graph = ImportGraph::from_candidates(files, |f| {
            entries
                .get(&f.path)
                .map(|e| e.imports.clone())
                .unwrap_or_else(|| extract_imports(&f.path, &f.content))
        });
        let tests = files
            .iter()
            .map(|f| (f.path.clone(), context::select_related_tests(f, tests)))
            .collect();
        Self {
            entries,
            graph,
            tests,
            changed_tests: BTreeMap::new(),
            stats,
        }
    }

    /// Replace diff-mode related-test selections using the current changed-file inventory.
    pub fn map_changed_tests(&mut self, files: &[ChangedFile], tests: &[ChangedFile]) {
        self.changed_tests = files
            .iter()
            .map(|f| {
                (
                    f.path.clone(),
                    context::select_related_changed_tests(f, tests),
                )
            })
            .collect();
    }
    /// Return bounded source-test context selected for this repository path.
    pub fn related_tests(&self, path: &str) -> Option<&[SourceFile]> {
        self.tests.get(path).map(Vec::as_slice)
    }
    /// Return bounded changed-test context selected for this diff path.
    pub fn related_changed_tests(&self, path: &str) -> Option<&[ChangedFile]> {
        self.changed_tests.get(path).map(Vec::as_slice)
    }
    /// Return one-hop importers and dependencies resolved against the current inventory.
    pub fn neighbors(&self, path: &str) -> Vec<&str> {
        self.graph.neighbors(path)
    }
    pub fn evidenced_edges(&self) -> &[(String, String, usize)] {
        self.graph.evidenced_edges()
    }
    /// Return persisted declaration signatures, or an empty string when no entry exists.
    pub fn signatures(&self, path: &str) -> &str {
        self.entries.get(path).map_or("", |e| e.signatures.as_str())
    }
    /// Use cached signatures when available, otherwise extract them from this source.
    pub fn source_signatures(&self, file: &SourceFile) -> String {
        self.entries.get(&file.path).map_or_else(
            || regions::export_signatures(&file.content, &file.path),
            |e| e.signatures.clone(),
        )
    }
    /// Materialize matching blob spans, falling back to source splitting for invalid/missing metadata.
    pub fn regions(&self, file: &SourceFile, lines: usize) -> Vec<Region> {
        self.entries
            .get(&file.path)
            .filter(|e| e.blob == blob_id(&file.content))
            .and_then(|e| {
                regions::materialize_regions(
                    &file.content,
                    if lines == 160 { &e.screen } else { &e.locate },
                    lines,
                )
            })
            .unwrap_or_else(|| regions::function_regions(&file.content, &file.path, lines))
    }
}

/// Git's SHA-1 blob address, over the bytes being reviewed, including dirty files.
fn blob_id(content: &str) -> String {
    let mut hash = Sha1::new();
    hash.update(format!("blob {}\0", content.len()));
    hash.update(content.as_bytes());
    format!("{:x}", hash.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn f(path: &str, content: &str) -> SourceFile {
        SourceFile {
            path: path.into(),
            content: content.into(),
        }
    }
    #[test]
    fn renamed_languages_and_fallbacks_cannot_reuse_wrong_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let store = IndexStore::open(dir.path().into());
        let lower = vec![f("a.rs", "pub fn a() {}\nfn b() {}\n")];
        RepoIndex::build(&lower, &[], &store);
        let upper = vec![f("a.RS", &lower[0].content)];
        let index = RepoIndex::build(&upper, &[], &store);
        assert_eq!(index.stats.computed, 1);
        assert_eq!(
            serde_json::to_value(index.regions(&upper[0], 80)).unwrap(),
            serde_json::to_value(regions::function_regions(&upper[0].content, "a.RS", 80)).unwrap()
        );
        let binary = vec![f("b.rs", "fn b() {}\0")];
        let fallback = RepoIndex::build(&binary, &[], &store);
        assert_eq!(fallback.stats.fallbacks, 1);
        assert_eq!(
            serde_json::to_value(fallback.regions(&binary[0], 160)).unwrap(),
            serde_json::to_value(regions::function_regions(&binary[0].content, "b.rs", 160))
                .unwrap()
        );
    }

    #[test]
    fn warm_metadata_is_identical_and_changed_blobs_recompute() {
        let dir = tempfile::tempdir().unwrap();
        let store = IndexStore::open(dir.path().into());
        let files = vec![
            f("src/a.rs", "mod b;\n\npub fn a() {}\n"),
            f("src/b.rs", "pub fn b() {}\n"),
        ];
        let tests = vec![f(
            "tests/a_test.rs",
            "#[test]\nfn a_works() { assert!(true); }\n",
        )];
        let cold = RepoIndex::build(&files, &tests, &store);
        let warm = RepoIndex::build(&files, &tests, &store);
        assert_eq!(cold.stats.computed, 3);
        assert_eq!(warm.stats.reused, 3);
        for file in &files {
            for size in [80, 160] {
                assert_eq!(
                    serde_json::to_value(warm.regions(file, size)).unwrap(),
                    serde_json::to_value(regions::function_regions(
                        &file.content,
                        &file.path,
                        size
                    ))
                    .unwrap()
                );
            }
            assert_eq!(cold.neighbors(&file.path), warm.neighbors(&file.path));
            assert_eq!(
                serde_json::to_value(cold.related_tests(&file.path)).unwrap(),
                serde_json::to_value(warm.related_tests(&file.path)).unwrap()
            );
            assert_eq!(cold.signatures(&file.path), warm.signatures(&file.path));
        }
        let mut changed = files.clone();
        changed[0].content.push_str("pub fn added() {}\n");
        let next = RepoIndex::build(&changed, &tests, &store);
        assert_eq!(
            next.stats,
            IndexStats {
                computed: 1,
                reused: 2,
                fallbacks: 0
            }
        );
        // New/deleted test paths must be resolved from the current tree.
        let removed = RepoIndex::build(&files, &[], &store);
        assert!(removed.related_tests("src/a.rs").unwrap().is_empty());
        assert_eq!(
            blob_id("hello\n"),
            "ce013625030ba8dba906f756967f9e9ca394464a"
        );
    }
    #[test]
    fn corrupt_metadata_and_unwritable_storage_do_not_break_regions() {
        let dir = tempfile::tempdir().unwrap();
        let store = IndexStore::open(dir.path().into());
        let files = vec![f("a.ts", "export function a() { return 1; }\n")];
        RepoIndex::build(&files, &[], &store);
        for entry in std::fs::read_dir(dir.path()).unwrap() {
            std::fs::write(entry.unwrap().path(), "broken").unwrap();
        }
        let recovered = RepoIndex::build(&files, &[], &store);
        assert_eq!(recovered.stats.computed, 1);
        let blocking = dir.path().join("file");
        std::fs::write(&blocking, "no directory").unwrap();
        let unavailable = RepoIndex::build(&files, &[], &IndexStore::open(blocking));
        assert_eq!(
            serde_json::to_value(recovered.regions(&files[0], 80)).unwrap(),
            serde_json::to_value(unavailable.regions(&files[0], 80)).unwrap()
        );
        assert_eq!(recovered.signatures("a.ts"), "export function a()");
    }
}
