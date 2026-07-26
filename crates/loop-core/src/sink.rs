//! The two persistence seams the engine writes through.
//!
//! `loop-ledger` implements both against real files; the engine's tests
//! implement them in memory. Same reason as [`crate::AgentRunner`]: the control
//! loop should be provable without a filesystem.

use crate::error::Result;
use crate::event::{ArtifactClaim, ArtifactRef, Event, EventPayload};

/// Append-only event log.
pub trait LedgerSink {
    /// Stamp with the current time, serialize, append, and durably flush.
    fn append(&mut self, payload: EventPayload) -> Result<Event>;
    /// Every well-formed event, in order.
    fn read_all(&self) -> Result<Vec<Event>>;
}

/// Captured stage outputs.
pub trait ArtifactSink {
    /// Copy a worker-claimed file into the store, returning a reference to the
    /// snapshot. Rejects paths that escape the project root.
    ///
    /// The claim is worker-authored and therefore routinely wrong — a path
    /// that never existed, a typo, a file the stage deleted before it called
    /// `transition`. Every such failure is an ordinary `Err` the engine
    /// records and moves past, never a reason to take the run down.
    fn capture(&self, state: &str, cycle: u32, claim: &ArtifactClaim) -> Result<ArtifactRef>;
}
