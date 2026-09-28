//! Adapters: git discovery with guarded reads, exclusion globs, and the
//! atomic report store.

pub mod exclude;
pub mod feedback_store;
pub mod git;
pub mod github;
pub mod imports;
pub mod report_store;
pub mod sarif;