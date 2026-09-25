//! Momus Review: fast, calibrated, staged code and security review.
//!
//! Layers depend downward only: `{ cli, dashboard } -> review -> adapters ->
//! domain`. Enforced by `pub(crate)` visibility + the module tree.

pub mod adapters;
pub mod cli;
pub mod dashboard;
pub mod domain;
pub mod review;