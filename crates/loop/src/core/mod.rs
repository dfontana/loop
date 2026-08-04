//! The IR every other module is written against.
//!
//! No I/O, no Lua, no subprocesses. A `Machine` here is fully resolved: prose
//! read, defaults applied, tool allowlists merged. [`AgentRunner`] is the seam
//! that keeps the control loop in `engine` testable without spawning
//! an agent.

pub mod config;
pub mod context;
pub mod error;
pub mod event;
pub mod fold;
pub mod machine;
pub mod runner;
pub mod sink;

pub use config::{Config, Paths};
pub use context::Context;
pub use error::{CoreError, IoContext, Result};
pub use event::{
    Actor, ArtifactClaim, ArtifactRef, ErrorKind, Event, EventPayload, GuardOutcome, RunStatus,
    Totals, Usage,
};
pub use fold::{FoldStatus, ResumePoint, RunState, fold, fold_with_loop_heads};
pub use machine::{
    Budgets, Check, DEFAULT_CHECK_TIMEOUT_S, Defaults, LoopSpec, Machine, ModelChoice, ModelSpec,
    OnExhausted, OnFail, PlaybookRef, QaCase, State, StateId, Thinking, Transition,
};
pub use runner::{
    ABSENT_HANDOFF_RATIONALE, AgentRunner, CheckOutcome, CheckRunner, Choice, JudgeSpec,
    NavigatorSpec, Proposal, Verdict, WorkerResult, WorkerSpec,
};
pub use sink::{ArtifactSink, LedgerSink};

/// The environment variable naming the file a Worker writes its proposal to.
///
/// The whole agent-side contract for ending a stage: write JSON here, stop.
/// The harness reads the file after the spawn exits, so the decision arrives
/// as structured data without loop having to own a tool inside the agent.
pub const HANDOFF_ENV: &str = "LOOP_HANDOFF";
