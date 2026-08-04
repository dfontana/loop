//! Folding the event log into "where are we".
//!
//! This lives in `core`, not `ledger`, because it is pure control-flow
//! semantics over a slice of events — no I/O — and the engine is its only
//! consumer. `ledger` owns getting events on and off disk; this owns
//! what they mean.

use std::collections::BTreeMap;

use crate::core::event::{Event, EventPayload, RunStatus, Totals};
use crate::core::machine::{Machine, StateId};
use crate::core::runner::Proposal;

impl Machine {
    /// Fold a ledger against *this* machine's loop heads.
    ///
    /// Lives here rather than in `machine.rs` so the IR stays free of the
    /// event schema, and exists at all because every caller that has a machine
    /// wants this: the engine, `CliStage`, and `status`/`recap` each used to
    /// write out the same `fold_with_loop_heads(events, &|s|
    /// machine.loop_with_head(s).is_some())` closure. Callers without a
    /// machine — a mid-edit `machine.fnl`, the digest — take the bare [`fold`],
    /// which reads every state as a loop head.
    pub fn fold(&self, events: &[Event]) -> RunState {
        fold_with_loop_heads(events, &|s| self.loop_with_head(s).is_some())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum FoldStatus {
    /// No `run_started` yet.
    NotStarted,
    Running,
    Finished(RunStatus),
}

/// Where `loop resume` picks up, per docs/02-how-it-works.md.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum ResumePoint {
    /// Nothing to do — the ledger ends in `run_finished`.
    Done,
    /// Start the run at the machine's entry state.
    #[default]
    Fresh,
    /// Enter (or re-enter) this state. `crashed` marks a `state_entered` with no
    /// following `worker_output` — the stage died mid-flight and re-runs from
    /// scratch, which is why stages must be idempotent (docs/02-how-it-works.md).
    EnterState { state: StateId, crashed: bool },
    /// A `worker_output` exists but no `transition_committed`: resume at the
    /// guard check for this proposal.
    GuardCheck { from: StateId, proposal: Proposal },
}

/// One committed hop, with the rationale the proposal behind it carried.
///
/// The pair the digest reports, kept by the fold rather than searched for
/// afterwards. `digest::render` used to fold, then walk the events again for
/// the commit indices, then walk *backwards from each one* looking for the
/// matching `transition_proposed` — three passes for two facts, and the
/// backwards walk was a nearest-match heuristic where the fold simply knows,
/// because it has the proposal in hand when the commit arrives.
#[derive(Clone, Debug, PartialEq)]
pub struct Hop {
    pub from: StateId,
    pub to: StateId,
    pub cycle: u32,
    /// `None` when the commit had no proposal in front of it — an escalation
    /// or an `on_fail: route`, both of which the harness commits directly.
    pub rationale: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct RunState {
    pub status: Option<RunStatus>,
    /// `None` before `run_started`; otherwise the state the run sits in.
    pub current: Option<StateId>,
    /// The state the last committed transition came *from* — what a stage
    /// prompt interpolates as `$PREV_STATE`.
    ///
    /// Derived here rather than by a second reverse scan beside the fold:
    /// `CliStage::context` folded the ledger and then walked it again looking
    /// for the very `transition_committed` the fold had just handled.
    pub prev_state: Option<StateId>,
    /// Every committed hop, in ledger order.
    pub hops: Vec<Hop>,
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

    /// Charge one spawn's usage to the run. The three roles all bill through
    /// here, so a fourth cannot be added without accounting for it.
    fn spend(&mut self, usage: crate::core::event::Usage) {
        self.totals.usage += usage;
    }
}

/// Fold the event log into the current run state.
///
/// Rules (docs/02-how-it-works.md):
/// - `state_entered` bumps `attempts[(state, cycle)]` and sets `current`.
/// - `transition_committed` sets `current = to`; the caller tells cycles apart
///   via [`fold_with_loop_heads`] — plain `fold` counts a re-entry of any state
///   as a cycle bump for that state, which is the machine-agnostic reading.
/// - `worker_output`, `guard_checked` (the Judge's spend), and
///   `navigator_invoked` accumulate `totals` and counters.
/// - every event carries `elapsed_s`, so `totals.wallclock_s` is the last
///   value seen rather than something only `run_finished` knows.
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
pub fn fold_with_loop_heads(events: &[Event], is_loop_head: &dyn Fn(&str) -> bool) -> RunState {
    /// Where the tail of the ledger leaves us, used to derive [`ResumePoint`].
    enum Tail {
        /// No `state_entered` has landed since the last commit (or ever) —
        /// either nothing has run yet, or the last thing that happened was a
        /// clean commit. Both resume the same way: enter `rs.current`, or start
        /// fresh when there isn't one.
        AtRest,
        /// `state_entered` with no `worker_output` yet — died mid-flight.
        EnteredWithoutOutput { state: StateId },
        /// `worker_output` landed but the worker never got as far as
        /// `transition_proposed` — treated the same as a mid-flight death.
        OutputWithoutProposal { state: StateId },
        /// A proposal exists; no `transition_committed` yet.
        ProposedWithoutCommit { from: StateId, proposal: Proposal },
    }

    let mut rs = RunState::default();
    let mut tail = Tail::AtRest;

    for e in events {
        // Wallclock is carried on the envelope rather than derived from `ts`,
        // so it survives a resume without charging the run for the hours it
        // spent stopped. Monotone by construction: every append stamps the
        // accumulator, so the last line seen is the total so far.
        rs.totals.wallclock_s = e.elapsed_s;
        match &e.payload {
            EventPayload::RunStarted { .. } => {}
            EventPayload::StateEntered(h) => {
                let (state, cycle) = (&h.state, h.cycle);
                *rs.attempts.entry((state.clone(), cycle)).or_insert(0) += 1;
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
                rs.spend(*usage);
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
            EventPayload::GuardChecked { usage, .. } => rs.spend(*usage),
            EventPayload::NavigatorInvoked { from, usage, .. } => {
                rs.navigator_invocations += 1;
                *rs.navigator_by_state.entry(from.clone()).or_insert(0) += 1;
                rs.spend(*usage);
            }
            EventPayload::TransitionCommitted { from, to, cycle } => {
                // The proposal this commit ratifies is whatever the tail is
                // holding — the control loop is propose → guard → commit, so
                // it is in hand right here.
                //
                // Matched on the edge, not merely on the tail being occupied:
                // `guard_checked` and `navigator_invoked` do not clear the
                // tail (a crash between either and the commit has to resume at
                // the guard, which needs the proposal), so a hop the *harness*
                // chose — an `on_fail: route`, a Navigator reroute, an
                // escalation — arrives here with the rejected proposal still
                // in hand. Attributing that proposal's rationale to a hop it
                // did not cause states the reason the run went to `debug` as
                // the reason a worker gave for wanting `done`, in the digest
                // every later stage reads.
                let rationale = match &tail {
                    Tail::ProposedWithoutCommit {
                        from: proposed_from,
                        proposal,
                    } if proposed_from == from && proposal.to.as_ref() == Some(to) => {
                        Some(proposal.rationale.clone())
                    }
                    _ => None,
                };
                rs.hops.push(Hop {
                    from: from.clone(),
                    to: to.clone(),
                    cycle: *cycle,
                    rationale,
                });
                rs.prev_state = Some(from.clone());
                rs.current = Some(to.clone());
                if is_loop_head(to) {
                    *rs.cycles.entry(to.clone()).or_insert(0) += 1;
                }
                rs.totals.transitions += 1;
                tail = Tail::AtRest;
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
            Tail::AtRest => match &rs.current {
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
        }
    };

    rs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::GuardOutcome;
    use crate::core::fixtures::{
        EventExt, committed, entered, guard_checked, proposed, started as started_for,
    };

    fn started() -> Event {
        started_for("T-1")
    }

    /// The fold's cost accounting reads `worker_output`, so this fixture's
    /// usage and artifact are load-bearing where the shared defaults are not.
    fn output(state: &str, cycle: u32) -> Event {
        crate::core::fixtures::output(state, cycle)
            .usage(100, 0.5)
            .artifact(
                "diff",
                &format!(".loop/artifacts/{state}-{cycle}-diff.patch"),
            )
    }

    fn finished(status: RunStatus, terminal: &str) -> Event {
        crate::core::fixtures::finished(status, terminal).totals(Totals {
            usage: crate::core::Usage {
                cost_usd: 1.23,
                ..Default::default()
            },
            wallclock_s: 60,
            transitions: 2,
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

    /// `$PREV_STATE` and the digest's transition list both come off the fold
    /// now, rather than from a second reverse walk beside it and a third
    /// backwards search for each commit's proposal.
    #[test]
    fn the_fold_carries_prev_state_and_every_hop_with_its_rationale() {
        let events = vec![
            started(),
            entered("implement", 1, 1),
            output("implement", 1),
            proposed("implement", "review").rationale("plan is addressed"),
            guard_checked("implement", "review"),
            committed("implement", "review", 1),
            entered("review", 1, 1),
            output("review", 1),
            proposed("review", "done").rationale("no blocking defects"),
            committed("review", "done", 1),
        ];
        let rs = fold(&events);

        assert_eq!(rs.prev_state.as_deref(), Some("review"));
        assert_eq!(
            rs.hops,
            vec![
                Hop {
                    from: "implement".into(),
                    to: "review".into(),
                    cycle: 1,
                    rationale: Some("plan is addressed".into()),
                },
                Hop {
                    from: "review".into(),
                    to: "done".into(),
                    cycle: 1,
                    rationale: Some("no blocking defects".into()),
                },
            ]
        );
    }

    /// A commit the harness took itself — an escalation, an `on_fail: route` —
    /// has no proposal in front of it, and must not inherit the rationale of
    /// whatever was proposed earlier.
    #[test]
    fn a_hop_with_no_proposal_behind_it_carries_no_rationale() {
        let events = vec![
            started(),
            entered("qa", 1, 1),
            output("qa", 1),
            proposed("qa", "done").rationale("looks shippable"),
            guard_checked("qa", "done"),
            committed("qa", "done", 1),
            // The harness routing on its own account, with nothing pending.
            committed("done", "blocked", 1),
        ];
        let rs = fold(&events);

        assert_eq!(rs.hops.len(), 2);
        assert_eq!(rs.hops[0].rationale.as_deref(), Some("looks shippable"));
        assert_eq!(
            rs.hops[1].rationale, None,
            "nobody proposed this hop, so it must not borrow the last one's words"
        );
    }

    /// The harder half of the same rule. `guard_checked` and
    /// `navigator_invoked` leave the tail occupied on purpose — a crash
    /// between either and the commit has to resume at the guard, which needs
    /// the proposal — so a hop the harness redirects arrives with the
    /// *rejected* proposal still in hand. Matching the edge is what keeps it
    /// from being read as the reason for a hop it did not cause.
    #[test]
    fn a_rejected_proposal_does_not_lend_its_rationale_to_the_reroute() {
        // `on_fail: route`: the worker wanted `done`, the Judge said no, and
        // the harness sent the run to `debug` instead.
        let routed = fold(&[
            started(),
            entered("implement", 1, 1),
            output("implement", 1),
            proposed("implement", "done").rationale("all criteria met"),
            guard_checked("implement", "done").guards(GuardOutcome::Skip, GuardOutcome::Fail),
            committed("implement", "debug", 1),
        ]);
        assert_eq!(routed.hops.len(), 1);
        assert_eq!(
            routed.hops[0].rationale, None,
            "the run went to debug *because the judge failed it*, not because \
             a worker said the criteria were met"
        );

        // A Navigator reroute: the worker blocked, and the Navigator picked
        // somewhere other than what the block proposed.
        let rerouted = fold(&[
            started(),
            entered("implement", 1, 1),
            output("implement", 1),
            proposed("implement", "done").rationale("I think this is finished"),
            crate::core::fixtures::navigator("implement", "debug"),
            committed("implement", "debug", 1),
        ]);
        assert_eq!(rerouted.hops[0].rationale, None);

        // And the ratifying case still works through a guard, which is the
        // whole reason the tail is not simply cleared.
        let ratified = fold(&[
            started(),
            entered("implement", 1, 1),
            output("implement", 1),
            proposed("implement", "review").rationale("ready for review"),
            guard_checked("implement", "review"),
            committed("implement", "review", 1),
        ]);
        assert_eq!(
            ratified.hops[0].rationale.as_deref(),
            Some("ready for review")
        );
    }

    #[test]
    fn navigator_and_worker_usage_accumulate_totals() {
        let events = vec![
            started(),
            entered("debug", 1, 1),
            output("debug", 1), // cost_usd 0.5
            proposed("debug", "implement"),
            crate::core::fixtures::navigator("debug", "qa_staging").usage(10, 0.05),
            committed("debug", "qa_staging", 1),
        ];
        let rs = fold(&events);
        assert!((rs.totals.usage.cost_usd - 0.55).abs() < 1e-9);
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
    use crate::core::fixtures::{committed, entered};

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
