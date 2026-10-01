//! Bounded repository evidence shared by optional local analyses.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryChange {
    pub path: String,
    /// Rename/copy source path used to attribute base-snapshot evidence.
    pub old_path: Option<String>,
    /// Readable base text. Empty means an empty blob or a verified Git
    /// addition with no previous blob. Missing/unreadable modified/removed
    /// baselines are unknowns and must never be supplied as empty text.
    pub base: String,
    /// None means removed; empty text means a present empty file.
    pub content: Option<String>,
}
