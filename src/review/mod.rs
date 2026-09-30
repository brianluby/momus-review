//! Review layer: the strategy abstraction, the thin TypeSafe client, the
//! staged orchestration, and the per-mode strategies + judgments.

pub mod auxiliary;
pub mod budget;
pub mod changes;
pub mod codebase;
pub mod codebase_judgments;
pub mod context;
pub mod docs_drift;
pub mod explain;
pub mod index;
pub mod judgments;
pub mod limiter;
pub mod merge_confidence;
pub mod meta;
pub mod planner;
pub mod publish;
pub mod refine;
pub mod regions;
pub mod shard_merge;
pub mod spec_drift;
pub mod strategy;
pub mod test_planner;
pub mod tour;
pub mod typesafe;
pub mod upgrades;
pub mod voi;
pub mod workflow;
