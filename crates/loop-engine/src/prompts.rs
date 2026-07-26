//! The seam between the engine and the filesystem.
//!
//! The engine decides *that* a stage should run; a [`StageBuilder`]
//! (implemented in the CLI over `loop-toolbox`) decides *what files and flags*
//! that spawn gets, including assembling the ledger digest. Keeping this a
//! trait is what lets the whole control loop be tested without a toolbox on
//! disk.

use loop_core::{
    ArtifactRef, Context, JudgeSpec, NavigatorSpec, Proposal, Result, StateId, WorkerSpec,
};

/// A stage's assembled inputs.
#[derive(Clone, Debug)]
pub struct StagePlan {
    pub spec: WorkerSpec,
    pub context: Context,
}

pub trait StageBuilder {
    /// Render the playbook, write it out, and assemble the spawn spec.
    fn build_stage(
        &self,
        state: &StateId,
        cycle: u32,
        attempt: u32,
        entry_addendum: Option<&str>,
    ) -> Result<StagePlan>;

    /// Assemble the Judge spawn for a `criteria` guard. The digest passed on
    /// must exclude the worker's own pass/fail claim (docs/07-risks.md #1).
    ///
    /// `check_output` is what the edge's deterministic check printed, when it
    /// has one — the only evidence the Judge gets that the worker had no hand
    /// in producing.
    fn build_judge(
        &self,
        criteria: &str,
        worker_summary: &str,
        artifacts: &[ArtifactRef],
        check_output: Option<&str>,
    ) -> Result<JudgeSpec>;

    /// Assemble the Navigator spawn, including the graph summary it routes over.
    fn build_navigator(&self, from: &StateId, proposal: Option<&Proposal>)
    -> Result<NavigatorSpec>;
}
