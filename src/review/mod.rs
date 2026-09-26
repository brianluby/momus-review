//! Review layer: the strategy abstraction, the thin TypeSafe client, the
//! staged orchestration, and the per-mode strategies + judgments.

pub mod changes;
pub mod codebase;
pub mod codebase_judgments;
pub mod explain;
pub mod judgments;
pub mod regions;
pub mod strategy;
pub mod typesafe;
pub mod workflow;