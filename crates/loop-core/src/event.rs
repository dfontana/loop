//! The ledger's event schema (docs/03-ledger.md).
//!
//! One JSON object per line, `ts` + `type` on every one. These types are the
//! wire format: changing a field name changes every ledger ever written, so
//! they are kept deliberately narrow and flat.

use serde::{Deserialize, Serialize};

use crate::machine::{Budgets, StateId};

/// A reference to a captured artifact on disk.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub name: String,
    /// Project-relative, e.g. `.loop/artifacts/implement-1-diff.patch`.
    pub path: String,
    pub sha256: String,
}

/// What a worker declares it produced, before the harness hashes it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactClaim {
    pub name: String,
    pub path: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub tokens: u64,
    pub cost_usd: f64,
}

impl std::ops::AddAssign for Usage {
    fn add_assign(&mut self, rhs: Self) {
        self.tokens += rhs.tokens;
        self.cost_usd += rhs.cost_usd;
    }
}

impl std::iter::Sum for Usage {
    fn sum<I: Iterator<Item = Usage>>(iter: I) -> Usage {
        iter.fold(Usage::default(), |mut acc, u| {
            acc += u;
            acc
        })
    }
}

/// The outcome of one guard tier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuardOutcome {
    Pass,
    Fail,
    /// This tier wasn't configured on the edge.
    Skip,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    Transient,
    Fatal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Done,
    Failed,
    Aborted,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Totals {
    pub cost_usd: f64,
    pub wallclock_s: u64,
    pub transitions: u32,
}

/// Who made a proposal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Actor {
    Worker,
    Navigator,
    Harness,
}

/// One ledger line: `ts` plus a flattened, `type`-tagged payload.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    /// ISO-8601, UTC.
    pub ts: String,
    #[serde(flatten)]
    pub payload: EventPayload,
}

impl Event {
    /// Stamp a payload with the current time.
    pub fn now(payload: EventPayload) -> Self {
        Self {
            ts: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            payload,
        }
    }

    pub fn kind(&self) -> &'static str {
        self.payload.kind()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventPayload {
    /// Always the first line. Pins the resolved machine + toolbox for the run.
    RunStarted {
        ticket: String,
        machine_hash: String,
        /// The fully-resolved machine and every referenced playbook/tool by
        /// content hash, so a mid-run toolbox edit cannot change behavior.
        resolved_config: serde_json::Value,
        budgets: Budgets,
    },
    /// A worker is about to run this stage.
    StateEntered {
        state: StateId,
        cycle: u32,
        attempt: u32,
        session_id: Option<String>,
        model: String,
        thinking: String,
        tools: Vec<String>,
    },
    /// Digest of what the worker did. Never a transcript.
    WorkerOutput {
        state: StateId,
        cycle: u32,
        summary: String,
        artifacts: Vec<ArtifactRef>,
        usage: Usage,
    },
    /// The worker's `transition` tool call.
    TransitionProposed {
        from: StateId,
        to: Option<StateId>,
        blocked: bool,
        rationale: String,
        by: Actor,
    },
    /// The result of the three guard tiers on one proposal.
    GuardChecked {
        from: StateId,
        to: StateId,
        structural: GuardOutcome,
        when: GuardOutcome,
        criteria: GuardOutcome,
        judge_rationale: Option<String>,
    },
    /// Only when a proposal was invalid or `blocked`.
    NavigatorInvoked {
        from: StateId,
        proposal: String,
        chosen_to: StateId,
        entry_prompt: Option<String>,
        usage: Usage,
    },
    /// The move is official; the current state advances.
    TransitionCommitted {
        from: StateId,
        to: StateId,
        cycle: u32,
    },
    /// Structured variables for `when` guards. Scraped from a tool's
    /// `LOOP_VARS` line (trusted) or from `transition(vars=…)` (untrusted).
    VarsSet {
        scope: Option<String>,
        values: serde_json::Value,
        /// False when the worker declared them; such vars must never gate a QA pass.
        trusted: bool,
    },
    Error {
        state: Option<StateId>,
        kind: ErrorKind,
        detail: String,
    },
    Note {
        text: String,
    },
    /// Terminal. Nothing follows it.
    RunFinished {
        status: RunStatus,
        terminal_state: Option<StateId>,
        totals: Totals,
    },
}

impl EventPayload {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::RunStarted { .. } => "run_started",
            Self::StateEntered { .. } => "state_entered",
            Self::WorkerOutput { .. } => "worker_output",
            Self::TransitionProposed { .. } => "transition_proposed",
            Self::GuardChecked { .. } => "guard_checked",
            Self::NavigatorInvoked { .. } => "navigator_invoked",
            Self::TransitionCommitted { .. } => "transition_committed",
            Self::VarsSet { .. } => "vars_set",
            Self::Error { .. } => "error",
            Self::Note { .. } => "note",
            Self::RunFinished { .. } => "run_finished",
        }
    }
}
