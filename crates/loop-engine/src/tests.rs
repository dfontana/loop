//! Engine integration tests, against the fakes in [`crate::test_support`].
//! No Lua, no subprocess, no filesystem, no API key.

use loop_core::{Config, OnExhausted, OnFail, Paths, PlaybookRef, RunStatus};

use crate::guards::select_edge;
use crate::test_support::*;
use crate::{Engine, Outcome};

fn config() -> Config {
    Config::defaults(Paths {
        config_dir: "cfg".into(),
        state_dir: "state".into(),
        project_dir: "proj".into(),
    })
}

fn run(machine: &loop_core::Machine, runner: &FakeRunner, ledger: &mut FakeLedger) -> Outcome {
    run_with_checks(machine, runner, &FakeChecks::default(), ledger)
}

fn run_with_checks(
    machine: &loop_core::Machine,
    runner: &FakeRunner,
    checks: &FakeChecks,
    ledger: &mut FakeLedger,
) -> Outcome {
    let cfg = config();
    let artifacts = FakeArtifacts;
    let stage = FakeStageBuilder { machine };
    let mut engine = Engine {
        machine,
        config: &cfg,
        runner,
        checks,
        ledger,
        artifacts: &artifacts,
        stage: &stage,
        started_at: None,
    };
    engine.run().expect("engine run should not error")
}

// ── happy path ────────────────────────────────────────────────────────────

#[test]
fn happy_path_entry_to_terminal_with_right_ledger_sequence() {
    let mut m = base_machine();
    m.entry = "implement".into();
    m.terminals.insert("done".into());
    m.states.insert("implement".into(), state("implement"));
    m.states.insert("review".into(), state("review"));
    m.transitions.push(judged_edge(
        "implement",
        "review",
        "plan addressed, build green",
    ));
    m.transitions
        .push(judged_edge("review", "done", "no blocking defects remain"));

    let runner = FakeRunner::default();
    runner.script_worker(
        "implement",
        worker_result(proposal_to("review", "plan done, build green")),
    );
    runner.script_worker("review", worker_result(proposal_to("done", "clean review")));
    runner.script_judge(verdict(true, "checklist covered, no TODOs"));
    runner.script_judge(verdict(true, "no defects worth blocking on"));

    let mut ledger = FakeLedger::default();
    let outcome = run(&m, &runner, &mut ledger);

    assert_eq!(outcome.status, RunStatus::Done);
    assert_eq!(outcome.terminal_state.as_deref(), Some("done"));

    let kinds = ledger.kinds();
    assert_eq!(
        kinds,
        vec![
            "run_started",
            "state_entered",
            "worker_output",
            "transition_proposed",
            "guard_checked",
            "transition_committed",
            "state_entered",
            "worker_output",
            "transition_proposed",
            "guard_checked",
            "transition_committed",
            "run_finished",
        ]
    );

    for i in 0..2 {
        if let loop_core::EventPayload::GuardChecked {
            structural,
            criteria,
            ..
        } = ledger.payloads_of("guard_checked")[i]
        {
            assert_eq!(*structural, loop_core::GuardOutcome::Pass);
            assert_eq!(*criteria, loop_core::GuardOutcome::Pass);
        } else {
            panic!("expected GuardChecked");
        }
    }
}

// ── guard tiers & on_fail ────────────────────────────────────────────────

#[test]
fn judge_fail_retries_source_state() {
    let mut m = base_machine();
    m.entry = "check".into();
    m.terminals.insert("done".into());
    m.states.insert("check".into(), state("check"));
    m.transitions
        .push(judged_edge("check", "done", "the suite is green"));

    let runner = FakeRunner::default();
    runner.script_worker(
        "check",
        worker_result(proposal_to("done", "think it's fine")),
    );
    runner.script_worker(
        "check",
        worker_result(proposal_to("done", "actually fine now")),
    );
    runner.script_judge(verdict(false, "two specs still red"));
    runner.script_judge(verdict(true, "suite is green"));

    let mut ledger = FakeLedger::default();
    let outcome = run(&m, &runner, &mut ledger);

    assert_eq!(outcome.status, RunStatus::Done);
    let entered = ledger.payloads_of("state_entered");
    assert_eq!(entered.len(), 2);
    let attempts: Vec<u32> = entered
        .iter()
        .map(|p| match p {
            loop_core::EventPayload::StateEntered { attempt, .. } => *attempt,
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(attempts, vec![1, 2]);

    let checks = ledger.payloads_of("guard_checked");
    assert_eq!(checks.len(), 2);
    match checks[0] {
        loop_core::EventPayload::GuardChecked { criteria, .. } => {
            assert_eq!(*criteria, loop_core::GuardOutcome::Fail)
        }
        _ => unreachable!(),
    }
    match checks[1] {
        loop_core::EventPayload::GuardChecked { criteria, .. } => {
            assert_eq!(*criteria, loop_core::GuardOutcome::Pass)
        }
        _ => unreachable!(),
    }
}

#[test]
fn judge_fail_with_on_fail_route_sends_elsewhere_without_further_guard_checks() {
    let mut m = base_machine();
    m.entry = "implement".into();
    m.terminals.insert("done".into());
    m.terminals.insert("needs_human".into());
    m.states.insert("implement".into(), state("implement"));
    m.states.insert("review".into(), state("review"));
    m.transitions.push(loop_core::Transition {
        on_fail: OnFail::Route("needs_human".into()),
        ..judged_edge("implement", "review", "clean and green")
    });

    let runner = FakeRunner::default();
    runner.script_worker(
        "implement",
        worker_result(proposal_to("review", "done, I think")),
    );
    runner.script_judge(verdict(false, "TODOs remain"));

    let mut ledger = FakeLedger::default();
    let outcome = run(&m, &runner, &mut ledger);

    assert_eq!(outcome.status, RunStatus::Done);
    assert_eq!(outcome.terminal_state.as_deref(), Some("needs_human"));

    let checks = ledger.payloads_of("guard_checked");
    assert_eq!(checks.len(), 1); // no second guard check for the routed commit
    let commits = ledger.payloads_of("transition_committed");
    assert_eq!(commits.len(), 1);
    match commits[0] {
        loop_core::EventPayload::TransitionCommitted { to, .. } => {
            assert_eq!(to, "needs_human")
        }
        _ => unreachable!(),
    }
}

#[test]
fn on_fail_abort_finishes_run_failed() {
    let mut m = base_machine();
    m.entry = "implement".into();
    m.terminals.insert("review".into()); // unreachable terminal, irrelevant here
    m.states.insert("implement".into(), state("implement"));
    m.transitions.push(loop_core::Transition {
        on_fail: OnFail::Abort,
        ..judged_edge("implement", "review", "clean and green")
    });

    let runner = FakeRunner::default();
    runner.script_worker(
        "implement",
        worker_result(proposal_to("review", "done, I think")),
    );
    runner.script_judge(verdict(false, "not clean"));

    let mut ledger = FakeLedger::default();
    let outcome = run(&m, &runner, &mut ledger);

    assert_eq!(outcome.status, RunStatus::Failed);
    assert_eq!(outcome.terminal_state.as_deref(), Some("implement"));
}

// ── the check tier ───────────────────────────────────────────────────────

/// The whole point of the tier: the check runs in the harness's own
/// subprocess, and its exit code decides. Nothing the worker said is consulted.
#[test]
fn a_failing_check_blocks_the_edge_and_never_reaches_the_judge() {
    let mut m = base_machine();
    m.entry = "implement".into();
    m.terminals.insert("done".into());
    m.states.insert("implement".into(), state("implement"));
    m.transitions.push(loop_core::Transition {
        criteria: Some("the plan is done".into()),
        on_fail: OnFail::Abort,
        ..checked_edge("implement", "done", "cargo test")
    });

    let runner = FakeRunner::default();
    runner.script_worker(
        "implement",
        worker_result(proposal_to("done", "all green, promise")),
    );
    let checks = FakeChecks::default();
    checks.script_fail("2 tests failed");

    let mut ledger = FakeLedger::default();
    let outcome = run_with_checks(&m, &runner, &checks, &mut ledger);

    assert_eq!(outcome.status, RunStatus::Failed);
    assert_eq!(
        runner.judge_calls.borrow().len(),
        0,
        "a failed check must not be appealable to the Judge"
    );

    match ledger.payloads_of("guard_checked")[0] {
        loop_core::EventPayload::GuardChecked {
            check,
            criteria,
            check_output,
            ..
        } => {
            assert_eq!(*check, loop_core::GuardOutcome::Fail);
            assert_eq!(*criteria, loop_core::GuardOutcome::Skip);
            assert_eq!(check_output.as_deref(), Some("2 tests failed"));
        }
        _ => unreachable!(),
    }
}

/// A passing check does not by itself pass the edge — it is a precondition,
/// and the semantic criterion still gets its say.
#[test]
fn a_passing_check_still_defers_to_the_judge_and_hands_it_the_output() {
    let mut m = base_machine();
    m.entry = "implement".into();
    m.terminals.insert("done".into());
    m.states.insert("implement".into(), state("implement"));
    m.transitions.push(loop_core::Transition {
        criteria: Some("every plan item is addressed".into()),
        on_fail: OnFail::Abort,
        ..checked_edge("implement", "done", "cargo test")
    });

    let runner = FakeRunner::default();
    runner.script_worker("implement", worker_result(proposal_to("done", "done")));
    runner.script_judge(verdict(false, "item 3 was never touched"));
    let checks = FakeChecks::default();
    checks.script_pass("test result: ok. 41 passed");

    let mut ledger = FakeLedger::default();
    let outcome = run_with_checks(&m, &runner, &checks, &mut ledger);

    assert_eq!(outcome.status, RunStatus::Failed);
    assert_eq!(
        runner.judge_calls.borrow()[0].check_output.as_deref(),
        Some("test result: ok. 41 passed"),
        "the Judge must see the one piece of evidence the worker didn't author"
    );
    match ledger.payloads_of("guard_checked")[0] {
        loop_core::EventPayload::GuardChecked {
            check, criteria, ..
        } => {
            assert_eq!(*check, loop_core::GuardOutcome::Pass);
            assert_eq!(*criteria, loop_core::GuardOutcome::Fail);
        }
        _ => unreachable!(),
    }
}

/// An edge with no `:check` skips the tier entirely — no subprocess, no
/// spurious `check_output` on the ledger line.
#[test]
fn an_edge_without_a_check_skips_the_tier() {
    let mut m = base_machine();
    m.entry = "implement".into();
    m.terminals.insert("done".into());
    m.states.insert("implement".into(), state("implement"));
    m.transitions
        .push(judged_edge("implement", "done", "the plan is done"));

    let runner = FakeRunner::default();
    runner.script_worker("implement", worker_result(proposal_to("done", "done")));
    runner.script_judge(verdict(true, "checklist covered"));
    let checks = FakeChecks::default();

    let mut ledger = FakeLedger::default();
    let outcome = run_with_checks(&m, &runner, &checks, &mut ledger);

    assert_eq!(outcome.status, RunStatus::Done);
    assert!(checks.commands().is_empty());
    match ledger.payloads_of("guard_checked")[0] {
        loop_core::EventPayload::GuardChecked {
            check,
            check_output,
            ..
        } => {
            assert_eq!(*check, loop_core::GuardOutcome::Skip);
            assert!(check_output.is_none());
        }
        _ => unreachable!(),
    }
}

/// The check runs with the identity of the stage that just finished, so a
/// cycle-scoped command inspects the namespace that stage actually deployed to.
#[test]
fn a_check_runs_with_the_finishing_stages_cycle_and_attempt() {
    let mut m = base_machine();
    m.entry = "start".into();
    m.terminals.insert("done".into());
    m.states.insert("start".into(), state("start"));
    m.states.insert("qa".into(), state("qa"));
    m.transitions.push(edge("start", "qa"));
    m.transitions.push(loop_core::Transition {
        backoff_s: Some(0),
        ..checked_edge("qa", "qa", "contract-check --ns loop-$CYCLE")
    });
    m.transitions
        .push(checked_edge("qa", "done", "contract-check --final"));
    m.loops
        .push(loop_spec("qa", &["qa"], 4, OnExhausted::Escalate));

    let runner = FakeRunner::default();
    runner.script_worker("start", worker_result(proposal_to("qa", "go")));
    runner.script_worker("qa", worker_result(proposal_to("qa", "flaky, retry")));
    runner.script_worker("qa", worker_result(proposal_to("done", "green")));
    let checks = FakeChecks::default();

    let mut ledger = FakeLedger::default();
    let outcome = run_with_checks(&m, &runner, &checks, &mut ledger);
    assert_eq!(outcome.status, RunStatus::Done);

    let ran = checks.ran.borrow();
    let positions: Vec<(String, u32)> = ran
        .iter()
        .map(|(_, from, cycle, _)| (from.clone(), *cycle))
        .collect();
    assert_eq!(
        positions,
        vec![("qa".to_string(), 1), ("qa".to_string(), 2)],
        "the self-loop's second check runs on cycle 2, not cycle 1"
    );
}

// ── select_edge ─────────────────────────────────────────────────────────

/// Parallel edges used to be told apart by their `when` guards. Without those,
/// the first declared edge wins and the rest are dead — which is why
/// [`crate::validate`] rejects the duplicate rather than letting it silently
/// decide which `criteria` applies.
#[test]
fn select_edge_takes_the_first_declared_edge_and_none_when_absent() {
    let mut m = base_machine();
    m.states.insert("a".into(), state("a"));
    m.states.insert("b".into(), state("b"));
    m.transitions.push(judged_edge("a", "b", "first"));
    m.transitions.push(judged_edge("a", "b", "second"));

    let found = select_edge(&m, "a", "b").expect("an edge exists");
    assert_eq!(found.criteria.as_deref(), Some("first"));

    assert!(select_edge(&m, "b", "a").is_none());
}

// ── docs/06: transient vs real routing, self-loop with backoff ─────────

#[test]
fn transient_vs_real_routing_drives_self_loop_then_real_failure_to_debug() {
    let mut m = base_machine();
    m.entry = "start".into();
    m.terminals.insert("validate".into());
    m.states.insert("start".into(), state("start"));
    m.states.insert("qa_staging".into(), state("qa_staging"));
    m.states.insert("debug".into(), state("debug"));

    m.transitions.push(edge("start", "qa_staging"));
    m.transitions
        .push(judged_edge("qa_staging", "validate", "the run passed"));
    m.transitions.push(loop_core::Transition {
        backoff_s: Some(0),
        ..judged_edge(
            "qa_staging",
            "qa_staging",
            "the failure was infrastructural",
        )
    });
    m.transitions.push(judged_edge(
        "qa_staging",
        "debug",
        "the failure was a real defect",
    ));
    m.transitions.push(edge("debug", "qa_staging"));
    m.loops
        .push(loop_spec("qa", &["qa_staging"], 5, OnExhausted::Escalate));

    let runner = FakeRunner::default();
    runner.script_worker("start", worker_result(proposal_to("qa_staging", "go")));
    runner.script_worker(
        "qa_staging",
        worker_result(proposal_to("qa_staging", "transient, retry")),
    );
    runner.script_worker(
        "qa_staging",
        worker_result(proposal_to("debug", "real failure")),
    );
    runner.script_worker(
        "debug",
        worker_result(proposal_to("qa_staging", "fixed it")),
    );
    runner.script_worker(
        "qa_staging",
        worker_result(proposal_to("validate", "all green")),
    );
    // One verdict per judged edge, in the order the run takes them.
    runner.script_judge(verdict(true, "executor was lost mid-stage"));
    runner.script_judge(verdict(true, "schema mismatch in the transform"));
    runner.script_judge(verdict(true, "output sample satisfies the QA cases"));

    let mut ledger = FakeLedger::default();
    let outcome = run(&m, &runner, &mut ledger);

    assert_eq!(outcome.status, RunStatus::Done);
    assert_eq!(outcome.terminal_state.as_deref(), Some("validate"));

    let entered = ledger.payloads_of("state_entered");
    let cycles: Vec<(String, u32)> = entered
        .iter()
        .map(|p| match p {
            loop_core::EventPayload::StateEntered { state, cycle, .. } => (state.clone(), *cycle),
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(
        cycles,
        vec![
            ("start".into(), 1),
            ("qa_staging".into(), 1),
            ("qa_staging".into(), 2),
            ("debug".into(), 1),
            ("qa_staging".into(), 3),
        ]
    );
}

// ── bounds ───────────────────────────────────────────────────────────────

#[test]
fn max_cycles_exhaustion_escalates() {
    let mut m = base_machine();
    m.entry = "start".into();
    m.terminals.insert("blocked".into());
    m.escalation_state = Some("blocked".into());
    m.states.insert("start".into(), state("start"));
    m.states.insert("qa_staging".into(), state("qa_staging"));
    m.transitions.push(edge("start", "qa_staging"));
    m.transitions.push(loop_core::Transition {
        backoff_s: Some(0),
        ..edge("qa_staging", "qa_staging")
    });
    m.loops
        .push(loop_spec("qa", &["qa_staging"], 2, OnExhausted::Escalate));

    let runner = FakeRunner::default();
    runner.script_worker("start", worker_result(proposal_to("qa_staging", "go")));
    for _ in 0..5 {
        runner.script_worker(
            "qa_staging",
            worker_result(proposal_to("qa_staging", "still flaky")),
        );
    }

    let mut ledger = FakeLedger::default();
    let outcome = run(&m, &runner, &mut ledger);

    // Escalation is a failed run, not a successful one: the ticket did not
    // go through, and the exit status must say so.
    assert_eq!(outcome.status, RunStatus::Failed);
    assert_eq!(outcome.terminal_state.as_deref(), Some("blocked"));

    // cycle 1 (from `start`), cycle 2 (self-loop) both run; the 3rd would
    // exceed max_cycles=2, so it's escalated instead of spawning a 3rd worker.
    let entered = ledger.payloads_of("state_entered");
    let qa_entries = entered
        .iter()
        .filter(
            |p| matches!(p, loop_core::EventPayload::StateEntered{state,..} if state=="qa_staging"),
        )
        .count();
    assert_eq!(qa_entries, 2);

    let errors = ledger.payloads_of("error");
    assert_eq!(errors.len(), 1);
}

#[test]
fn max_cycles_exhaustion_aborts_when_configured() {
    let mut m = base_machine();
    m.entry = "start".into();
    m.terminals.insert("done".into());
    m.states.insert("start".into(), state("start"));
    m.states.insert("qa_staging".into(), state("qa_staging"));
    m.transitions.push(edge("start", "qa_staging"));
    m.transitions.push(loop_core::Transition {
        backoff_s: Some(0),
        ..edge("qa_staging", "qa_staging")
    });
    m.loops
        .push(loop_spec("qa", &["qa_staging"], 1, OnExhausted::Abort));

    let runner = FakeRunner::default();
    runner.script_worker("start", worker_result(proposal_to("qa_staging", "go")));
    for _ in 0..3 {
        runner.script_worker(
            "qa_staging",
            worker_result(proposal_to("qa_staging", "still flaky")),
        );
    }

    let mut ledger = FakeLedger::default();
    let outcome = run(&m, &runner, &mut ledger);

    assert_eq!(outcome.status, RunStatus::Aborted);
}

#[test]
fn navigator_cap_exceeded_escalates_without_spawning() {
    let mut m = base_machine();
    m.entry = "debug".into();
    m.terminals.insert("blocked".into());
    m.escalation_state = Some("blocked".into());
    m.navigator_max_invocations = 2;
    m.states.insert("debug".into(), state("debug"));
    // An unconditional self-loop so a navigator choice of `debug` is a valid
    // declared edge and re-enters the same state, exercising the per-state cap.
    m.transitions.push(edge("debug", "debug"));

    let runner = FakeRunner::default();
    for _ in 0..3 {
        runner.script_worker("debug", worker_result(proposal_blocked("stuck")));
    }
    // Only two navigator calls should ever actually happen (the cap).
    runner.script_navigator(choice("debug"));
    runner.script_navigator(choice("debug"));

    let mut ledger = FakeLedger::default();
    let outcome = run(&m, &runner, &mut ledger);

    // Escalation is a failed run, not a successful one: the ticket did not
    // go through, and the exit status must say so.
    assert_eq!(outcome.status, RunStatus::Failed);
    assert_eq!(outcome.terminal_state.as_deref(), Some("blocked"));
    assert_eq!(runner.navigator_calls.borrow().len(), 2);
    assert_eq!(ledger.payloads_of("navigator_invoked").len(), 2);
}

#[test]
fn max_transitions_budget_aborts() {
    let mut m = base_machine();
    m.entry = "ping".into();
    m.terminals.insert("done".into());
    m.budgets.max_transitions = Some(1);
    m.states.insert("ping".into(), state("ping"));
    m.states.insert("pong".into(), state("pong"));
    m.transitions.push(edge("ping", "pong"));
    m.transitions.push(edge("pong", "ping"));

    let runner = FakeRunner::default();
    runner.script_worker("ping", worker_result(proposal_to("pong", "go")));
    runner.script_worker("pong", worker_result(proposal_to("ping", "back")));

    let mut ledger = FakeLedger::default();
    let outcome = run(&m, &runner, &mut ledger);

    assert_eq!(outcome.status, RunStatus::Aborted);
    assert_eq!(outcome.totals.transitions, 1);
}

#[test]
fn budget_usd_aborts() {
    let mut m = base_machine();
    m.entry = "spend".into();
    m.terminals.insert("done".into());
    m.budgets.usd = Some(1.0);
    m.states.insert("spend".into(), state("spend"));
    m.states.insert("more".into(), state("more"));
    m.transitions.push(edge("spend", "more"));
    m.transitions.push(edge("more", "spend"));

    let runner = FakeRunner::default();
    for _ in 0..5 {
        runner.script_worker(
            "spend",
            worker_result_costing(proposal_to("more", "go"), 0.8),
        );
        runner.script_worker(
            "more",
            worker_result_costing(proposal_to("spend", "back"), 0.8),
        );
    }

    let mut ledger = FakeLedger::default();
    let outcome = run(&m, &runner, &mut ledger);

    assert_eq!(outcome.status, RunStatus::Aborted);
    assert!(outcome.totals.cost_usd > 1.0);
}

#[test]
fn wallclock_budget_aborts() {
    let mut m = base_machine();
    m.entry = "spend".into();
    m.terminals.insert("done".into());
    m.budgets.wallclock_s = Some(1);
    m.states.insert("spend".into(), state("spend"));

    let runner = FakeRunner::default();
    // Should never even be reached: the wallclock check happens before spawn.
    runner.script_worker("spend", worker_result(proposal_to("done", "go")));

    let mut ledger = FakeLedger::default();
    let cfg = config();
    let artifacts = FakeArtifacts;
    let stage = FakeStageBuilder { machine: &m };
    let checks = FakeChecks::default();
    let mut engine = Engine {
        machine: &m,
        config: &cfg,
        runner: &runner,
        checks: &checks,
        ledger: &mut ledger,
        artifacts: &artifacts,
        stage: &stage,
        started_at: Some(std::time::Instant::now() - std::time::Duration::from_secs(10)),
    };
    let outcome = engine.run().unwrap();
    assert_eq!(outcome.status, RunStatus::Aborted);
    assert_eq!(runner.worker_calls.borrow().len(), 0);
}

// ── navigator: blocked proposal, and navigator choosing escalate ────────

#[test]
fn blocked_proposal_routes_through_navigator_to_chosen_state() {
    let mut m = base_machine();
    m.entry = "debug".into();
    m.terminals.insert("qa_staging".into());
    m.states.insert("debug".into(), state("debug"));
    // The navigator's chosen target still needs to be a declared edge (the
    // structural tier isn't waived just because it went via the Navigator).
    m.transitions.push(edge("debug", "qa_staging"));

    let runner = FakeRunner::default();
    runner.script_worker("debug", worker_result(proposal_blocked("need more data")));
    runner.script_navigator(choice("qa_staging"));

    let mut ledger = FakeLedger::default();
    let outcome = run(&m, &runner, &mut ledger);

    assert_eq!(outcome.status, RunStatus::Done);
    assert_eq!(outcome.terminal_state.as_deref(), Some("qa_staging"));
    let navs = ledger.payloads_of("navigator_invoked");
    assert_eq!(navs.len(), 1);
}

#[test]
fn navigator_choosing_escalate_routes_to_escalation_state() {
    let mut m = base_machine();
    m.entry = "debug".into();
    m.terminals.insert("blocked".into());
    m.escalation_state = Some("blocked".into());
    m.states.insert("debug".into(), state("debug"));

    let runner = FakeRunner::default();
    runner.script_worker("debug", worker_result(proposal_blocked("truly stuck")));
    runner.script_navigator(choice_with_addendum(
        "blocked",
        "needs a human; nothing reachable fits",
    ));

    let mut ledger = FakeLedger::default();
    let outcome = run(&m, &runner, &mut ledger);

    // Escalation is a failed run, not a successful one: the ticket did not
    // go through, and the exit status must say so.
    assert_eq!(outcome.status, RunStatus::Failed);
    assert_eq!(outcome.terminal_state.as_deref(), Some("blocked"));
    // Escalation is a harness override: no guard_checked for this commit.
    assert!(ledger.payloads_of("guard_checked").is_empty());
}

// ── validate() ────────────────────────────────────────────────────────────

fn always_resolves(_: &PlaybookRef) -> bool {
    true
}

fn never_resolves(_: &PlaybookRef) -> bool {
    false
}

#[test]
fn validate_catches_missing_entry_state() {
    let mut m = base_machine();
    m.entry = "nope".into();
    m.terminals.insert("done".into());
    let diags = crate::validate(&m, &always_resolves);
    assert!(diags.iter().any(|d| d.message.contains("entry state")));
}

#[test]
fn validate_catches_dangling_transition_targets() {
    let mut m = base_machine();
    m.entry = "a".into();
    m.states.insert("a".into(), state("a"));
    m.transitions.push(edge("a", "nowhere"));
    let diags = crate::validate(&m, &always_resolves);
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("names no state or terminal"))
    );
}

#[test]
fn validate_catches_unreachable_state() {
    let mut m = base_machine();
    m.entry = "a".into();
    m.terminals.insert("done".into());
    m.states.insert("a".into(), state("a"));
    m.states.insert("island".into(), state("island"));
    m.transitions.push(edge("a", "done"));
    let diags = crate::validate(&m, &always_resolves);
    assert!(diags.iter().any(|d| d.message.contains("unreachable")));
}

#[test]
fn validate_catches_no_path_to_terminal() {
    let mut m = base_machine();
    m.entry = "a".into();
    m.terminals.insert("done".into());
    m.states.insert("a".into(), state("a"));
    m.states.insert("dead_end".into(), state("dead_end"));
    m.transitions.push(edge("a", "dead_end"));
    let diags = crate::validate(&m, &always_resolves);
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("no path to any terminal"))
    );
}

#[test]
fn validate_catches_unresolved_playbook() {
    let mut m = base_machine();
    m.entry = "a".into();
    m.terminals.insert("done".into());
    m.states.insert("a".into(), state("a"));
    m.transitions.push(edge("a", "done"));
    let diags = crate::validate(&m, &never_resolves);
    assert!(diags.iter().any(|d| d.message.contains("does not resolve")));
}

#[test]
fn validate_catches_loop_head_never_re_entered() {
    let mut m = base_machine();
    m.entry = "a".into();
    m.terminals.insert("done".into());
    m.states.insert("a".into(), state("a"));
    m.transitions.push(edge("a", "done"));
    m.loops
        .push(loop_spec("orphan", &["a"], 3, OnExhausted::Escalate));
    let diags = crate::validate(&m, &always_resolves);
    assert!(diags.iter().any(|d| d.message.contains("never re-entered")));
}

#[test]
fn validate_catches_escalation_state_not_a_terminal() {
    let mut m = base_machine();
    m.entry = "a".into();
    m.terminals.insert("done".into());
    m.escalation_state = Some("a".into());
    m.states.insert("a".into(), state("a"));
    m.transitions.push(edge("a", "done"));
    let diags = crate::validate(&m, &always_resolves);
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("is not a declared terminal"))
    );
}

/// Two edges between the same pair used to be disambiguated by their `when`
/// guards. Now `select_edge` takes the first and the rest are dead, so the
/// duplicate has to be an error — not a silently ignored `criteria`.
#[test]
fn validate_rejects_duplicate_edges_between_the_same_pair() {
    let mut m = base_machine();
    m.entry = "a".into();
    m.terminals.insert("done".into());
    m.states.insert("a".into(), state("a"));
    m.transitions.push(judged_edge("a", "done", "first"));
    m.transitions.push(judged_edge("a", "done", "second"));
    let diags = crate::validate(&m, &always_resolves);
    assert!(
        diags
            .iter()
            .any(|d| d.severity == crate::Severity::Error
                && d.message.contains("duplicate transition")),
        "got: {diags:?}"
    );
}

#[test]
fn validate_warns_on_validation_state_with_edit_or_write() {
    let mut m = base_machine();
    m.entry = "qa-staging".into();
    m.terminals.insert("done".into());
    m.states.insert(
        "qa-staging".into(),
        state_with_tools("qa-staging", &["read", "edit"]),
    );
    m.transitions
        .push(judged_edge("qa-staging", "done", "looks correct"));
    let diags = crate::validate(&m, &always_resolves);
    assert!(
        diags
            .iter()
            .any(|d| d.severity == crate::Severity::Warning && d.message.contains("allowlists"))
    );
}

/// The counterpart, and the reason the trigger is the stage's identity rather
/// than "gates a criteria edge": `implement → review` is criteria-gated and
/// `implement` must be able to edit. Warning there fires on every ordinary
/// machine — including the shipped `standard-ticket` template — which is how
/// you teach someone to ignore warnings.
#[test]
fn validate_does_not_warn_on_an_implement_state_that_edits() {
    let mut m = base_machine();
    m.entry = "implement".into();
    m.terminals.insert("done".into());
    m.states.insert(
        "implement".into(),
        state_with_tools("implement", &["read", "edit", "write"]),
    );
    m.transitions
        .push(judged_edge("implement", "done", "the plan is done"));
    let diags = crate::validate(&m, &always_resolves);
    assert!(
        !diags.iter().any(|d| d.message.contains("allowlists")),
        "implement legitimately edits; got: {diags:?}"
    );
}

/// A loop head re-entered only by an `on_fail: route` is still a loop head.
/// The shipped `standard-ticket` template loops exactly this way — a failed
/// Judge routes back to `implement` — so treating routes as non-re-entry made
/// `loop validate` reject the template it ships with.
#[test]
fn validate_counts_an_on_fail_route_as_loop_head_re_entry() {
    let mut m = base_machine();
    m.entry = "implement".into();
    m.terminals.insert("done".into());
    m.states.insert("implement".into(), state("implement"));
    m.states.insert("review".into(), state("review"));
    m.transitions.push(loop_core::Transition {
        on_fail: OnFail::Route("implement".into()),
        ..judged_edge("review", "done", "no blocking defects")
    });
    m.transitions
        .push(judged_edge("implement", "review", "plan done"));
    m.loops.push(loop_core::LoopSpec {
        name: "fix".into(),
        states: vec!["implement".into(), "review".into()],
        max_cycles: 4,
        on_exhausted: OnExhausted::Escalate,
    });

    let diags = crate::validate(&m, &always_resolves);
    assert!(
        !diags.iter().any(|d| d.message.contains("never re-entered")),
        "got: {diags:?}"
    );
}

/// Reaching the escalation terminal is a failed run, not a successful one.
/// `blocked` is where an exhausted loop, a capped Navigator, or a worker that
/// cannot route itself ends up — reporting `done` there would tell a human, or
/// a CI wrapper reading the exit status, that the ticket went through.
#[test]
fn reaching_the_escalation_terminal_reports_failed_not_done() {
    let mut m = base_machine();
    m.entry = "work".into();
    m.terminals.insert("done".into());
    m.terminals.insert("blocked".into());
    m.escalation_state = Some("blocked".into());
    m.states.insert("work".into(), state("work"));
    m.transitions.push(edge("work", "done"));

    let runner = FakeRunner::default();
    runner.script_worker("work", worker_result(proposal_blocked("cannot proceed")));
    runner.script_navigator(choice_with_addendum("blocked", "needs a human"));

    let mut ledger = FakeLedger::default();
    let outcome = run(&m, &runner, &mut ledger);

    assert_eq!(outcome.terminal_state.as_deref(), Some("blocked"));
    assert_eq!(outcome.status, RunStatus::Failed);
}

// ── crashed vs stuck workers ─────────────────────────────────────────────

/// A worker whose *process* died must be re-entered, not escalated. The two
/// failures look alike at the proposal layer (both yield no transition) but
/// mean opposite things: a stuck worker asked for routing, a crashed one never
/// got to decide. Escalating on a crash throws away a run a retry would have
/// finished.
#[test]
fn a_crashed_worker_is_re_entered_not_escalated() {
    let mut m = base_machine();
    m.entry = "test".into();
    m.terminals.insert("done".into());
    m.terminals.insert("blocked".into());
    m.escalation_state = Some("blocked".into());
    m.states.insert("test".into(), state("test"));
    m.transitions.push(edge("test", "done"));

    let runner = FakeRunner::default();
    let mut crashed = worker_result(proposal_to("done", "unused"));
    crashed.exit_ok = false;
    crashed.proposal = None;
    runner.script_worker("test", crashed);
    runner.script_worker("test", worker_result(proposal_to("done", "suite green")));

    let mut ledger = FakeLedger::default();
    let outcome = run(&m, &runner, &mut ledger);

    assert_eq!(outcome.status, RunStatus::Done);
    assert_eq!(outcome.terminal_state.as_deref(), Some("done"));

    // Re-entered with a second attempt, and no navigator was consulted.
    let entered = ledger.payloads_of("state_entered");
    assert_eq!(entered.len(), 2);
    assert!(ledger.payloads_of("navigator_invoked").is_empty());

    // The crash left no `worker_output`, so the ledger tail keeps the shape
    // the fold reads as "crashed mid-flight" — an out-of-process `loop resume`
    // therefore recovers exactly as this in-process retry did.
    assert_eq!(ledger.payloads_of("worker_output").len(), 1);
    assert_eq!(ledger.payloads_of("error").len(), 1);
}

/// ...but a stage that dies every time is a real fault, not a flake. It must
/// stop rather than spin on the budget.
#[test]
fn a_stage_that_always_crashes_escalates_after_the_cap() {
    let mut m = base_machine();
    m.entry = "test".into();
    m.terminals.insert("done".into());
    m.terminals.insert("blocked".into());
    m.escalation_state = Some("blocked".into());
    m.states.insert("test".into(), state("test"));
    m.transitions.push(edge("test", "done"));

    let runner = FakeRunner::default();
    for _ in 0..crate::MAX_CRASH_ATTEMPTS + 2 {
        let mut crashed = worker_result(proposal_to("done", "unused"));
        crashed.exit_ok = false;
        crashed.proposal = None;
        runner.script_worker("test", crashed);
    }

    let mut ledger = FakeLedger::default();
    let outcome = run(&m, &runner, &mut ledger);

    assert_eq!(outcome.status, RunStatus::Failed);
    assert_eq!(outcome.terminal_state.as_deref(), Some("blocked"));
    assert_eq!(
        ledger.payloads_of("state_entered").len(),
        crate::MAX_CRASH_ATTEMPTS as usize
    );
}
