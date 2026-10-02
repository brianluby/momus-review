//! Bounded repository evidence shared by optional local analyses.
//!
//! Values carry readable text, rather than a third state for unreadable blobs.
//! Evidence collectors must disclose unavailable inputs separately and omit
//! them from these values instead of replacing them with empty strings.
use serde::{Deserialize, Serialize};

/// Base and current text for one repository-relative changed path.
///
/// This is a serializable data carrier, not a path or provenance validator.
/// Callers must supply paths from the same repository snapshot and distinguish
/// a verified addition's absent base blob from an unreadable existing blob.
/// Consumers such as docs and dependency analyses rely on that distinction.
///
/// A removed file serializes with `content: null`; a present, empty file retains
/// an empty string. Both retain their readable base text:
///
/// ```
/// use momus_review::domain::repository::RepositoryChange;
///
/// let mut change = RepositoryChange {
///     path: "src/api.rs".into(),
///     old_path: None,
///     base: "pub fn api() {}\n".into(),
///     content: None,
/// };
/// let removed = serde_json::to_value(&change)?;
/// assert!(removed["content"].is_null());
/// change.content = Some(String::new());
/// let empty = serde_json::to_value(&change)?;
/// assert_eq!(empty["content"], "");
/// let decoded: RepositoryChange = serde_json::from_value(empty)?;
/// assert_eq!(decoded.content.as_deref(), Some(""));
/// assert_eq!(decoded.base, "pub fn api() {}\n");
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryChange {
    /// Destination path relative to the repository root, including deletions.
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
