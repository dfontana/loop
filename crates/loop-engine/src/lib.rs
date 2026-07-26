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
    AgentRunner, ArtifactRef, ArtifactSink, CheckRunner, Config, ErrorKind, Event, EventPayload,
    FoldStatus, GuardOutcome, LedgerSink, LoopSpec, Machine, OnExhausted, OnFail, Proposal, Result,
    ResumePoint, RunState, RunStatus, StateId, Totals, Transition, fold_with_loop_heads,
};

pub mod guards;
pub mod prompts;
pub mod validate;

use guards::check as guard_check;
use guards::select_edge;

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
    pub config: &'a Config,
    pub runner: &'a dyn AgentRunner,
    /// Runs a transition's deterministic check — the harness acting for
    /// itself, with no agent in the path.
    pub checks: &'a dyn CheckRunner,
    pub ledger: &'a mut dyn LedgerSink,
    pub artifacts: &'a dyn ArtifactSink,
    /// Renders prompts and assembles spawn specs. Implemented in the CLI over
    /// `loop-toolbox`, so the engine stays free of filesystem concerns.
    pub stage: &'a dyn StageBuilder,
    /// Wall-clock start, for the budget check. `None` starts it at `run()`.
    pub started_at: Option<std::time::Instant>,
}

/// How many times a stage may die mid-flight before the run escalates rather
/// than retrying. A crash is worth retrying — a flaky spawn, an OOM, a dropped
/// connection — but a stage that dies every time is a real fault, and spinning
/// on it burns budget silently, which is the failure docs/07 #3 exists to stop.
const MAX_CRASH_ATTEMPTS: u32 = 3;

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
    /// 1. Fold the ledger. A fresh run appends `run_started`; a resume picks up at the folded
    ///    [`loop_core::ResumePoint`].
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
        let events = self.ledger.read_all()?;
        let has_run_started = events
            .iter()
            .any(|e| matches!(e.payload, EventPayload::RunStarted { .. }));
        let rs = self.fold(&events);

        if let FoldStatus::Finished(status) = rs.fold_status() {
            return Ok(Some(Outcome {
                status,
                terminal_state: rs.current.clone(),
                totals: rs.totals,
            }));
        }

        if !has_run_started {
            self.append_run_started()?;
        }

        match rs.resume.clone() {
            ResumePoint::Done => unreachable!("Finished handled above"),
            ResumePoint::Fresh => {
                let entry = self.machine.entry.clone();
                self.enter_state(&rs, entry, false)
            }
            ResumePoint::EnterState { state, crashed } => self.enter_state(&rs, state, crashed),
            ResumePoint::GuardCheck { from, proposal } => {
                if let Some(outcome) = self.check_budgets(&rs)? {
                    return Ok(Some(outcome));
                }
                self.route_proposal(&rs, &from, proposal)
            }
        }
    }

    // ── internal ─────────────────────────────────────────────────────────

    fn fold(&self, events: &[Event]) -> RunState {
        let is_loop_head = |s: &str| self.machine.loop_with_head(s).is_some();
        fold_with_loop_heads(events, &is_loop_head)
    }

    /// The `(cycle, attempt)` a state is currently on — the same numbers
    /// [`Self::enter_state`] stamped on `state_entered`, so a check runs with
    /// the identity of the stage that just finished.
    fn position_of(&self, rs: &RunState, state: &str) -> (u32, u32) {
        let cycle = if self.machine.loop_with_head(state).is_some() {
            rs.cycle_of(state).max(1)
        } else {
            1
        };
        (cycle, rs.attempts_of(state, cycle).max(1))
    }

    fn elapsed_s(&self) -> u64 {
        self.started_at.map(|t| t.elapsed().as_secs()).unwrap_or(0)
    }

    fn totals_now(&self, rs: &RunState) -> Totals {
        Totals {
            cost_usd: rs.totals.cost_usd,
            wallclock_s: self.elapsed_s(),
            transitions: rs.totals.transitions,
        }
    }

    fn append_run_started(&mut self) -> Result<()> {
        self.ledger.append(EventPayload::RunStarted {
            ticket: self.machine.ticket.clone(),
            machine_hash: self.machine.source_hash.clone(),
            budgets: self.machine.budgets,
        })?;
        Ok(())
    }

    /// The global guardrails — `$`, wallclock, total transitions — checked
    /// *before* any spawn. Exceeding one aborts the run, naming the guardrail
    /// via an `error` line right before `run_finished`.
    fn check_budgets(&mut self, rs: &RunState) -> Result<Option<Outcome>> {
        let b = &self.machine.budgets;
        let mut reason = None;
        if reason.is_none() {
            if let Some(usd) = b.usd {
                if rs.totals.cost_usd > usd {
                    reason = Some(format!(
                        "budget_usd exceeded: ${:.2} > ${:.2}",
                        rs.totals.cost_usd, usd
                    ));
                }
            }
        }
        if reason.is_none() {
            if let Some(wc) = b.wallclock_s {
                let elapsed = self.elapsed_s();
                if elapsed > wc {
                    reason = Some(format!("wallclock_s exceeded: {elapsed}s > {wc}s"));
                }
            }
        }
        if reason.is_none() {
            if let Some(mt) = b.max_transitions {
                if rs.totals.transitions >= mt {
                    reason = Some(format!(
                        "max_transitions exceeded: {} >= {mt}",
                        rs.totals.transitions
                    ));
                }
            }
        }
        let Some(reason) = reason else {
            return Ok(None);
        };
        self.ledger.append(EventPayload::Error {
            state: rs.current.clone(),
            kind: ErrorKind::Fatal,
            detail: reason,
        })?;
        let totals = self.totals_now(rs);
        self.ledger.append(EventPayload::RunFinished {
            status: RunStatus::Aborted,
            terminal_state: rs.current.clone(),
            totals,
        })?;
        Ok(Some(Outcome {
            status: RunStatus::Aborted,
            terminal_state: rs.current.clone(),
            totals,
        }))
    }

    /// Append `run_finished` directly — used by budget-exhausted, `on_fail:
    /// abort`, and `on_exhausted: abort` paths, none of which route through a
    /// declared terminal state.
    fn finish_now(
        &mut self,
        rs: &RunState,
        status: RunStatus,
        terminal_state: Option<StateId>,
    ) -> Result<Option<Outcome>> {
        let totals = self.totals_now(rs);
        self.ledger.append(EventPayload::RunFinished {
            status,
            terminal_state: terminal_state.clone(),
            totals,
        })?;
        Ok(Some(Outcome {
            status,
            terminal_state,
            totals,
        }))
    }

    /// Route to the machine's escalation state — a harness override, not a
    /// worker choice, so it bypasses guard checks. Falls back to an aborted
    /// run if no `escalation_state` is configured.
    fn escalate(&mut self, rs: &RunState, from: &str) -> Result<Option<Outcome>> {
        match self.machine.escalation_state.clone() {
            Some(esc) => self.commit(rs, from, &esc, None),
            None => self.finish_now(rs, RunStatus::Aborted, Some(from.to_string())),
        }
    }

    /// A loop's head has been re-entered past `max_cycles`.
    fn handle_exhausted(
        &mut self,
        rs: &RunState,
        from: &str,
        l: &LoopSpec,
    ) -> Result<Option<Outcome>> {
        self.ledger.append(EventPayload::Error {
            state: Some(from.to_string()),
            kind: ErrorKind::Fatal,
            detail: format!(
                "loop `{}` exhausted max_cycles={} at head `{}`",
                l.name,
                l.max_cycles,
                l.head().map(|s| s.as_str()).unwrap_or("?")
            ),
        })?;
        match l.on_exhausted {
            OnExhausted::Escalate => self.escalate(rs, from),
            OnExhausted::Abort => self.finish_now(rs, RunStatus::Aborted, Some(from.to_string())),
        }
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
    ) -> Result<Option<Outcome>> {
        if let Some(l) = self
            .machine
            .loops
            .iter()
            .find(|l| l.head().map(|h| h.as_str()) == Some(to))
        {
            let prospective = rs.cycle_of(to) + 1;
            if prospective > l.max_cycles {
                return self.handle_exhausted(rs, from, l);
            }
        }

        let cycle_for_event = rs.cycle_of(from).max(1);
        self.ledger.append(EventPayload::TransitionCommitted {
            from: from.to_string(),
            to: to.to_string(),
            cycle: cycle_for_event,
        })?;

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
    ) -> Result<Option<Outcome>> {
        match &edge.on_fail {
            OnFail::Retry => {
                let events = self.ledger.read_all()?;
                let fresh = self.fold(&events);
                self.enter_state(&fresh, from.to_string(), false)
            }
            OnFail::Abort => self.finish_now(rs, RunStatus::Failed, Some(from.to_string())),
            OnFail::Route(target) => {
                let target = target.clone();
                self.commit(rs, from, &target, None)
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
    ) -> Result<NavOutcome> {
        let cap = self.machine.navigator_max_invocations;
        if rs.navigator_invocations >= cap || rs.navigator_from(from) >= cap {
            return Ok(NavOutcome::Done(self.escalate(rs, from)?));
        }
        let spec = self.stage.build_navigator(&from.to_string(), proposal)?;
        let choice = self.runner.run_navigator(&spec)?;
        self.ledger.append(EventPayload::NavigatorInvoked {
            from: from.to_string(),
            proposal: proposal.map(|p| p.rationale.clone()).unwrap_or_default(),
            chosen_to: choice.to.clone(),
            entry_prompt: choice.entry_prompt.clone(),
            usage: choice.usage,
        })?;
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
    ) -> Result<Option<Outcome>> {
        let needs_navigator = proposal.blocked
            || match &proposal.to {
                None => true,
                Some(t) => !self.machine.neighbors(from).contains(t),
            };

        let target = if needs_navigator {
            match self.invoke_navigator(rs, from, Some(&proposal))? {
                NavOutcome::Done(outcome) => return Ok(outcome),
                NavOutcome::Target(to) => to,
            }
        } else {
            proposal.to.clone().expect("checked by needs_navigator")
        };

        // Escalation is a harness-declared override, not a graph edge —
        // commit straight there, no guard tiers.
        if Some(&target) == self.machine.escalation_state.as_ref() {
            return self.commit(rs, from, &target, None);
        }

        let edge = match select_edge(self.machine, from, &target) {
            Some(e) => e.clone(),
            None => {
                // Structurally unreachable even after navigator routing —
                // shouldn't happen by construction, but stay total.
                self.ledger.append(EventPayload::GuardChecked {
                    from: from.to_string(),
                    to: target.clone(),
                    structural: GuardOutcome::Fail,
                    check: GuardOutcome::Skip,
                    criteria: GuardOutcome::Skip,
                    check_output: None,
                    judge_rationale: None,
                })?;
                return self.escalate(rs, from);
            }
        };

        let events = self.ledger.read_all()?;
        let (worker_summary, worker_artifacts) = last_worker_output_for(&events, from);
        let (cycle, attempt) = self.position_of(rs, from);

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

        self.ledger.append(EventPayload::GuardChecked {
            from: from.to_string(),
            to: target.clone(),
            structural: report.structural,
            check: report.check,
            criteria: report.criteria,
            check_output: report.check_output.clone(),
            judge_rationale: report.judge_rationale.clone(),
        })?;

        if !report.passed() {
            return self.handle_on_fail(rs, &edge, from);
        }

        self.commit(rs, from, &target, Some(&edge))
    }

    /// Enter (or re-enter) a state: the terminal fast-path, the budget
    /// guardrail, then a full worker spawn through to a routed proposal.
    fn enter_state(
        &mut self,
        rs: &RunState,
        state: StateId,
        _crashed: bool,
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
            return self.finish_now(rs, status, Some(state));
        }

        if let Some(outcome) = self.check_budgets(rs)? {
            return Ok(Some(outcome));
        }

        let is_loop_head = self.machine.loop_with_head(&state).is_some();
        let cycle = if is_loop_head {
            rs.cycle_of(&state).max(1)
        } else {
            1
        };
        let attempt = rs.attempts_of(&state, cycle) + 1;

        let events = self.ledger.read_all()?;
        let entry_addendum = pending_entry_addendum(&events, &state);

        let plan = self
            .stage
            .build_stage(&state, cycle, attempt, entry_addendum.as_deref())?;

        self.ledger.append(EventPayload::StateEntered {
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
        })?;

        let result = self.runner.run_worker(&plan.spec)?;

        // A worker whose process died is a *crashed* stage, not a *stuck* one,
        // and the two must not be conflated: a stuck worker asked to be routed
        // and the Navigator should answer, while a crash is infrastructure
        // failing under a worker that never got to decide anything. Escalating
        // on a crash abandons a run that a re-entry would have finished
        // (docs/03 "Idempotency & re-entry", docs/07 #8).
        //
        // No `worker_output` is written, so the ledger tail stays
        // "state_entered with nothing after it" — exactly the shape the fold
        // already reads as a crash, which is what makes an out-of-process
        // `loop resume` behave identically to this in-process retry.
        if !result.exit_ok {
            let attempts_so_far = rs.attempts_of(&state, cycle) + 1;
            self.ledger.append(EventPayload::Error {
                state: Some(state.clone()),
                kind: ErrorKind::Transient,
                detail: format!(
                    "worker process failed (attempt {attempts_so_far} of {MAX_CRASH_ATTEMPTS})"
                ),
            })?;
            if attempts_so_far >= MAX_CRASH_ATTEMPTS {
                self.ledger.append(EventPayload::Note {
                    text: format!(
                        "state `{state}` crashed {attempts_so_far} times; escalating rather than \
                         retrying forever"
                    ),
                })?;
                return self.escalate(rs, &state);
            }
            return Ok(None);
        }

        // Capture artifacts *before* `worker_output` — if a crash lands between
        // them, re-entry redoes the stage and nothing already-durable is lost
        // (docs/03).
        let mut artifacts = Vec::new();
        if let Some(proposal) = &result.proposal {
            for claim in &proposal.artifacts {
                artifacts.push(self.artifacts.capture(&state, cycle, claim)?);
            }
        }

        self.ledger.append(EventPayload::WorkerOutput {
            state: state.clone(),
            cycle,
            summary: result.summary.clone(),
            artifacts,
            usage: result.usage,
        })?;

        let proposal = result.proposal.unwrap_or(Proposal {
            to: None,
            blocked: true,
            rationale: "worker ended its turn without calling transition".into(),
            artifacts: Vec::new(),
        });

        self.ledger.append(EventPayload::TransitionProposed {
            from: state.clone(),
            to: proposal.to.clone(),
            blocked: proposal.blocked,
            rationale: proposal.rationale.clone(),
            by: loop_core::Actor::Worker,
        })?;

        // Re-fold: the events just appended (navigator/cycle context) must be
        // visible to the guard tiers about to run.
        let events = self.ledger.read_all()?;
        let fresh = self.fold(&events);
        self.route_proposal(&fresh, &state, proposal)
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
/// see (never the worker's own claim of success, docs/07 #1).
fn last_worker_output_for(events: &[Event], state: &str) -> (String, Vec<ArtifactRef>) {
    events
        .iter()
        .rev()
        .find_map(|e| match &e.payload {
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
/// stashed in its `navigator_invoked` event — the one immediately preceding
/// the `transition_committed` that landed us here.
fn pending_entry_addendum(events: &[Event], state: &str) -> Option<String> {
    let commit_idx = events.iter().rposition(
        |e| matches!(&e.payload, EventPayload::TransitionCommitted { to, .. } if to == state),
    )?;
    if commit_idx == 0 {
        return None;
    }
    match &events[commit_idx - 1].payload {
        EventPayload::NavigatorInvoked {
            chosen_to,
            entry_prompt,
            ..
        } if chosen_to == state => entry_prompt.clone(),
        _ => None,
    }
}
