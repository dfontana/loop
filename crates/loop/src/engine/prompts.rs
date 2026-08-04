//! The seam between the engine and the filesystem.
//!
//! The engine decides *that* a stage should run; a [`StageBuilder`]
//! (implemented in the CLI over `toolbox`) decides *what files and flags*
//! that spawn gets, including assembling the ledger digest. Keeping this a
//! trait is what lets the whole control loop be tested without a toolbox on
//! disk.

use crate::core::{Artifact, JudgeSpec, NavigatorSpec, Proposal, Result, StateId, WorkerSpec};

/// A stage's assembled inputs: the spawn, plus the two name lists the spawn
/// itself has no use for.
///
/// No `context` field: it carried the substitution map the builder rendered
/// with, which the loop never reads — only the engine's fake asserted on it,
/// and a fake can keep its own record.
#[derive(Clone, Debug)]
pub struct StagePlan {
    pub spec: WorkerSpec,
    /// The skill *names* the stage resolved, for the ledger. `spec.skill_paths`
    /// carries the paths pi actually loads; the names are what a human reading
    /// `state_entered` recognizes.
    pub skills: Vec<String>,
    /// The MCP server names the stage resolved, for the ledger. They reach the
    /// agent through `spec.entry_message` as an instruction to connect them,
    /// so — like `skills` — they are a record of what the stage was told, not
    /// an input to the spawn. This used to sit on `WorkerSpec`, which claims
    /// to be "everything a worker spawn needs" while `worker_command` never
    /// read it.
    pub mcp: Vec<String>,
}

pub trait StageBuilder {
    /// Render the stage prompt, write it out, and assemble the spawn spec.
    ///
    /// `crashed` marks a re-entry that follows a stage which died mid-flight
    /// rather than a clean arrival, and reaches the stage prompt as `$CRASHED`. A
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
        artifacts: &[Artifact],
        check_output: Option<&str>,
    ) -> Result<JudgeSpec>;

    /// Assemble the Navigator spawn, including the graph summary it routes over.
    fn build_navigator(&self, from: &StateId, proposal: Option<&Proposal>)
    -> Result<NavigatorSpec>;
}
