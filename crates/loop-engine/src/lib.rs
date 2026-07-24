//! The control loop — the deterministic half of the system.
//!
//! docs/01-architecture.md is the spec; this crate is its transcription. The
//! loop body contains no LLM call: every decision an agent makes (the worker's
//! proposal, the judge's verdict, the navigator's choice) arrives through a
//! constrained tool schema, is recorded, and is bounded.
//!
//! This crate depends only on `loop-core` and its traits, so the entire control
//! flow is testable against in-process fakes — no Lua, no subprocess, no API
//! key, no filesystem.
//!
//! TASK T5 implements this crate, plus `loop_core::fold`.

use loop_core::{
    AgentRunner, ArtifactSink, Config, GuardEvaluator, LedgerSink, Machine, Result, RunStatus,
    Totals,
};

pub mod guards;
pub mod prompts;
pub mod validate;

pub use prompts::{StageBuilder, StagePlan};
pub use validate::{Diagnostic, Severity, validate};

/// Everything the loop needs that isn't the machine itself. The engine borrows
/// its collaborators as traits; the CLI supplies the real ones.
pub struct Engine<'a> {
    pub machine: &'a Machine,
    pub config: &'a Config,
    pub guards: &'a dyn GuardEvaluator,
    pub runner: &'a dyn AgentRunner,
    pub ledger: &'a mut dyn LedgerSink,
    pub artifacts: &'a dyn ArtifactSink,
    /// Renders prompts and assembles spawn specs. Implemented in the CLI over
    /// `loop-toolbox`, so the engine stays free of filesystem concerns.
    pub stage: &'a dyn StageBuilder,
    /// Wall-clock start, for the budget check. `None` starts it at `run()`.
    pub started_at: Option<std::time::Instant>,
}

/// How a finished run came out.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    pub status: RunStatus,
    pub terminal_state: Option<String>,
    pub totals: Totals,
}

impl Engine<'_> {
    /// Drive the machine to a terminal, appending every decision to the ledger.
    ///
    /// TASK T5. The loop, per docs/01-architecture.md:
    ///
    /// 1. Fold the ledger. A fresh run appends `run_started` (with the resolved
    ///    config snapshot); a resume picks up at the folded
    ///    [`loop_core::ResumePoint`].
    /// 2. If the current state is terminal → `run_finished`, done.
    /// 3. Check global guardrails (wallclock, `$`, transition count) **before**
    ///    spawning. Exceeded → `run_finished{aborted}` naming the guardrail.
    /// 4. `state_entered`, build the stage, spawn the worker, append
    ///    `worker_output` (capturing artifacts and merging trusted vars first,
    ///    so a crash between them loses nothing).
    /// 5. `transition_proposed`. If blocked, or (in `open` mode) the target is
    ///    not a declared edge → Navigator, capped per run and per state; over
    ///    the cap → escalate.
    /// 6. Guard tiers on the chosen edge: structural, `when`, then `criteria`
    ///    via the Judge. A failure runs the edge's `on_fail`.
    /// 7. `transition_committed`, honor `backoff_s`, bump cycle counters, and
    ///    enforce `max_cycles` — on exhaustion, `on_exhausted`.
    ///
    /// Every step appends before it acts, so a crash anywhere is resumable.
    pub fn run(&mut self) -> Result<Outcome> {
        todo!("T5")
    }

    /// One iteration. Returns `Some` when the run reached a terminal. Exposed so
    /// tests can step the machine and assert on the ledger between steps.
    pub fn step(&mut self) -> Result<Option<Outcome>> {
        todo!("T5")
    }
}
