//! Bounded repository evidence shared by optional local analyses.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryChange {
    pub path: String,
    pub old_path: Option<String>,
    pub base: String,
    /// None means removed; empty text means a present empty file.
    pub content: Option<String>,
}
