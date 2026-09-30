//! Adapters: git discovery with guarded reads, exclusion globs, the atomic
//! report store, and the content-addressed result cache.

pub mod cache;
pub mod exclude;
pub mod feedback_store;
pub mod git;
pub mod github;
pub mod imports;
pub mod index_store;
pub mod report_store;
pub mod sarif;
