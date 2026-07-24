//! The append-only JSONL run record: getting events on and off disk, capturing
//! artifacts, and rendering the digest.
//!
//! See docs/03-ledger.md. The contract, in one line: **state is never stored,
//! only folded** — so there is no mutable state file to desync from the log.
//! The fold itself lives in `loop_core::fold`; this crate is its I/O half.
//!
//! TASK T1 implements this crate. The signatures below are the contract the
//! engine is already written against; fill them in, don't reshape them.

use std::path::{Path, PathBuf};

use loop_core::{Event, EventPayload, LedgerSink, Result, RunState};

pub mod artifacts;
pub mod digest;

pub use artifacts::ArtifactStore;
pub use loop_core::{FoldStatus, ResumePoint, RunState as FoldedState};

/// An open ledger file. Appends are `fsync`ed per event; reads tolerate (and
/// discard) a trailing partial line left by a crash mid-write.
pub struct Ledger {
    path: PathBuf,
}

impl Ledger {
    /// Open or create the ledger at `path`, creating parent directories.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let _ = path;
        todo!("T1")
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether any events have been written — i.e. a run has started here.
    pub fn started(&self) -> bool {
        todo!("T1")
    }

    /// Read and fold in one step.
    pub fn fold(&self) -> Result<RunState> {
        Ok(loop_core::fold(&self.read_all()?))
    }
}

impl LedgerSink for Ledger {
    /// Stamp, serialize, append, fsync. One JSON object per line, never
    /// rewritten.
    ///
    /// TASK T1.
    fn append(&mut self, payload: EventPayload) -> Result<Event> {
        let _ = payload;
        todo!("T1")
    }

    /// Every well-formed event in order. A trailing partial line is discarded
    /// (that is a crash mid-write, and costs at most the last event); a
    /// malformed line *in the middle* is an error, because that is corruption.
    ///
    /// TASK T1.
    fn read_all(&self) -> Result<Vec<Event>> {
        todo!("T1")
    }
}
