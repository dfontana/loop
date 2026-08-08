//! The control loop — the deterministic half of the system.
//!
//! skills/loop-authoring/references/runtime.md is the spec; this module is its transcription. The
//! loop body contains no LLM call: every decision an agent makes (the worker's
//! proposal, the judge's verdict, the navigator's choice) arrives through a
//! fixed contract, is recorded, and is bounded.
//!
//! This module reaches for nothing but [`crate::core`] and its traits, so the
//! entire control flow is testable against in-process fakes — no Lua, no
//! subprocess, no API key, no filesystem. That used to be a fact about the
//! crate graph: `loop-engine` listed one dependency and cargo refused to
//! compile a reach into `loop-fennel` or `loop-runner`. Now it is a convention,
//! and the thing standing behind it is `tests/` — a suite that runs the whole
//! machine in-process, in microseconds, on a struct of trait objects. An import
//! from `fennel`, `runner`, `ledger` or `toolbox` added here would compile
//! fine; it would also mean those tests have to spin up a Lua VM or spawn a
//! process to keep passing, and that is the cost that keeps this honest.
//! Nothing else does. Treat a new `use crate::` line in this module as a design
//! change, not a convenience.

use crate::core::{
    AgentRunner, Artifact, ArtifactSink, CheckRunner, ESCALATE, ErrorKind, Event, EventPayload,
    FoldStatus, LedgerSink, LoopSpec, Machine, OnExhausted, OnFail, Proposal, Result, ResumePoint,
    RunState, RunStatus, StateId, Totals, Transition, with_stderr_tail,
};

pub mod guards;
pub mod mermaid;
pub mod prompts;
pub mod validate;

use guards::check as guard_check;

pub use mermaid::mermaid;
pub use prompts::{StageBuilder, StagePlan};
pub use validate::{Diagnostic, Severity, validate};

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;

/// Everything the loop needs that isn't the machine itself. The engine borrows
/// its collaborators as traits; the CLI supplies the real ones.
pub struct Engine<'a> {
    pub machine: &'a Machine,
    // No `config` field. Everything the loop body would want from it has
    // already been folded into the `Machine` or into `stage` by the time the
    // engine runs, so holding one only made `Engine`'s signature stop
    // describing what the loop actually consults.
    pub runner: &'a dyn AgentRunner,
    /// Runs a transition's deterministic check — the harness acting for
    /// itself, with no agent in the path.
    pub checks: &'a dyn CheckRunner,
    pub ledger: &'a mut dyn LedgerSink,
    pub artifacts: &'a dyn ArtifactSink,
    /// Renders prompts and assembles spawn specs. Implemented in the CLI over
    /// `toolbox`, so the engine stays free of filesystem concerns.
    pub stage: &'a dyn StageBuilder,
    /// Wall-clock start, for the budget check. `None` starts it at `run()`.
    pub started_at: Option<std::time::Instant>,
    /// Run seconds this ledger had already accumulated before this process
    /// opened it — `Ledger::elapsed_offset_s`. The time budget bounds the
    /// *run*, not the process, so a resume has to start counting from what the
    /// interrupted session burned rather than from zero.
    pub elapsed_offset_s: u64,
}

/// How many times a stage may die mid-flight before the run escalates rather
/// than retrying. A crash is worth retrying — a flaky spawn, an OOM, a dropped
/// connection — but a stage that dies every time is a real fault, and spinning
/// on it burns budget silently, which is the failure docs/design-notes.md exists to stop.
const MAX_CRASH_ATTEMPTS: u32 = 3;

/// How a finished run came out — the `run_finished` payload, owned.
///
/// Deliberately the same three fields, because it *is* that line: `finish_now`
/// appends the event and returns this, and `step` reconstitutes one from a
/// ledger that already ends in it. Kept as its own type only so `run()` has a
/// return type that isn't an enum with nine irrelevant variants;
/// [`Outcome::of`] and [`Outcome::payload`] are the two conversions, so the
/// field list is written once on each side rather than at every construction.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    pub status: RunStatus,
    pub terminal_state: Option<String>,
    pub totals: Totals,
}

impl Outcome {
    /// The outcome a ledger's own closing line records.
    fn of(f: crate::core::RunFinish<'_>) -> Self {
        Self {
            status: f.status,
            terminal_state: f.terminal_state.map(str::to_string),
            totals: *f.totals,
        }
    }

    /// The event that records it.
    fn payload(&self) -> EventPayload {
        EventPayload::RunFinished {
            status: self.status,
            terminal_state: self.terminal_state.clone(),
            totals: self.totals,
        }
    }
}

impl Engine<'_> {
    /// Drive the machine to a terminal, appending every decision to the ledger.
    ///
    /// The loop, per skills/loop-authoring/references/runtime.md:
    ///
    /// 1. Fold the ledger. A fresh run appends `run_started`; a resume picks up at the folded
    ///    [`crate::core::ResumePoint`].
    /// 2. If the current state is terminal → `run_finished`, done.
    /// 3. Check global guardrails (wallclock, `$`, transition count) **before**
    ///    spawning. Exceeded → `run_finished{aborted}` naming the guardrail.
    /// 4. `state_entered`, build the stage, spawn the worker, append
    ///    `worker_output` (capturing artifacts first, so a crash between them
    ///    loses nothing).
    /// 5. `transition_proposed`. If blocked, or (in `open` mode) the target is
    ///    not a declared edge → Navigator, capped per run and per state; over
    ///    the cap → escalate.
    /// 6. Guard tiers on the chosen edge: structural, then `criteria` via the
    ///    Judge. A failure runs the edge's `on_fail`.
    /// 7. `transition_committed`, honor `backoff_s`, bump cycle counters, and
    ///    enforce `max_cycles` — on exhaustion, `on_exhausted`.
    ///
    /// Every step appends before it acts, so a crash anywhere is resumable.
    pub fn run(&mut self) -> Result<Outcome> {
        self.started_at.get_or_insert_with(std::time::Instant::now);
        loop {
            if let Some(outcome) = self.step()? {
                return Ok(outcome);
            }
        }
    }

    /// One iteration. Returns `Some` when the run reached a terminal. Exposed so
    /// tests can step the machine and assert on the ledger between steps.
    pub fn step(&mut self) -> Result<Option<Outcome>> {
        // Read once per iteration. Everything appended below is pushed onto
        // this by `record`, so the loop body never reads the file again.
        let mut events = self.ledger.read_all()?;
        let has_run_started = events
            .iter()
            .any(|e| matches!(e.payload, EventPayload::RunStarted { .. }));
        let rs = self.machine.fold(&events);

        // Read off the closing line itself, not rebuilt from the fold. The
        // fold leaves `current` at the last state it saw when `run_finished`
        // named no terminal, so rebuilding here reported a terminal state for a
        // run that recorded none — and disagreed with the `Outcome` the
        // in-process `finish_now` returns for that same ledger. One resume and
        // the answer changed.
        if let FoldStatus::Finished(_) = rs.fold_status() {
            return Ok(crate::core::run_finished(&events).map(Outcome::of));
        }

        if !has_run_started {
            self.append_run_started(&mut events)?;
        }

        match rs.resume.clone() {
            ResumePoint::Done => unreachable!("Finished handled above"),
            ResumePoint::Fresh => {
                let entry = self.machine.entry.clone();
                self.enter_state(&rs, entry, false, &mut events)
            }
            ResumePoint::EnterState { state, crashed } => {
                self.enter_state(&rs, state, crashed, &mut events)
            }
            ResumePoint::GuardCheck { from, proposal } => {
                if let Some(outcome) = self.check_budgets(&rs, &mut events)? {
                    return Ok(Some(outcome));
                }
                self.route_proposal(&rs, &from, proposal, &mut events)
            }
        }
    }

    // ── internal ─────────────────────────────────────────────────────────

    /// The cycle a state is on, and how many attempts it has already had there.
    ///
    /// The one derivation of "where is this state up to", shared by the entry
    /// path (which wants `attempts + 1` — the attempt it is about to make) and
    /// the guard path (which wants `attempts.max(1)` — the attempt that just
    /// finished). Computing the cycle half twice is how the two drift.
    fn position_of(&self, rs: &RunState, state: &str) -> (u32, u32) {
        let cycle = if self.machine.loop_with_head(state).is_some() {
            rs.cycle_of(state).max(1)
        } else {
            1
        };
        (cycle, rs.attempts_of(state, cycle))
    }

    /// Append, and keep the event.
    ///
    /// The loop body folds the ledger several times inside one `step` — after
    /// the worker's events land, and again on an `on_fail` retry — and each of
    /// those used to re-`read_all`, so a single iteration read and re-parsed the
    /// whole growing file three or four times over. `LedgerSink::append` hands
    /// the stamped event back, so a caller holding the ledger's contents can
    /// keep them current instead: `events` is the file, and stays the file.
    fn record(&mut self, events: &mut Vec<Event>, payload: EventPayload) -> Result<()> {
        let event = self.ledger.append(payload)?;
        events.push(event);
        Ok(())
    }

    fn elapsed_s(&self) -> u64 {
        self.elapsed_offset_s + self.started_at.map(|t| t.elapsed().as_secs()).unwrap_or(0)
    }

    fn totals_now(&self, rs: &RunState) -> Totals {
        Totals {
            wallclock_s: self.elapsed_s(),
            ..rs.totals
        }
    }

    fn append_run_started(&mut self, events: &mut Vec<Event>) -> Result<()> {
        self.record(
            events,
            EventPayload::RunStarted {
                ticket: self.machine.ticket.clone(),
                machine_hash: self.machine.source_hash.clone(),
                budgets: self.machine.budgets,
            },
        )?;
        Ok(())
    }

    /// The global guardrails — `$`, wallclock, total transitions — checked
    /// *before* any spawn. Exceeding one aborts the run, naming the guardrail
    /// via an `error` line right before `run_finished`.
    fn check_budgets(&mut self, rs: &RunState, events: &mut Vec<Event>) -> Result<Option<Outcome>> {
        let b = &self.machine.budgets;
        let spent = rs.totals.usage.cost_usd;
        let elapsed = self.elapsed_s();
        let moves = rs.totals.transitions;

        // First one to bite names the run's cause of death, so the order here
        // is the order they are reported in.
        let reason = b
            .usd
            .filter(|&usd| spent > usd)
            .map(|usd| format!("budget_usd exceeded: ${spent:.2} > ${usd:.2}"))
            .or_else(|| {
                b.wallclock_s
                    .filter(|&wc| elapsed > wc)
                    .map(|wc| format!("wallclock_s exceeded: {elapsed}s > {wc}s"))
            })
            .or_else(|| {
                b.max_transitions
                    .filter(|&mt| moves >= mt)
                    .map(|mt| format!("max_transitions exceeded: {moves} >= {mt}"))
            });

        let Some(reason) = reason else {
            return Ok(None);
        };
        self.record(
            events,
            EventPayload::Error {
                state: rs.current.clone(),
                kind: ErrorKind::Fatal,
                detail: reason,
            },
        )?;
        self.finish_now(rs, RunStatus::Aborted, rs.current.clone(), events)
    }

    /// Append `run_finished` directly — used by budget-exhausted, `on_fail:
    /// abort`, and `on_exhausted: abort` paths, none of which route through a
    /// declared terminal state.
    fn finish_now(
        &mut self,
        rs: &RunState,
        status: RunStatus,
        terminal_state: Option<StateId>,
        events: &mut Vec<Event>,
    ) -> Result<Option<Outcome>> {
        let outcome = Outcome {
            status,
            terminal_state,
            totals: self.totals_now(rs),
        };
        self.record(events, outcome.payload())?;
        Ok(Some(outcome))
    }

    /// Route to the machine's escalation state — a harness override, not a
    /// worker choice, so it bypasses guard checks. Falls back to an aborted
    /// run if no `escalation_state` is configured.
    fn escalate(
        &mut self,
        rs: &RunState,
        from: &str,
        events: &mut Vec<Event>,
    ) -> Result<Option<Outcome>> {
        match self.machine.escalation_state.clone() {
            Some(esc) => self.commit(rs, from, &esc, None, events),
            None => self.finish_now(rs, RunStatus::Aborted, Some(from.to_string()), events),
        }
    }

    /// A loop's head has been re-entered past `max_cycles`.
    fn handle_exhausted(
        &mut self,
        rs: &RunState,
        from: &str,
        l: &LoopSpec,
        events: &mut Vec<Event>,
    ) -> Result<Option<Outcome>> {
        self.record(
            events,
            EventPayload::Error {
                state: Some(from.to_string()),
                kind: ErrorKind::Fatal,
                detail: format!(
                    "loop `{}` exhausted max_cycles={} at head `{}`",
                    l.name,
                    l.max_cycles,
                    l.head().map(|s| s.as_str()).unwrap_or("?")
                ),
            },
        )?;
        match l.on_exhausted {
            OnExhausted::Escalate => self.escalate(rs, from, events),
            OnExhausted::Abort => {
                self.finish_now(rs, RunStatus::Aborted, Some(from.to_string()), events)
            }
        }
    }

    /// An edge's `max_attempts` is spent: the source state has run and failed
    /// this same edge that many times in one cycle.
    ///
    /// Nothing else bounds this. A retry writes no `transition_committed`, so a
    /// loop head's cycle counter never advances and `max_cycles` never bites;
    /// before this existed, an edge whose `check` could not pass re-spawned the
    /// stage until the dollar budget aborted the run — measured at 200 spawns of
    /// one stage against the bundled machine's $8. Crashes were already capped
    /// this way ([`MAX_CRASH_ATTEMPTS`]); a guard that keeps failing is the same
    /// shape of problem and gets the same treatment.
    fn handle_attempts_exhausted(
        &mut self,
        rs: &RunState,
        from: &str,
        edge: &Transition,
        attempts: u32,
        events: &mut Vec<Event>,
    ) -> Result<Option<Outcome>> {
        self.record(
            events,
            EventPayload::Error {
                state: Some(from.to_string()),
                kind: ErrorKind::Fatal,
                detail: format!(
                    "edge `{}` -> `{}` exhausted max_attempts={} ({attempts} attempt(s) at \
                     `{from}` failed its guard)",
                    edge.from, edge.to, edge.max_attempts
                ),
            },
        )?;
        self.escalate(rs, from, events)
    }

    /// Commit a transition. Prospectively enforces `max_cycles` when `to` is a
    /// loop head — the head's cycle counter only ever advances via a
    /// committed transition, so this is the one place the bound can bite.
    /// Honors the edge's `backoff_s`, when one is given.
    fn commit(
        &mut self,
        rs: &RunState,
        from: &str,
        to: &str,
        edge: Option<&Transition>,
        events: &mut Vec<Event>,
    ) -> Result<Option<Outcome>> {
        if let Some(l) = self.machine.loop_with_head(to) {
            if rs.cycle_of(to) + 1 > l.max_cycles {
                return self.handle_exhausted(rs, from, l, events);
            }
        }

        let cycle_for_event = rs.cycle_of(from).max(1);
        self.record(
            events,
            EventPayload::TransitionCommitted {
                from: from.to_string(),
                to: to.to_string(),
                cycle: cycle_for_event,
            },
        )?;

        if let Some(edge) = edge {
            if let Some(secs) = edge.backoff_s {
                std::thread::sleep(std::time::Duration::from_secs(secs));
            }
        }

        Ok(None)
    }

    /// `on_fail`: retry re-enters the source state (attempt+1, same cycle);
    /// route sends it to a named state directly (a harness override, no
    /// further guard check); abort finishes the run failed.
    fn handle_on_fail(
        &mut self,
        rs: &RunState,
        edge: &Transition,
        from: &str,
        events: &mut Vec<Event>,
    ) -> Result<Option<Outcome>> {
        match &edge.on_fail {
            OnFail::Retry => {
                // The guard's own events are already in `events`, so the fold
                // the retry needs costs no read.
                let fresh = self.machine.fold(events);
                // `position_of` counts the attempt that just failed, since its
                // `state_entered` is in `events` above.
                let (_, attempts) = self.position_of(&fresh, from);
                if attempts >= edge.max_attempts {
                    return self.handle_attempts_exhausted(rs, from, edge, attempts, events);
                }
                self.enter_state(&fresh, from.to_string(), false, events)
            }
            OnFail::Abort => self.finish_now(rs, RunStatus::Failed, Some(from.to_string()), events),
            OnFail::Route(target) => {
                let target = target.clone();
                self.commit(rs, from, &target, None, events)
            }
        }
    }

    /// Spawn the Navigator, capped per-run and per-state. Over the cap,
    /// escalates without spawning.
    fn invoke_navigator(
        &mut self,
        rs: &RunState,
        from: &str,
        proposal: Option<&Proposal>,
        events: &mut Vec<Event>,
    ) -> Result<NavOutcome> {
        let cap = self.machine.navigator_max_invocations;
        if rs.navigator_invocations >= cap || rs.navigator_from(from) >= cap {
            return Ok(NavOutcome::Done(self.escalate(rs, from, events)?));
        }
        let spec = self.stage.build_navigator(&from.to_string(), proposal)?;
        let choice = self.runner.run_navigator(&spec)?;
        self.record(
            events,
            EventPayload::NavigatorInvoked {
                from: from.to_string(),
                proposal: proposal.map(|p| p.rationale.clone()).unwrap_or_default(),
                chosen_to: choice.to.clone(),
                entry_prompt: choice.entry_prompt.clone(),
                usage: choice.usage,
            },
        )?;
        Ok(NavOutcome::Target(choice.to))
    }

    /// The tail of the loop body: given a settled `(from, proposal)` — fresh
    /// off the worker or recovered from a crash-resumed `GuardCheck` — decide
    /// the target (routing through the Navigator if needed), run the guard
    /// tiers, and commit or handle `on_fail`.
    fn route_proposal(
        &mut self,
        rs: &RunState,
        from: &str,
        proposal: Proposal,
        events: &mut Vec<Event>,
    ) -> Result<Option<Outcome>> {
        let needs_navigator = proposal.blocked
            || match &proposal.to {
                None => true,
                Some(t) => !self.machine.neighbors(from).contains(t),
            };

        let target = if needs_navigator {
            match self.invoke_navigator(rs, from, Some(&proposal), events)? {
                NavOutcome::Done(outcome) => return Ok(outcome),
                NavOutcome::Target(to) => to,
            }
        } else {
            proposal.to.clone().expect("checked by needs_navigator")
        };

        // Two routing outcomes resolve before any edge lookup, because neither
        // is an edge. The escalation state is a harness-declared override —
        // commit straight there, no guard tiers. [`ESCALATE`] is the Navigator
        // saying nothing reachable fits (or the harness saying the Navigator's
        // reply was unusable); `escalate` turns that into whichever of the two
        // endings the machine declares.
        //
        // Recognized here rather than falling through to the arm below: that
        // arm records a `Fatal`, and an escalation is an expected decision a
        // run routinely recovers from. Logging it as fatal put a line reading
        // like an internal invariant breach into the ledger of every run that
        // ever escalated, and handed `report::last_fatal` a false cause of
        // death to quote in `loop recap`.
        if target == ESCALATE {
            return self.escalate(rs, from, events);
        }
        if Some(&target) == self.machine.escalation_state.as_ref() {
            return self.commit(rs, from, &target, None, events);
        }

        let edge = match self.machine.edge(from, &target) {
            Some(e) => e.clone(),
            None => {
                // Genuinely unreachable now that `ESCALATE` is handled above:
                // the target came either from a declared edge or from the set
                // the Navigator was offered, and `parse_choice` matches that
                // set exactly. Stay total anyway. No guard ran, so this is an
                // error rather than a `guard_checked` with every tier skipped.
                self.record(
                    events,
                    EventPayload::Error {
                        state: Some(from.to_string()),
                        kind: ErrorKind::Fatal,
                        detail: format!(
                            "no declared edge `{from}` -> `{target}` after routing; escalating"
                        ),
                    },
                )?;
                return self.escalate(rs, from, events);
            }
        };

        let (worker_summary, worker_artifacts) = last_worker_output_for(events, from);
        // The attempt that just finished, not the next one.
        let (cycle, attempts) = self.position_of(rs, from);
        let attempt = attempts.max(1);

        // The Judge spec is built *after* the check runs so the verdict can be
        // grounded in what the check printed. Borrow `stage` out of `self`
        // first — the closure runs while `self.ledger` is still live.
        let stage = self.stage;
        let report = guard_check(
            self.runner,
            self.checks,
            &edge,
            &from.to_string(),
            cycle,
            attempt,
            |criteria, check_output| {
                stage.build_judge(criteria, &worker_summary, &worker_artifacts, check_output)
            },
        )?;

        self.record(
            events,
            EventPayload::GuardChecked {
                from: from.to_string(),
                to: target.clone(),
                check: report.check,
                criteria: report.criteria,
                check_output: report.check_output.clone(),
                judge_rationale: report.judge_rationale.clone(),
                usage: report.usage,
            },
        )?;

        if !report.passed() {
            return self.handle_on_fail(rs, &edge, from, events);
        }

        self.commit(rs, from, &target, Some(&edge), events)
    }

    /// Enter (or re-enter) a state: the terminal fast-path, the budget
    /// guardrail, then a full worker spawn through to a routed proposal.
    fn enter_state(
        &mut self,
        rs: &RunState,
        state: StateId,
        crashed: bool,
        events: &mut Vec<Event>,
    ) -> Result<Option<Outcome>> {
        if self.machine.is_terminal(&state) {
            // Not every terminal is a success. Landing on the escalation state
            // is how an exhausted loop, a capped Navigator, or a stuck worker
            // gives up — reporting that as `done` would tell a human (or a CI
            // wrapper reading the exit status) that the ticket went through.
            let status = if self.machine.escalation_state.as_deref() == Some(state.as_str()) {
                RunStatus::Failed
            } else {
                RunStatus::Done
            };
            return self.finish_now(rs, status, Some(state), events);
        }

        if let Some(outcome) = self.check_budgets(rs, events)? {
            return Ok(Some(outcome));
        }

        let (cycle, attempts_so_far) = self.position_of(rs, &state);
        let attempt = attempts_so_far + 1;

        let entry_addendum = pending_entry_addendum(events, &state);

        let plan =
            self.stage
                .build_stage(&state, cycle, attempt, entry_addendum.as_deref(), crashed)?;

        self.record(
            events,
            EventPayload::StateEntered(crate::core::StateEntered {
                state: state.clone(),
                cycle,
                attempt,
                session_id: plan.spec.session_id.clone(),
                // The bare model id: `thinking` is its own field, and
                // `pi_model_arg()` would render `claude-sonnet-5:high` here and
                // then `high` again beside it.
                model: plan.spec.model.model.clone(),
                thinking: plan.spec.model.thinking.to_string(),
                skills: plan.skills.clone(),
                mcp: plan.mcp.clone(),
            }),
        )?;

        let result = self.runner.run_worker(&plan.spec)?;

        // A worker whose process died is a *crashed* stage, not a *stuck* one,
        // and the two must not be conflated: a stuck worker asked to be routed
        // and the Navigator should answer, while a crash is infrastructure
        // failing under a worker that never got to decide anything. Escalating
        // on a crash abandons a run that a re-entry would have finished
        // (skills/loop-authoring/references/runtime.md "Resuming an interrupted run", docs/design-notes.md).
        //
        // No `worker_output` is written, so the ledger tail stays
        // "state_entered with nothing after it" — exactly the shape the fold
        // already reads as a crash, which is what makes an out-of-process
        // `loop resume` behave identically to this in-process retry.
        if !result.exit_ok {
            // Whatever the spawn said on its way out travels with it. Without
            // that the ledger records that a stage died and nothing about why,
            // and debugging means reproducing the spawn by hand.
            let detail = with_stderr_tail(
                format!("worker process failed (attempt {attempt} of {MAX_CRASH_ATTEMPTS})"),
                &result.stderr_tail,
            );
            self.record(
                events,
                EventPayload::Error {
                    state: Some(state.clone()),
                    kind: ErrorKind::Transient,
                    detail,
                },
            )?;
            if attempt >= MAX_CRASH_ATTEMPTS {
                self.record(
                    events,
                    EventPayload::Note {
                        text: format!(
                            "state `{state}` crashed {attempt} times; escalating rather than \
                             retrying forever"
                        ),
                    },
                )?;
                return self.escalate(rs, &state, events);
            }
            return Ok(None);
        }

        // Capture artifacts *before* `worker_output` — if a crash lands between
        // them, re-entry redoes the stage and nothing already-durable is lost
        // (skills/loop-authoring/references/runtime.md).
        //
        // A claim is worker-authored, so an unusable one — a path that was
        // never written, a typo, a file outside the project root — is an
        // ordinary event, not a harness fault. Record it, drop that one claim,
        // and let the stage's other artifacts through: the missing evidence
        // shows up where it actually matters, at the guard that wanted it,
        // and routes through the edge's `on_fail` like any other shortfall.
        // Propagating the error instead killed the run process outright and
        // left the ledger tail at `state_entered`.
        let mut artifacts = Vec::new();
        if let Some(proposal) = &result.proposal {
            for claim in &proposal.artifacts {
                match self.artifacts.capture(&state, cycle, claim) {
                    Ok(art) => artifacts.push(art),
                    Err(e) => {
                        self.record(
                            events,
                            EventPayload::Error {
                                state: Some(state.clone()),
                                kind: ErrorKind::Transient,
                                detail: format!(
                                    "dropped artifact claim `{}` -> `{}`: {e}",
                                    claim.name, claim.path
                                ),
                            },
                        )?;
                    }
                }
            }
        }

        self.record(
            events,
            EventPayload::WorkerOutput {
                state: state.clone(),
                cycle,
                summary: result.summary.clone(),
                artifacts,
                usage: result.usage,
            },
        )?;

        let proposal = result.proposal.unwrap_or(Proposal {
            to: None,
            blocked: true,
            rationale: crate::core::ABSENT_HANDOFF_RATIONALE.into(),
            artifacts: Vec::new(),
        });

        self.record(
            events,
            EventPayload::TransitionProposed {
                from: state.clone(),
                to: proposal.to.clone(),
                blocked: proposal.blocked,
                rationale: proposal.rationale.clone(),
            },
        )?;

        // Re-fold, so the events just appended are visible to the guard tiers
        // about to run. No re-read: `record` has been keeping `events` current.
        let fresh = self.machine.fold(events);
        self.route_proposal(&fresh, &state, proposal, events)
    }
}

enum NavOutcome {
    /// The addendum, if any, is already durable in the `navigator_invoked`
    /// event; the target's next `enter_state` recovers it from the ledger
    /// via [`pending_entry_addendum`].
    Target(StateId),
    Done(Option<Outcome>),
}

/// The most recent `worker_output` for `state` — what the Judge is allowed to
/// see (never the worker's own claim of success, docs/design-notes.md).
fn last_worker_output_for(events: &[Event], state: &str) -> (String, Vec<Artifact>) {
    crate::core::last(events, |e| match &e.payload {
        EventPayload::WorkerOutput {
            state: s,
            summary,
            artifacts,
            ..
        } if s == state => Some((summary.clone(), artifacts.clone())),
        _ => None,
    })
    .unwrap_or_default()
}

/// The Navigator's entry-prompt addendum for the state it just routed into,
/// stashed in its `navigator_invoked` event.
///
/// Scans back from the commit through *that commit's routing episode* — the
/// run of events since the previous commit or state entry — rather than
/// looking only at the immediately preceding line. The Navigator's event is
/// not always adjacent: a routed target with a guard on the edge puts a
/// `guard_checked` between the two, which is the ordinary case, and reading
/// only `commit_idx - 1` meant the note survived exactly when it was useless
/// (a route to the terminal escalation state, which never renders a stage prompt).
///
/// The episode bound is what keeps this from reaching back into an earlier
/// cycle and re-delivering a stale note the run has long since moved past.
fn pending_entry_addendum(events: &[Event], state: &str) -> Option<String> {
    let commit_idx = events.iter().rposition(
        |e| matches!(&e.payload, EventPayload::TransitionCommitted { to, .. } if to == state),
    )?;
    for e in events[..commit_idx].iter().rev() {
        match &e.payload {
            EventPayload::NavigatorInvoked {
                chosen_to,
                entry_prompt,
                ..
            } if chosen_to == state => return entry_prompt.clone(),
            // The start of this episode: anything older belongs to a previous
            // decision and its addendum was either already delivered or was
            // never meant for this entry.
            EventPayload::TransitionCommitted { .. } | EventPayload::StateEntered(_) => break,
            _ => {}
        }
    }
    None
}
