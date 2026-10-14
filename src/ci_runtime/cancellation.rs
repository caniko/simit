//! Caller-owned cancellation; the library never installs process-global handlers.
use anyhow::Result;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// Cancellation shared across an entire group, including between commands.
#[derive(Clone, Debug, Default)]
pub struct Cancellation(pub(super) Arc<AtomicBool>);

impl Cancellation {
    /// Request cancellation; all subsequent dispatches fail closed.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    /// Return an error when execution has been cancelled.
    pub fn check(&self) -> Result<()> {
        if self.0.load(Ordering::SeqCst) {
            return Err(Cancelled.into());
        }
        Ok(())
    }
}

/// Cancellation is distinct from member dispatch/build failure.
#[derive(Debug)]
pub(super) struct Cancelled;
impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("group execution cancelled")
    }
}
impl std::error::Error for Cancelled {}
