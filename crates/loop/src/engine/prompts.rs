//! The seam between the engine and the filesystem.
//!
//! The engine decides *that* a stage should run; a [`StageBuilder`]
//! (implemented in the CLI over `toolbox`) decides *what files and flags*
//! that spawn gets, including assembling the ledger digest. Keeping this a
//! trait is what lets the whole control loop be tested without a toolbox on
//! disk.

use crate::core::{
    ArtifactRef, Context, JudgeSpec, NavigatorSpec, Proposal, Result, StateId, WorkerSpec,
};

/// A stage's assembled inputs.
#[derive(Clone, Debug)]
pub struct StagePlan {
    pub spec: WorkerSpec,
    /// The substitution map the builder rendered this stage's prose with.
    /// Returned so the caller can see what the prompt was built from — the
    /// engine's fakes assert on it — rather than because the loop reads it.
    #[allow(dead_code)]
    pub context: Context,
    /// The skill *names* the stage resolved, for the ledger. `spec.skill_paths`
    /// carries the paths pi actually loads; the names are what a human reading
    /// `state_entered` recognizes.
    pub skills: Vec<String>,
}

pub trait StageBuilder {
    /// Render the playbook, write it out, and assemble the spawn spec.
    ///
    /// `crashed` marks a re-entry that follows a stage which died mid-flight
    /// rather than a clean arrival, and reaches the playbook as `$CRASHED`. A
    /// stage doing something non-idempotent — opening a PR, kicking a deploy —
    /// is the reason it is worth telling apart from a first attempt.
    fn build_stage(
        &self,
        state: &StateId,
        cycle: u32,
        attempt: u32,
        entry_addendum: Option<&str>,
        crashed: bool,
    ) -> Result<StagePlan>;

    /// Assemble the Judge spawn for a `criteria` guard. The digest passed on
    /// must exclude the worker's own pass/fail claim (docs/05-design-notes.md).
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
