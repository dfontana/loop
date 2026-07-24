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
    /// Copy a worker-claimed file into the store, returning its hashed
    /// reference. Rejects paths that escape the project root.
    fn capture(&self, state: &str, cycle: u32, claim: &ArtifactClaim) -> Result<ArtifactRef>;
}
