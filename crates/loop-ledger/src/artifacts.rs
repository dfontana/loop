//! Artifact capture: temp-file + atomic rename + sha256, so a crash never
//! leaves a half-written file a later stage might read.

use std::path::{Path, PathBuf};

use loop_core::{ArtifactClaim, ArtifactRef, ArtifactSink, Result};

pub struct ArtifactStore {
    root: PathBuf,
    project_root: PathBuf,
}

impl ArtifactStore {
    /// `root` is `./.loop/artifacts/`; `project_root` bounds what a worker may
    /// claim.
    pub fn new(root: impl AsRef<Path>, project_root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
            project_root: project_root.as_ref().to_path_buf(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Verify a previously captured artifact still matches its recorded hash.
    ///
    /// TASK T1.
    pub fn verify(&self, art: &ArtifactRef) -> Result<bool> {
        let _ = art;
        todo!("T1")
    }
}

impl ArtifactSink for ArtifactStore {
    /// Copy a worker-claimed file into the store as `<state>-<cycle>-<name>`,
    /// writing a sibling `.sha256`.
    ///
    /// TASK T1. Must be atomic (write a temp file in the destination
    /// directory, fsync, rename) and must reject a claim whose source path
    /// escapes `project_root` after canonicalization — a worker naming
    /// `/etc/passwd` is a prompt-injection vector, not an artifact
    /// (docs/07-risks.md #10).
    fn capture(&self, state: &str, cycle: u32, claim: &ArtifactClaim) -> Result<ArtifactRef> {
        let _ = (state, cycle, claim);
        todo!("T1")
    }
}

/// Hex sha256 of a byte slice.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let _ = bytes;
    todo!("T1")
}
