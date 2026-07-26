//! `loop-core` — the IR every other crate is written against.
//!
//! No I/O, no Lua, no subprocesses. A `Machine` here is fully resolved: prose
//! read, defaults applied, tool allowlists merged. [`AgentRunner`] is the seam
//! that keeps the control loop in `loop-engine` testable without spawning an
//! agent.

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
    OnExhausted, OnFail, PlaybookRef, QaCase, State, StateId, Thinking, Transition, TransitionMode,
};
pub use runner::{
    AgentRunner, CheckOutcome, CheckRunner, Choice, JudgeSpec, NavigatorSpec, Proposal, Verdict,
    WorkerResult, WorkerSpec,
};
pub use sink::{ArtifactSink, LedgerSink};

/// The marker the injected `transition` tool returns, carrying the proposal.
pub const LOOP_TRANSITION_MARKER: &str = "LOOP_TRANSITION";

/// The marker the Judge's `verdict` tool returns.
pub const LOOP_VERDICT_MARKER: &str = "LOOP_VERDICT";

/// The marker the Navigator's `choose` tool returns.
pub const LOOP_CHOICE_MARKER: &str = "LOOP_CHOICE";
