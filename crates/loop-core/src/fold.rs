//! Folding the event log into "where are we".
//!
//! This lives in `loop-core`, not `loop-ledger`, because it is pure control-flow
//! semantics over a slice of events — no I/O — and the engine is its only
//! consumer. `loop-ledger` owns getting events on and off disk; this owns what
//! they mean.
//!
//! TASK T5 implements [`fold`].

use std::collections::BTreeMap;

use crate::event::{Event, EventPayload, RunStatus, Totals};
use crate::machine::StateId;
use crate::runner::Proposal;
use crate::vars::Vars;

#[derive(Clone, Debug, PartialEq)]
pub enum FoldStatus {
    /// No `run_started` yet.
    NotStarted,
    Running,
    Finished(RunStatus),
}

/// Where `loop resume` picks up, per docs/03-ledger.md.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum ResumePoint {
    /// Nothing to do — the ledger ends in `run_finished`.
    Done,
    /// Start the run at the machine's entry state.
    #[default]
    Fresh,
    /// Enter (or re-enter) this state. `crashed` marks a `state_entered` with no
    /// following `worker_output` — the stage died mid-flight and re-runs from
    /// scratch, which is why stages must be idempotent (docs/03).
    EnterState { state: StateId, crashed: bool },
    /// A `worker_output` exists but no `transition_committed`: resume at the
    /// guard check for this proposal.
    GuardCheck { from: StateId, proposal: Proposal },
}

#[derive(Clone, Debug, Default)]
pub struct RunState {
    pub status: Option<RunStatus>,
    /// `None` before `run_started`; otherwise the state the run sits in.
    pub current: Option<StateId>,
    /// Loop-head state → completed cycles.
    pub cycles: BTreeMap<StateId, u32>,
    /// (state, cycle) → attempts made.
    pub attempts: BTreeMap<(StateId, u32), u32>,
    /// Every var seen, trusted or not. For prompts and display.
    pub vars: Vars,
    /// Only vars a tool asserted via `LOOP_VARS`. **`when` guards gate on these
    /// alone** — a worker-declared var must never open a QA gate (docs/07 #2).
    pub trusted_vars: Vars,
    pub totals: Totals,
    pub navigator_invocations: u32,
    /// Navigator invocations per source state, for the per-state ping-pong cap.
    pub navigator_by_state: BTreeMap<StateId, u32>,
    /// Artifact name → path, latest wins.
    pub artifacts: BTreeMap<String, String>,
    pub resume: ResumePoint,
}

impl RunState {
    pub fn fold_status(&self) -> FoldStatus {
        match (self.status, &self.current) {
            (Some(s), _) => FoldStatus::Finished(s),
            (None, Some(_)) => FoldStatus::Running,
            (None, None) => FoldStatus::NotStarted,
        }
    }

    pub fn cycle_of(&self, state: &str) -> u32 {
        self.cycles.get(state).copied().unwrap_or(0)
    }

    pub fn attempts_of(&self, state: &str, cycle: u32) -> u32 {
        self.attempts
            .get(&(state.to_string(), cycle))
            .copied()
            .unwrap_or(0)
    }

    pub fn navigator_from(&self, state: &str) -> u32 {
        self.navigator_by_state.get(state).copied().unwrap_or(0)
    }
}

/// Fold the event log into the current run state.
///
/// TASK T5. Rules (docs/03-ledger.md):
/// - `state_entered` bumps `attempts[(state, cycle)]` and sets `current`.
/// - `vars_set` deep-merges into `vars`; when `trusted`, also into `trusted_vars`.
/// - `transition_committed` sets `current = to`; the caller tells cycles apart
///   via [`fold_with_loop_heads`] — plain `fold` counts a re-entry of any state
///   as a cycle bump for that state, which is the machine-agnostic reading.
/// - `worker_output` and `navigator_invoked` accumulate `totals` and counters.
/// - `run_finished` sets `status`; nothing after it is read.
/// - `resume` is derived from the tail — see [`ResumePoint`].
///
/// The function must be **total**: a truncated or surprising log yields a
/// sensible state, never a panic.
pub fn fold(events: &[Event]) -> RunState {
    fold_with_loop_heads(events, &|_| true)
}

/// Fold, consulting the machine about which states are loop heads (only a loop
/// head's re-entry increments a cycle counter).
///
/// TASK T5.
pub fn fold_with_loop_heads(events: &[Event], is_loop_head: &dyn Fn(&str) -> bool) -> RunState {
    let _ = (events, is_loop_head);
    todo!("T5")
}

/// Convenience for `status`: the last event of a given kind.
pub fn last_of<'e>(events: &'e [Event], kind: &str) -> Option<&'e EventPayload> {
    events
        .iter()
        .rev()
        .find(|e| e.kind() == kind)
        .map(|e| &e.payload)
}
