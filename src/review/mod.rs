//! Review layer: the strategy abstraction, the thin TypeSafe client, the
//! staged orchestration, and the per-mode strategies + judgments.

pub mod changes;
pub mod codebase;
pub mod codebase_judgments;
pub mod context;
pub mod explain;
pub mod index;
pub mod judgments;
pub mod merge_confidence;
pub mod meta;
pub mod publish;
pub mod refine;
pub mod regions;
pub mod strategy;
pub mod typesafe;
pub mod workflow;
