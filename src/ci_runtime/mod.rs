//! Runtime contracts for generated CI, independent of rendering and review policy.
pub mod cancellation;
pub mod cli;
pub mod dependencies;
pub mod group;
pub mod log_archive;
pub mod log_custody;
mod log_http;
pub mod log_reconcile;
pub mod process;
