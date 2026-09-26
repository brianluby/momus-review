//! Adapters: git discovery with guarded reads, exclusion globs, and the
//! atomic report store.

pub mod exclude;
pub mod git;
pub mod imports;
pub mod report_store;
pub mod sarif;