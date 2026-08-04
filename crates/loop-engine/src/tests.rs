//! Engine integration tests, against the fakes in [`crate::test_support`].
//! No Lua, no subprocess, no filesystem, no API key.

use loop_core::{Config, OnExhausted, OnFail, Paths, PlaybookRef, RunStatus};

use crate::guards::select_edge;
use crate::test_support::*;
use crate::{Engine, Outcome};

fn config() -> Config {
    Config::defaults(Paths {
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
    let stage = FakeStageBuilder::new(machine);
    let mut engine = Engine {
        machine,
        config: &cfg,
        runner,
        checks,
        ledger,
        artifacts: &artifacts,
        stage: &stage,
        started_at: None,
        elapsed_offset_s: 0,
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

// ── examples/: transient vs real routing, self-loop with backoff ─────────

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
    let stage = FakeStageBuilder::new(&m);
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
        elapsed_offset_s: 0,
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
    let diags = crate::validate(&m, &always_resolves, &|_| true);
    assert!(diags.iter().any(|d| d.message.contains("entry state")));
}

#[test]
fn validate_catches_dangling_transition_targets() {
    let mut m = base_machine();
    m.entry = "a".into();
    m.states.insert("a".into(), state("a"));
    m.transitions.push(edge("a", "nowhere"));
    let diags = crate::validate(&m, &always_resolves, &|_| true);
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
    let diags = crate::validate(&m, &always_resolves, &|_| true);
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
    let diags = crate::validate(&m, &always_resolves, &|_| true);
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
    let diags = crate::validate(&m, &never_resolves, &|_| true);
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
    let diags = crate::validate(&m, &always_resolves, &|_| true);
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
    let diags = crate::validate(&m, &always_resolves, &|_| true);
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
    let diags = crate::validate(&m, &always_resolves, &|_| true);
    assert!(
        diags
            .iter()
            .any(|d| d.severity == crate::Severity::Error
                && d.message.contains("duplicate transition")),
        "got: {diags:?}"
    );
}

/// Skills resolve through the same filesystem seam playbooks do, so a typo in
/// a skill name is caught by `loop validate` rather than at spawn time.
#[test]
fn validate_reports_a_skill_that_does_not_resolve() {
    let mut m = base_machine();
    m.entry = "qa-staging".into();
    m.terminals.insert("done".into());
    m.states.insert(
        "qa-staging".into(),
        state_with_skills("qa-staging", &["contract-check"]),
    );
    m.transitions
        .push(judged_edge("qa-staging", "done", "looks correct"));

    let diags = crate::validate(&m, &always_resolves, &|name| name != "contract-check");
    assert!(
        diags
            .iter()
            .any(|d| d.severity == crate::Severity::Error && d.message.contains("contract-check")),
        "got: {diags:?}"
    );
}

/// A stage loads the union of the machine's `:defaults {:skills ..}` and the
/// state's — so that union is what has to lint. Checking only the state's
/// layer left a typo in the machine defaults to surface as a failed spawn
/// mid-run, which is the one place `validate` exists to prevent.
#[test]
fn validate_checks_skills_that_come_from_the_machine_defaults() {
    let mut m = base_machine();
    m.entry = "implement".into();
    m.terminals.insert("done".into());
    m.states.insert("implement".into(), state("implement"));
    m.transitions
        .push(judged_edge("implement", "done", "looks correct"));

    m.defaults.skills = vec!["hose-typo".to_string()];
    let diags = crate::validate(&m, &always_resolves, &|name| name != "hose-typo");
    let d = diags
        .iter()
        .find(|d| d.message.contains("hose-typo"))
        .unwrap_or_else(|| panic!("expected a diagnostic for the default skill: {diags:?}"));
    assert_eq!(d.severity, crate::Severity::Error);
    assert!(
        d.message.contains(":defaults"),
        "the diagnostic must say where the name came from: {}",
        d.message
    );
}

/// Same reasoning for MCP, one layer up: a server named in the machine's
/// `:defaults` is the identical misconfiguration as one named on the state.
#[test]
fn validate_checks_mcp_servers_that_come_from_the_machine_defaults() {
    let mut m = base_machine();
    m.entry = "implement".into();
    m.terminals.insert("done".into());
    m.states.insert("implement".into(), state("implement"));
    m.transitions
        .push(judged_edge("implement", "done", "looks correct"));

    m.defaults.mcp = vec!["warehouse".to_string()];
    m.pi_extensions.clear();
    let diags = crate::validate(&m, &always_resolves, &|_| true);
    assert!(
        diags.iter().any(|d| d.message.contains("MCP servers")),
        "got: {diags:?}"
    );
}

/// The server names are the user's business, but the tool that connects them
/// is loop's: a stage told to call `mcp({connect: …})` in a spawn without the
/// `mcp` extension fails at run time for a reason `validate` can see now.
#[test]
fn validate_reports_named_mcp_servers_without_the_mcp_extension() {
    let mut m = base_machine();
    m.entry = "qa-staging".into();
    m.terminals.insert("done".into());
    m.states.insert(
        "qa-staging".into(),
        state_with_mcp("qa-staging", &["warehouse"]),
    );
    m.transitions
        .push(judged_edge("qa-staging", "done", "looks correct"));
    m.pi_extensions.clear();

    let diags = crate::validate(&m, &always_resolves, &|_| true);
    assert!(
        diags
            .iter()
            .any(|d| d.severity == crate::Severity::Error && d.message.contains("pi-extensions")),
        "got: {diags:?}"
    );

    // With the extension declared, an unverifiable server name is not an
    // error: loop never reads the user's mcp.json and has nothing to check it
    // against.
    m.pi_extensions.push("mcp".into());
    let ok = crate::validate(&m, &always_resolves, &|_| true);
    assert!(!ok.iter().any(|d| d.message.contains("MCP")), "got: {ok:?}");
}

/// An edge with neither tier commits whatever the worker proposed. That is
/// occasionally right — an unconditional hand-off — so it warns rather than
/// failing. It must still be said: with `when` gone, unguarded is the shape
/// you get by forgetting.
#[test]
fn validate_warns_on_an_edge_with_no_check_and_no_criteria() {
    let mut m = base_machine();
    m.entry = "a".into();
    m.terminals.insert("done".into());
    m.states.insert("a".into(), state("a"));
    m.transitions.push(edge("a", "done"));

    let diags = crate::validate(&m, &always_resolves, &|_| true);
    assert!(
        diags.iter().any(|d| d.severity == crate::Severity::Warning
            && d.message.contains("committed unexamined")),
        "got: {diags:?}"
    );
}

/// The counterpart: a guarded edge is silent, so the warning stays worth reading.
#[test]
fn validate_does_not_warn_on_a_guarded_edge() {
    let mut m = base_machine();
    m.entry = "a".into();
    m.terminals.insert("done".into());
    m.states.insert("a".into(), state("a"));
    m.transitions
        .push(judged_edge("a", "done", "the work is done"));

    let diags = crate::validate(&m, &always_resolves, &|_| true);
    assert!(
        !diags
            .iter()
            .any(|d| d.message.contains("committed unexamined")),
        "got: {diags:?}"
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

    let diags = crate::validate(&m, &always_resolves, &|_| true);
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

// ── what actually reaches the stage ──────────────────────────────────────

/// Runs a machine and hands back the contexts every stage was built with, so a
/// test can assert on what the playbook would have seen.
fn run_capturing_stages(
    machine: &loop_core::Machine,
    runner: &FakeRunner,
    ledger: &mut FakeLedger,
) -> (Outcome, Vec<loop_core::Context>) {
    let cfg = config();
    let artifacts = FakeArtifacts;
    let checks = FakeChecks::default();
    let stage = FakeStageBuilder::new(machine);
    let outcome = {
        let mut engine = Engine {
            machine,
            config: &cfg,
            runner,
            checks: &checks,
            ledger,
            artifacts: &artifacts,
            stage: &stage,
            started_at: None,
            elapsed_offset_s: 0,
        };
        engine.run().expect("engine run should not error")
    };
    let contexts = stage.contexts.borrow().clone();
    (outcome, contexts)
}

/// The Navigator's get-back-on-track note has to survive the ordinary route:
/// through a guarded edge, which puts a `guard_checked` between the
/// `navigator_invoked` and the commit. Reading only the line before the commit
/// meant the note arrived exactly when it could not be used — on the route to
/// the terminal escalation state, which never renders a playbook.
#[test]
fn the_navigators_addendum_reaches_the_state_it_routed_into() {
    let mut m = base_machine();
    m.entry = "implement".into();
    m.terminals.insert("done".into());
    m.states.insert("implement".into(), state("implement"));
    m.states.insert("debug".into(), state("debug"));
    m.transitions.push(judged_edge(
        "implement",
        "debug",
        "a real failure to diagnose",
    ));
    m.transitions.push(edge("debug", "done"));

    let runner = FakeRunner::default();
    runner.script_worker("implement", worker_result(proposal_blocked("I am lost")));
    runner.script_navigator(choice_with_addendum(
        "debug",
        "the migration is half-applied; start there",
    ));
    runner.script_judge(verdict(true, "routing to debug is right"));
    runner.script_worker("debug", worker_result(proposal_to("done", "fixed")));

    let mut ledger = FakeLedger::default();
    let (outcome, contexts) = run_capturing_stages(&m, &runner, &mut ledger);

    assert_eq!(outcome.status, RunStatus::Done);
    let debug_ctx = contexts
        .iter()
        .find(|c| c.state == "debug")
        .expect("debug stage was built");
    assert_eq!(
        debug_ctx.entry_addendum.as_deref(),
        Some("the migration is half-applied; start there"),
    );
    // And it is delivered once: the next stage's entry must not re-serve a
    // note from a decision the run has already moved past.
    let implement_ctx = contexts
        .iter()
        .find(|c| c.state == "implement")
        .expect("implement stage was built");
    assert_eq!(implement_ctx.entry_addendum, None);
}

/// A re-entry after a crash is a different situation from a first attempt —
/// the stage may have already opened the PR it is about to open again — so the
/// playbook gets told.
#[test]
fn a_re_entry_after_a_crash_is_marked_for_the_playbook() {
    let mut m = base_machine();
    m.entry = "ship".into();
    m.terminals.insert("done".into());
    m.states.insert("ship".into(), state("ship"));
    m.transitions.push(edge("ship", "done"));

    let runner = FakeRunner::default();
    let mut crashed = worker_result(proposal_to("done", "unused"));
    crashed.exit_ok = false;
    crashed.proposal = None;
    runner.script_worker("ship", crashed);
    runner.script_worker("ship", worker_result(proposal_to("done", "pr is up")));

    let mut ledger = FakeLedger::default();
    let (outcome, contexts) = run_capturing_stages(&m, &runner, &mut ledger);

    assert_eq!(outcome.status, RunStatus::Done);
    let crashed_flags: Vec<bool> = contexts.iter().map(|c| c.crashed).collect();
    assert_eq!(
        crashed_flags,
        vec![false, true],
        "the first entry is clean; the retry follows a death"
    );
    assert_eq!(contexts[1].to_map()["CRASHED"], "1");
    assert_eq!(contexts[0].to_map()["CRASHED"], "");
}

// ── artifact claims ──────────────────────────────────────────────────────

/// A worker naming a file it never wrote is an ordinary mistake, and the run
/// has to absorb it: record the drop, keep the claims that did resolve, and
/// let the guard that wanted the evidence be the thing that fails. Propagating
/// the capture error instead killed the `loop run` process outright, leaving
/// the ledger tail at `state_entered`.
#[test]
fn an_unresolvable_artifact_claim_is_recorded_and_dropped_not_fatal() {
    let mut m = base_machine();
    m.entry = "implement".into();
    m.terminals.insert("done".into());
    m.states.insert("implement".into(), state("implement"));
    m.transitions.push(edge("implement", "done"));

    let runner = FakeRunner::default();
    let mut proposal = proposal_to("done", "wrote the diff");
    proposal.artifacts = vec![
        loop_core::ArtifactClaim {
            name: "diff".into(),
            path: "diff.patch".into(),
        },
        loop_core::ArtifactClaim {
            name: "notes".into(),
            path: "never-written.md".into(),
        },
    ];
    runner.script_worker("implement", worker_result(proposal));

    let mut ledger = FakeLedger::default();
    let artifacts = RefusingArtifacts {
        refuse: "never-written.md",
    };
    let cfg = config();
    let checks = FakeChecks::default();
    let stage = FakeStageBuilder::new(&m);
    let outcome = {
        let mut engine = Engine {
            machine: &m,
            config: &cfg,
            runner: &runner,
            checks: &checks,
            ledger: &mut ledger,
            artifacts: &artifacts,
            stage: &stage,
            started_at: None,
            elapsed_offset_s: 0,
        };
        engine.run().expect("a bad claim must not fail the run")
    };

    assert_eq!(outcome.status, RunStatus::Done);
    match ledger.payloads_of("worker_output")[0] {
        loop_core::EventPayload::WorkerOutput { artifacts, .. } => {
            let names: Vec<&str> = artifacts.iter().map(|a| a.name.as_str()).collect();
            assert_eq!(names, vec!["diff"], "the good claim still lands");
        }
        _ => unreachable!(),
    }
    let errors = ledger.payloads_of("error");
    assert_eq!(errors.len(), 1);
    match errors[0] {
        loop_core::EventPayload::Error { detail, .. } => {
            assert!(detail.contains("never-written.md"), "got: {detail}");
            assert!(detail.contains("notes"), "got: {detail}");
        }
        _ => unreachable!(),
    }
}

// ── the Judge's spend ────────────────────────────────────────────────────

/// Criteria are not free, and a machine that leans on them was spending real
/// money the `:usd` budget could not see. The verdict's usage lands on the
/// guard event, which is what the fold adds up.
#[test]
fn the_judges_usage_lands_on_the_guard_event_and_in_the_totals() {
    let mut m = base_machine();
    m.entry = "implement".into();
    m.terminals.insert("done".into());
    m.states.insert("implement".into(), state("implement"));
    m.transitions
        .push(judged_edge("implement", "done", "the plan is addressed"));

    let runner = FakeRunner::default();
    runner.script_worker(
        "implement",
        worker_result_costing(proposal_to("done", "did it"), 1.0),
    );
    runner.script_judge(loop_core::Verdict {
        pass: true,
        rationale: "addressed".into(),
        usage: loop_core::Usage {
            tokens: 500,
            cost_usd: 0.25,
        },
    });

    let mut ledger = FakeLedger::default();
    let outcome = run(&m, &runner, &mut ledger);

    match ledger.payloads_of("guard_checked")[0] {
        loop_core::EventPayload::GuardChecked { usage, .. } => {
            assert_eq!(usage.cost_usd, 0.25);
            assert_eq!(usage.tokens, 500);
        }
        _ => unreachable!(),
    }
    assert!(
        (outcome.totals.cost_usd - 1.25).abs() < 1e-9,
        "the run's cost must include the judge: {}",
        outcome.totals.cost_usd
    );
}

/// A judge-heavy machine has to be able to blow the dollar budget on judging
/// alone, or the guardrail is not a guardrail.
#[test]
fn judge_spend_alone_can_exhaust_the_usd_budget() {
    let mut m = base_machine();
    m.entry = "a".into();
    m.terminals.insert("done".into());
    m.budgets.usd = Some(0.30);
    m.states.insert("a".into(), state("a"));
    m.states.insert("b".into(), state("b"));
    m.transitions.push(judged_edge("a", "b", "good enough"));
    m.transitions.push(judged_edge("b", "done", "good enough"));

    let runner = FakeRunner::default();
    // Free workers; every dollar on this run comes from the criteria tier.
    runner.script_worker("a", worker_result_costing(proposal_to("b", "on"), 0.0));
    runner.script_worker("b", worker_result_costing(proposal_to("done", "on"), 0.0));
    for _ in 0..2 {
        runner.script_judge(loop_core::Verdict {
            pass: true,
            rationale: "fine".into(),
            usage: loop_core::Usage {
                tokens: 100,
                cost_usd: 0.4,
            },
        });
    }

    let mut ledger = FakeLedger::default();
    let outcome = run(&m, &runner, &mut ledger);

    assert_eq!(outcome.status, RunStatus::Aborted);
    let errors = ledger.payloads_of("error");
    match errors.last().expect("a budget error") {
        loop_core::EventPayload::Error { detail, .. } => {
            assert!(detail.contains("budget_usd exceeded"), "got: {detail}");
        }
        _ => unreachable!(),
    }
}
