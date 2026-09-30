//! Best-effort atomic storage of blob-index metadata, never full source bodies.
use super::report_store::write_atomic;
use serde::{Serialize, de::DeserializeOwned};
use std::path::PathBuf;

pub struct IndexStore {
    dir: PathBuf,
}
impl IndexStore {
    pub fn from_env() -> Self {
        Self::open(
            std::env::var("MOMUS_INDEX_DIR")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("reviews/index")),
        )
    }
    pub fn open(dir: PathBuf) -> Self {
        Self { dir }
    }
    pub fn read<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        serde_json::from_str(&std::fs::read_to_string(self.dir.join(format!("{key}.json"))).ok()?)
            .ok()
    }
    pub fn write<T: Serialize>(&self, key: &str, entry: &T) {
        if let Ok(text) = serde_json::to_string(entry) {
            let _ = write_atomic(&self.dir.join(format!("{key}.json")), &text);
        }
    }
}
