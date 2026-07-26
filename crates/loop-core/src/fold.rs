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
    /// Where the tail of the ledger leaves us, used to derive [`ResumePoint`].
    enum Tail {
        /// No `state_entered` has landed since the last commit (or ever).
        Initial,
        /// `state_entered` with no `worker_output` yet — died mid-flight.
        EnteredWithoutOutput { state: StateId },
        /// `worker_output` landed but the worker never got as far as
        /// `transition_proposed` — treated the same as a mid-flight death.
        OutputWithoutProposal { state: StateId },
        /// A proposal exists; no `transition_committed` yet.
        ProposedWithoutCommit { from: StateId, proposal: Proposal },
        /// The last thing that happened was a clean commit.
        Committed,
    }

    let mut rs = RunState::default();
    let mut tail = Tail::Initial;

    for e in events {
        match &e.payload {
            EventPayload::RunStarted { .. } => {}
            EventPayload::StateEntered { state, cycle, .. } => {
                *rs.attempts.entry((state.clone(), *cycle)).or_insert(0) += 1;
                rs.current = Some(state.clone());
                // A loop head reached by an edge got its cycle bumped by the
                // `transition_committed` below. A loop head that is also the
                // machine's *entry* is never committed into, so its first
                // entry has to seed the counter here — otherwise it sits at 0
                // through cycle one and every later cycle is off by one, which
                // silently buys the loop an extra `max_cycles` iteration.
                // Guarded on `== 0`, so a retry (a new attempt, not a new
                // cycle) never bumps it.
                if is_loop_head(state) && rs.cycles.get(state).copied().unwrap_or(0) == 0 {
                    rs.cycles.insert(state.clone(), 1);
                }
                tail = Tail::EnteredWithoutOutput {
                    state: state.clone(),
                };
            }
            EventPayload::WorkerOutput {
                state,
                artifacts,
                usage,
                ..
            } => {
                rs.totals.cost_usd += usage.cost_usd;
                for a in artifacts {
                    rs.artifacts.insert(a.name.clone(), a.path.clone());
                }
                tail = Tail::OutputWithoutProposal {
                    state: state.clone(),
                };
            }
            EventPayload::TransitionProposed {
                from,
                to,
                blocked,
                rationale,
                by: _,
            } => {
                let proposal = Proposal {
                    to: to.clone(),
                    blocked: *blocked,
                    rationale: rationale.clone(),
                    artifacts: Vec::new(),
                };
                tail = Tail::ProposedWithoutCommit {
                    from: from.clone(),
                    proposal,
                };
            }
            EventPayload::GuardChecked { .. } => {}
            EventPayload::NavigatorInvoked { from, usage, .. } => {
                rs.navigator_invocations += 1;
                *rs.navigator_by_state.entry(from.clone()).or_insert(0) += 1;
                rs.totals.cost_usd += usage.cost_usd;
            }
            EventPayload::TransitionCommitted { to, .. } => {
                rs.current = Some(to.clone());
                if is_loop_head(to) {
                    *rs.cycles.entry(to.clone()).or_insert(0) += 1;
                }
                rs.totals.transitions += 1;
                tail = Tail::Committed;
            }
            EventPayload::Error { .. } => {}
            EventPayload::Note { .. } => {}
            EventPayload::RunFinished {
                status,
                terminal_state,
                totals,
            } => {
                rs.status = Some(*status);
                if terminal_state.is_some() {
                    rs.current = terminal_state.clone();
                }
                rs.totals = *totals;
                // Nothing after `run_finished` is read.
                break;
            }
        }
    }

    rs.resume = if rs.status.is_some() {
        ResumePoint::Done
    } else {
        match tail {
            Tail::Initial => match &rs.current {
                Some(state) => ResumePoint::EnterState {
                    state: state.clone(),
                    crashed: false,
                },
                None => ResumePoint::Fresh,
            },
            Tail::EnteredWithoutOutput { state } | Tail::OutputWithoutProposal { state } => {
                ResumePoint::EnterState {
                    state,
                    crashed: true,
                }
            }
            Tail::ProposedWithoutCommit { from, proposal } => {
                ResumePoint::GuardCheck { from, proposal }
            }
            Tail::Committed => match &rs.current {
                Some(state) => ResumePoint::EnterState {
                    state: state.clone(),
                    crashed: false,
                },
                None => ResumePoint::Fresh,
            },
        }
    };

    rs
}

/// Convenience for `status`: the last event of a given kind.
pub fn last_of<'e>(events: &'e [Event], kind: &str) -> Option<&'e EventPayload> {
    events
        .iter()
        .rev()
        .find(|e| e.kind() == kind)
        .map(|e| &e.payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{ArtifactRef, GuardOutcome, Usage};
    use crate::machine::Budgets;

    fn ev(payload: EventPayload) -> Event {
        Event::now(payload)
    }

    fn started() -> Event {
        ev(EventPayload::RunStarted {
            ticket: "T-1".into(),
            machine_hash: "sha256:deadbeef".into(),
            budgets: Budgets::default(),
        })
    }

    fn entered(state: &str, cycle: u32, attempt: u32) -> Event {
        ev(EventPayload::StateEntered {
            state: state.into(),
            cycle,
            attempt,
            session_id: None,
            model: "m".into(),
            thinking: "low".into(),
            tools: vec![],
        })
    }

    fn output(state: &str, cycle: u32) -> Event {
        ev(EventPayload::WorkerOutput {
            state: state.into(),
            cycle,
            summary: "did stuff".into(),
            artifacts: vec![ArtifactRef {
                name: "diff".into(),
                path: format!(".loop/artifacts/{state}-{cycle}-diff.patch"),
                sha256: "abc123".into(),
            }],
            usage: Usage {
                tokens: 100,
                cost_usd: 0.5,
            },
        })
    }

    fn proposed(from: &str, to: &str) -> Event {
        ev(EventPayload::TransitionProposed {
            from: from.into(),
            to: Some(to.into()),
            blocked: false,
            rationale: "looks good".into(),
            by: crate::event::Actor::Worker,
        })
    }

    fn guard_checked(from: &str, to: &str) -> Event {
        ev(EventPayload::GuardChecked {
            from: from.into(),
            to: to.into(),
            structural: GuardOutcome::Pass,
            criteria: GuardOutcome::Skip,
            judge_rationale: None,
        })
    }

    fn committed(from: &str, to: &str, cycle: u32) -> Event {
        ev(EventPayload::TransitionCommitted {
            from: from.into(),
            to: to.into(),
            cycle,
        })
    }

    fn finished(status: RunStatus, terminal: &str) -> Event {
        ev(EventPayload::RunFinished {
            status,
            terminal_state: Some(terminal.into()),
            totals: Totals {
                cost_usd: 1.23,
                wallclock_s: 60,
                transitions: 2,
            },
        })
    }

    #[test]
    fn fresh_run_has_no_events() {
        let rs = fold(&[]);
        assert_eq!(rs.resume, ResumePoint::Fresh);
        assert!(rs.status.is_none());
        assert!(rs.current.is_none());
    }

    #[test]
    fn fresh_run_started_but_nothing_else() {
        // A crash right after `run_started`, before any `state_entered`.
        let events = vec![started()];
        let rs = fold(&events);
        assert_eq!(rs.resume, ResumePoint::Fresh);
    }

    #[test]
    fn mid_run_tracks_current_cycles_and_attempts() {
        let events = vec![
            started(),
            entered("implement", 1, 1),
            output("implement", 1),
            proposed("implement", "review"),
            guard_checked("implement", "review"),
            committed("implement", "review", 1),
            entered("review", 1, 1),
        ];
        let rs = fold_with_loop_heads(&events, &|s| s == "qa_staging");
        assert_eq!(rs.current.as_deref(), Some("review"));
        assert_eq!(rs.attempts_of("implement", 1), 1);
        assert_eq!(rs.attempts_of("review", 1), 1);
        // `implement` isn't a declared loop head in this fold, so its cycle
        // counter never bumps.
        assert_eq!(rs.cycle_of("implement"), 0);
        assert_eq!(
            rs.resume,
            ResumePoint::EnterState {
                state: "review".into(),
                crashed: true,
            }
        );
    }

    #[test]
    fn crashed_mid_stage_has_no_worker_output() {
        let events = vec![started(), entered("implement", 1, 1)];
        let rs = fold(&events);
        assert_eq!(
            rs.resume,
            ResumePoint::EnterState {
                state: "implement".into(),
                crashed: true,
            }
        );
    }

    #[test]
    fn worker_output_without_commit_resumes_at_guard_check() {
        let events = vec![
            started(),
            entered("implement", 1, 1),
            output("implement", 1),
            proposed("implement", "review"),
        ];
        let rs = fold(&events);
        match rs.resume {
            ResumePoint::GuardCheck { from, proposal } => {
                assert_eq!(from, "implement");
                assert_eq!(proposal.to.as_deref(), Some("review"));
                assert!(!proposal.blocked);
            }
            other => panic!("expected GuardCheck, got {other:?}"),
        }
    }

    #[test]
    fn worker_output_without_proposal_is_treated_as_crashed() {
        let events = vec![
            started(),
            entered("implement", 1, 1),
            output("implement", 1),
        ];
        let rs = fold(&events);
        assert_eq!(
            rs.resume,
            ResumePoint::EnterState {
                state: "implement".into(),
                crashed: true,
            }
        );
    }

    #[test]
    fn clean_commit_with_no_next_entry_resumes_fresh_into_current() {
        let events = vec![
            started(),
            entered("implement", 1, 1),
            output("implement", 1),
            proposed("implement", "review"),
            guard_checked("implement", "review"),
            committed("implement", "review", 1),
        ];
        let rs = fold(&events);
        assert_eq!(
            rs.resume,
            ResumePoint::EnterState {
                state: "review".into(),
                crashed: false,
            }
        );
    }

    #[test]
    fn completed_run_resumes_done() {
        let events = vec![
            started(),
            entered("open_pr", 1, 1),
            output("open_pr", 1),
            proposed("open_pr", "done"),
            guard_checked("open_pr", "done"),
            committed("open_pr", "done", 1),
            finished(RunStatus::Done, "done"),
        ];
        let rs = fold(&events);
        assert_eq!(rs.resume, ResumePoint::Done);
        assert_eq!(rs.status, Some(RunStatus::Done));
        assert_eq!(rs.current.as_deref(), Some("done"));
        assert_eq!(rs.totals.transitions, 2); // from `run_finished`'s totals
    }

    #[test]
    fn loop_head_cycle_only_bumps_for_declared_heads() {
        let events = vec![
            started(),
            entered("review", 1, 1),
            output("review", 1),
            proposed("review", "qa_staging"),
            guard_checked("review", "qa_staging"),
            committed("review", "qa_staging", 1), // 1st commit *into* qa_staging
            entered("qa_staging", 1, 1),
            output("qa_staging", 1),
            proposed("qa_staging", "qa_staging"),
            guard_checked("qa_staging", "qa_staging"),
            committed("qa_staging", "qa_staging", 1), // 2nd commit *into* qa_staging
            entered("qa_staging", 2, 1),
            output("qa_staging", 2),
            proposed("qa_staging", "debug"),
            guard_checked("qa_staging", "debug"),
            committed("qa_staging", "debug", 2), // commit *into* debug, not qa_staging
        ];
        let rs = fold_with_loop_heads(&events, &|s| s == "qa_staging");
        assert_eq!(rs.cycle_of("qa_staging"), 2);
        assert_eq!(rs.current.as_deref(), Some("debug"));
        // `debug` was never declared a loop head, so it never accrues a cycle
        // count even though it was just committed into.
        assert_eq!(rs.cycle_of("debug"), 0);

        // The machine-agnostic plain `fold` (is_loop_head always true) counts
        // a commit into *any* state as a cycle bump for that state.
        let plain = fold(&events);
        assert_eq!(plain.cycle_of("qa_staging"), 2);
        assert_eq!(plain.cycle_of("debug"), 1);
    }

    #[test]
    fn navigator_and_worker_usage_accumulate_totals() {
        let events = vec![
            started(),
            entered("debug", 1, 1),
            output("debug", 1), // cost_usd 0.5
            proposed("debug", "implement"),
            ev(EventPayload::NavigatorInvoked {
                from: "debug".into(),
                proposal: "blocked".into(),
                chosen_to: "qa_staging".into(),
                entry_prompt: Some("try again".into()),
                usage: Usage {
                    tokens: 10,
                    cost_usd: 0.05,
                },
            }),
            committed("debug", "qa_staging", 1),
        ];
        let rs = fold(&events);
        assert!((rs.totals.cost_usd - 0.55).abs() < 1e-9);
        assert_eq!(rs.totals.transitions, 1);
        assert_eq!(rs.navigator_invocations, 1);
        assert_eq!(rs.navigator_from("debug"), 1);
        assert_eq!(
            rs.artifacts.get("diff").map(String::as_str),
            Some(".loop/artifacts/debug-1-diff.patch")
        );
    }
}

#[cfg(test)]
mod entry_head_tests {
    use super::*;
    use crate::event::{Event, EventPayload};

    fn ev(payload: EventPayload) -> Event {
        Event {
            ts: "2026-07-24T00:00:00.000Z".into(),
            payload,
        }
    }

    fn entered(state: &str, cycle: u32, attempt: u32) -> Event {
        ev(EventPayload::StateEntered {
            state: state.into(),
            cycle,
            attempt,
            session_id: None,
            model: "m".into(),
            thinking: "low".into(),
            tools: vec![],
        })
    }

    fn committed(from: &str, to: &str, cycle: u32) -> Event {
        ev(EventPayload::TransitionCommitted {
            from: from.into(),
            to: to.into(),
            cycle,
        })
    }

    /// A loop head that is also the entry state — the shape the shipped
    /// `standard-ticket` template has. Its first entry is never committed
    /// into, so without seeding at `state_entered` it would still read as
    /// cycle 0 partway through cycle two, handing the loop a free iteration
    /// past `max_cycles`.
    #[test]
    fn entry_state_that_is_a_loop_head_counts_its_first_entry() {
        let head = |s: &str| s == "implement";

        let rs = fold_with_loop_heads(&[entered("implement", 1, 1)], &head);
        assert_eq!(rs.cycle_of("implement"), 1, "first entry is cycle 1");

        let rs = fold_with_loop_heads(
            &[
                entered("implement", 1, 1),
                committed("implement", "review", 1),
                entered("review", 1, 1),
                committed("review", "implement", 1),
                entered("implement", 2, 1),
            ],
            &head,
        );
        assert_eq!(rs.cycle_of("implement"), 2, "back-edge starts cycle 2");
    }

    /// A retry is a new attempt, not a new cycle — the seeding must not fire
    /// twice for the same entry.
    #[test]
    fn retrying_the_entry_head_does_not_advance_the_cycle() {
        let head = |s: &str| s == "implement";
        let rs = fold_with_loop_heads(
            &[entered("implement", 1, 1), entered("implement", 1, 2)],
            &head,
        );
        assert_eq!(rs.cycle_of("implement"), 1);
        assert_eq!(rs.attempts_of("implement", 1), 2);
    }

    /// A loop head reached by an edge is bumped by the commit, and must not be
    /// double-counted by the subsequent `state_entered`.
    #[test]
    fn loop_head_reached_by_an_edge_is_counted_once() {
        let head = |s: &str| s == "qa";
        let rs = fold_with_loop_heads(
            &[
                entered("implement", 1, 1),
                committed("implement", "qa", 1),
                entered("qa", 1, 1),
            ],
            &head,
        );
        assert_eq!(rs.cycle_of("qa"), 1);
    }
}
