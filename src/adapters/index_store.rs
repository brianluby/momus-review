//! Best-effort atomic storage of blob-index metadata, never full source bodies.
use super::report_store::write_atomic;
use serde::{Serialize, de::DeserializeOwned};
use std::path::PathBuf;

pub struct IndexStore {
    dir: PathBuf,
}
impl IndexStore {
    /// Use MOMUS_INDEX_DIR when configured, otherwise reviews/index.
    pub fn from_env() -> Self {
        Self::open(
            std::env::var("MOMUS_INDEX_DIR")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("reviews/index")),
        )
    }
    /// Select a metadata directory; it is created lazily on successful writes.
    pub fn open(dir: PathBuf) -> Self {
        Self { dir }
    }
    /// Deserialize one metadata entry; missing, unreadable or malformed entries are misses.
    pub fn read<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        serde_json::from_str(&std::fs::read_to_string(self.dir.join(format!("{key}.json"))).ok()?)
            .ok()
    }
    /// Atomically replace an entry; serialization and filesystem failures are best-effort misses.
    pub fn write<T: Serialize>(&self, key: &str, entry: &T) {
        if let Ok(text) = serde_json::to_string(entry) {
            let _ = write_atomic(&self.dir.join(format!("{key}.json")), &text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Missing, corrupt, and schema-incompatible metadata must be cache misses.
    #[test]
    fn invalid_entries_are_misses_and_valid_writes_replace_them() {
        let dir = tempfile::tempdir().unwrap();
        let store = IndexStore::open(dir.path().to_path_buf());
        assert_eq!(store.read::<Vec<u64>>("missing"), None);
        std::fs::write(dir.path().join("entry.json"), "truncated{").unwrap();
        assert_eq!(store.read::<Vec<u64>>("entry"), None);
        store.write("entry", &vec![1_u64, 2]);
        assert_eq!(store.read::<Vec<u64>>("entry"), Some(vec![1, 2]));
        assert_eq!(
            store.read::<std::collections::HashMap<String, u64>>("entry"),
            None
        );
        store.write("entry", &vec![3_u64]);
        assert_eq!(store.read::<Vec<u64>>("entry"), Some(vec![3]));
    }
    /// An unusable store must not turn a best-effort cache into a run failure.
    #[test]
    fn non_directory_store_is_best_effort() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("not-a-directory");
        std::fs::write(&file, "original").unwrap();
        let store = IndexStore::open(file.clone());
        store.write("entry", &vec![1_u64]);
        assert_eq!(store.read::<Vec<u64>>("entry"), None);
        assert_eq!(std::fs::read_to_string(file).unwrap(), "original");
    }
}
