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
pub mod text;

/// Ledger event builders shared by every test module in the crate — and, via
/// the `testing` feature, by the integration tests under `tests/`, which are a
/// separate compilation unit and cannot see a `#[cfg(test)]` module.
#[cfg(any(test, feature = "testing"))]
pub mod fixtures;

pub use config::{Floor, Paths, machine_hash, names_a_path, pi_bin, sanitize_component};
pub use context::Context;
pub use error::{CoreError, IoContext, Result};
pub use event::{
    Artifact, ErrorKind, Event, EventPayload, GuardOutcome, RunFinish, RunStart, RunStatus,
    StateEntered, Totals, Usage, last, run_finished, run_started,
};
// No `fold_with_loop_heads` here: every caller outside this module has a
// `Machine` and wants [`Machine::fold`], or has none and wants `fold`.
pub use fold::{FoldStatus, Hop, ResumePoint, RunState, fold};
pub use machine::{
    Budgets, Check, DEFAULT_CHECK_TIMEOUT_S, Defaults, LoopSpec, Machine, ModelChoice, ModelSpec,
    OnExhausted, OnFail, QaCase, StagePromptRef, State, StateId, Thinking, Transition,
};
pub use runner::{
    ABSENT_HANDOFF_RATIONALE, AgentRunner, CheckOutcome, CheckRunner, Choice, JudgeSpec,
    NavigatorSpec, Proposal, Verdict, WorkerResult, WorkerSpec, with_stderr_tail,
    worker_digest_for_judge,
};
pub use sink::{ArtifactSink, LedgerSink};
pub use text::{brief, first_line, one_line, truncate};

/// The items, first occurrence of each kept, in the order they arrive.
///
/// Four callers hand-rolled the same `if !out.contains(x) { out.push(x) }`
/// loop: the skill and MCP unions, the states a `loop sessions` hint lists, the
/// variables a stage prompt references, and the choices a Navigator is offered.
///
/// `PartialEq` and a `Vec` rather than `Hash` and a set, because every one of
/// them is a short list whose *order* is the point — a `BTreeSet` would sort
/// the machine's declaration order away, and a `HashSet` would scramble it.
pub fn dedup<T: PartialEq>(items: impl IntoIterator<Item = T>) -> Vec<T> {
    let mut out: Vec<T> = Vec::new();
    for item in items {
        if !out.contains(&item) {
            out.push(item);
        }
    }
    out
}

/// The environment variable naming the file a Worker writes its proposal to.
///
/// The whole agent-side contract for ending a stage: write JSON here, stop.
/// The harness reads the file after the spawn exits, so the decision arrives
/// as structured data without loop having to own a tool inside the agent.
pub const HANDOFF_ENV: &str = "LOOP_HANDOFF";

/// The target a Navigator names when no reachable state fits, and the one the
/// harness substitutes when a Navigator's reply is unusable at all.
///
/// Not a state: nothing declares it, no edge leads to it, and `engine` resolves
/// it to the machine's escalation state (or to an aborted run, when none is
/// declared). It is a *routing outcome* spelled as a string because that is the
/// only shape a first-line reply contract has.
///
/// Here beside [`HANDOFF_ENV`] rather than next to the prompt that offers it,
/// because both ends need it and they sit on opposite sides of the layering:
/// `runner` writes it into the Navigator's choices and parses it back, `engine`
/// recognizes it as a decision rather than as an unroutable target, and `engine`
/// imports nothing but `core`.
pub const ESCALATE: &str = "escalate";
