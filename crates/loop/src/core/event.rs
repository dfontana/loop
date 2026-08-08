//! The ledger's event schema (skills/loop-authoring/references/runtime.md).
//!
//! One JSON object per line, `ts` + `type` on every one. These types are the
//! wire format: changing a field name changes every ledger ever written, so
//! they are kept deliberately narrow and flat.

use serde::{Deserialize, Serialize};

use crate::core::machine::{Budgets, StateId};

/// A file one stage produced, under the name a later stage asks for it by.
///
/// One type wears two hats, and what tells them apart is who wrote it, not
/// what it holds: a Worker *claims* a path in the working tree, and the
/// harness records the *snapshot* it took of that file.
/// [`crate::core::ArtifactSink::capture`] is the function between the two, and
/// the only place the difference is real — it takes a claim and returns a
/// snapshot. This used to be two identical structs eleven lines apart, which
/// stated the distinction without enforcing any part of it.
///
/// A snapshot, not a live path: the harness copies the worker's claimed file
/// into the store under a `<state>-<cycle>-<name>` key, so a later stage
/// reading `$ARTIFACT_DIFF` gets the diff *that* cycle produced rather than
/// whatever the working tree holds now. There is no content hash — nothing
/// consumed one, and a hash nobody checks is a claim about integrity the
/// system does not actually make.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    pub name: String,
    /// Whatever the worker wrote on a claim; project-relative on a snapshot,
    /// e.g. `.loop/artifacts/implement-1-diff.patch`.
    pub path: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    /// `default` for the ledgers written before [`Totals`] carried a `Usage`
    /// and this field only existed on the per-spawn events.
    #[serde(default)]
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

impl From<bool> for GuardOutcome {
    /// A tier that ran, from whether it passed. `Skip` has no `bool` — it is
    /// the tier that never ran — which is exactly why this conversion is
    /// total and safe to reach for.
    fn from(passed: bool) -> Self {
        if passed { Self::Pass } else { Self::Fail }
    }
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

/// What a run has spent, by the three things it is bounded by.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Totals {
    /// Every token and every dollar every role has burned. The same [`Usage`]
    /// the per-spawn events carry, so `RunState::spend` is one `+=` against
    /// the `AddAssign` impl rather than a field-by-field copy of it — which is
    /// how a new cost-bearing event stays a one-line change.
    ///
    /// Flattened, so the wire format is unchanged: `cost_usd` and `tokens`
    /// still sit directly on `run_finished.totals`, where every ledger ever
    /// written has them.
    #[serde(flatten)]
    pub usage: Usage,
    pub wallclock_s: u64,
    pub transitions: u32,
}

/// The body of a `state_entered` line.
///
/// A named struct rather than eight fields inline on the variant, so
/// [`crate::episode::Episode`] can borrow the whole entry instead of copying
/// each field out of it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StateEntered {
    pub state: StateId,
    pub cycle: u32,
    pub attempt: u32,
    pub session_id: Option<String>,
    pub model: String,
    pub thinking: String,
    /// Skills loaded into this stage, by name.
    pub skills: Vec<String>,
    /// MCP servers this stage was told to connect, by name.
    pub mcp: Vec<String>,
}

impl StateEntered {
    /// The session id, when there is one worth reopening. A blank string is
    /// nothing to hand back to pi.
    pub fn session(&self) -> Option<&str> {
        self.session_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }
}

/// One ledger line: `ts`, the run's accumulated wallclock, and a flattened,
/// `type`-tagged payload.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    /// ISO-8601, UTC.
    pub ts: String,
    /// Seconds of *run* time accumulated when this line was written, summed
    /// across every process that has driven this ledger.
    ///
    /// `ts` alone cannot answer that question: the gap between the last event
    /// of an interrupted run and the first event of its `loop resume` is
    /// wall-clock time during which nothing was running, and charging it to a
    /// time budget would put any run left overnight instantly over. So each
    /// append carries the accumulator forward, and a resuming process picks up
    /// from the last line rather than from zero.
    pub elapsed_s: u64,
    #[serde(flatten)]
    pub payload: EventPayload,
}

impl Event {
    /// Stamp a payload with the current time and the run's elapsed seconds.
    pub fn stamped(payload: EventPayload, elapsed_s: u64) -> Self {
        Self {
            ts: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            elapsed_s,
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
    /// Always the first line. Records the run's machine identity and guardrails.
    RunStarted {
        ticket: String,
        machine_hash: String,
        budgets: Budgets,
    },
    /// A worker is about to run this stage.
    StateEntered(StateEntered),
    /// Digest of what the worker did. Never a transcript.
    WorkerOutput {
        state: StateId,
        cycle: u32,
        summary: String,
        artifacts: Vec<Artifact>,
        usage: Usage,
    },
    /// The worker's handoff, or the blocked proposal the harness synthesized
    /// when it left none.
    ///
    /// There is no `by`: only a Worker ever proposes. The Navigator's routing
    /// is its own `navigator_invoked` line and the harness's commits are
    /// `transition_committed`, so a field naming the author was a constant.
    TransitionProposed {
        from: StateId,
        to: Option<StateId>,
        blocked: bool,
        rationale: String,
    },
    /// The result of the guard tiers on one proposal.
    ///
    /// Two tiers. There is no "structural" outcome: an edge is resolved out of
    /// the machine's declared transitions before the guards run, so a target
    /// with no declared edge is an `error`, not a failed tier.
    GuardChecked {
        from: StateId,
        to: StateId,
        check: GuardOutcome,
        criteria: GuardOutcome,
        /// What the edge's deterministic check printed, truncated. The one
        /// piece of evidence on this line the worker did not author.
        check_output: Option<String>,
        judge_rationale: Option<String>,
        /// What the Judge spawn cost, when the `criteria` tier ran. Zero
        /// otherwise. Recorded here rather than nowhere, so a criteria-heavy
        /// machine's spend is visible to the `:usd` budget instead of showing
        /// up only on the invoice.
        usage: Usage,
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

/// The `run_started` line, destructured once.
///
/// Four callers wanted a different field off it — the ticket for a session
/// listing and for the digest's heading, the recorded hash for `recap`'s
/// provenance check, the whole line for the recap timeline — and each
/// re-matched the variant to get it.
pub struct RunStart<'e> {
    pub ts: &'e str,
    pub ticket: &'e str,
    pub machine_hash: &'e str,
    pub budgets: &'e Budgets,
}

/// The run's opening line, if this ledger has one.
///
/// First one wins: a hand-concatenated ledger with two starts still describes
/// the run its first line opened. A ledger with none is not a broken ledger —
/// a repaired or hand-assembled one still answers every other question — so
/// this is an `Option` rather than an error.
pub fn run_started(events: &[Event]) -> Option<RunStart<'_>> {
    events.iter().find_map(|e| match &e.payload {
        EventPayload::RunStarted {
            ticket,
            machine_hash,
            budgets,
        } => Some(RunStart {
            ts: &e.ts,
            ticket,
            machine_hash,
            budgets,
        }),
        _ => None,
    })
}

/// The `run_finished` line, destructured once — [`RunStart`]'s closing
/// counterpart.
///
/// The opening line got this treatment when four callers wanted a field off
/// it; the closing line had three readers and no shared destructuring, so its
/// three fields were written out in three places: this variant, the engine's
/// `Outcome`, and a private struct in `report`. A fourth field on
/// `run_finished` had to be added to all three, and nothing related them.
pub struct RunFinish<'e> {
    pub ts: &'e str,
    pub status: RunStatus,
    pub terminal_state: Option<&'e str>,
    pub totals: &'e Totals,
}

/// The last event `pick` says something about, searched from the end.
///
/// The ledger is append-only, so "the most recent X" is the shape nearly every
/// reader of it wants — the last fatal error, the last worker output for a
/// state, the closing line. Each used to spell out its own
/// `events.iter().rev().find_map(|e| match &e.payload { .. })`, six of them
/// across five modules with no shared vocabulary. `pick` gets the whole event,
/// so a caller that wants the timestamp as well as the payload can have it.
pub fn last<'e, T>(events: &'e [Event], pick: impl Fn(&'e Event) -> Option<T>) -> Option<T> {
    events.iter().rev().find_map(pick)
}

/// The run's closing line, if it has one.
///
/// **Last** one wins, where [`run_started`] takes the first: both pick the
/// outermost bracket, which for a hand-concatenated ledger means the start of
/// the first run and the end of the last. Nothing is supposed to follow
/// `run_finished` — the fold stops there — so in a well-formed ledger there is
/// exactly one either way.
pub fn run_finished(events: &[Event]) -> Option<RunFinish<'_>> {
    last(events, |e| match &e.payload {
        EventPayload::RunFinished {
            status,
            terminal_state,
            totals,
        } => Some(RunFinish {
            ts: &e.ts,
            status: *status,
            terminal_state: terminal_state.as_deref(),
            totals,
        }),
        _ => None,
    })
}

impl EventPayload {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::RunStarted { .. } => "run_started",
            Self::StateEntered(_) => "state_entered",
            Self::WorkerOutput { .. } => "worker_output",
            Self::TransitionProposed { .. } => "transition_proposed",
            Self::GuardChecked { .. } => "guard_checked",
            Self::NavigatorInvoked { .. } => "navigator_invoked",
            Self::TransitionCommitted { .. } => "transition_committed",
            Self::Error { .. } => "error",
            Self::Note { .. } => "note",
            Self::RunFinished { .. } => "run_finished",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Totals` holds a [`Usage`] but must not *look* like it holds one: the
    /// two fields sit directly on `run_finished.totals` in every ledger ever
    /// written, `examples/proj-1487/ledger.jsonl` included.
    #[test]
    fn totals_are_flat_on_the_wire_in_both_directions() {
        let totals = Totals {
            usage: Usage {
                tokens: 12_400,
                cost_usd: 3.58,
            },
            wallclock_s: 3414,
            transitions: 10,
        };
        let json: serde_json::Value = serde_json::to_value(totals).unwrap();
        assert_eq!(json["cost_usd"], 3.58);
        assert_eq!(json["tokens"], 12_400);
        assert!(
            json.get("usage").is_none(),
            "no nesting on the wire: {json}"
        );

        // A ledger written before tokens were totalled at all — the shipped
        // example is one — still reads, at zero rather than as a parse error.
        let legacy: Totals =
            serde_json::from_str(r#"{"cost_usd":3.58,"wallclock_s":3414,"transitions":10}"#)
                .expect("a pre-tokens ledger still parses");
        assert_eq!(legacy.usage.tokens, 0);
        assert_eq!(legacy.usage.cost_usd, 3.58);
        assert_eq!(legacy.transitions, 10);
    }
}
