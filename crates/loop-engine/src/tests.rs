//! Engine integration tests, against the fakes in [`crate::test_support`].
//! No Lua, no subprocess, no filesystem, no API key.

use serde_json::json;

use loop_core::{Config, OnExhausted, OnFail, Paths, PlaybookRef, RunStatus, Vars};

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

fn run(
    machine: &loop_core::Machine,
    guards: &FakeGuards,
    runner: &FakeRunner,
    ledger: &mut FakeLedger,
) -> Outcome {
    let cfg = config();
    let artifacts = FakeArtifacts;
    let stage = FakeStageBuilder { machine };
    let mut engine = Engine {
        machine,
        config: &cfg,
        guards,
        runner,
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
    let reg = GuardRegistry::default();
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
    m.transitions.push(
        reg.edge_when("review", "done", "review.result == 'clean'", |v| {
            v.get_path("review.result") == Some(&json!("clean"))
        }),
    );

    let runner = FakeRunner::default();
    runner.script_worker(
        "implement",
        worker_result(proposal_to("review", "plan done, build green")),
    );
    runner.script_worker(
        "review",
        worker_result_with_vars(
            proposal_to("done", "clean review"),
            Vars::from_value(json!({"review": {"result": "clean"}})),
        ),
    );
    runner.script_judge(verdict(true, "checklist covered, no TODOs"));

    let guards = reg.evaluator();
    let mut ledger = FakeLedger::default();
    let outcome = run(&m, &guards, &runner, &mut ledger);

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
            "vars_set",
            "worker_output",
            "transition_proposed",
            "guard_checked",
            "transition_committed",
            "run_finished",
        ]
    );

    // implement -> review: judged (criteria), no `when`.
    if let loop_core::EventPayload::GuardChecked {
        structural,
        when,
        criteria,
        ..
    } = ledger.payloads_of("guard_checked")[0]
    {
        assert_eq!(*structural, loop_core::GuardOutcome::Pass);
        assert_eq!(*when, loop_core::GuardOutcome::Skip);
        assert_eq!(*criteria, loop_core::GuardOutcome::Pass);
    } else {
        panic!("expected GuardChecked");
    }

    // review -> done: `when`, no criteria.
    if let loop_core::EventPayload::GuardChecked { when, criteria, .. } =
        ledger.payloads_of("guard_checked")[1]
    {
        assert_eq!(*when, loop_core::GuardOutcome::Pass);
        assert_eq!(*criteria, loop_core::GuardOutcome::Skip);
    } else {
        panic!("expected GuardChecked");
    }
}

// ── guard tiers & on_fail ────────────────────────────────────────────────

#[test]
fn when_guard_fail_retries_source_state() {
    let reg = GuardRegistry::default();
    let mut m = base_machine();
    m.entry = "check".into();
    m.terminals.insert("done".into());
    m.states.insert("check".into(), state("check"));
    m.transitions
        .push(reg.edge_when("check", "done", "qa.result == 'pass'", |v| {
            v.get_path("qa.result") == Some(&json!("pass"))
        }));

    let runner = FakeRunner::default();
    let mut fail = worker_result(proposal_to("done", "think it's fine"));
    fail.vars = Vars::from_value(json!({"qa": {"result": "fail"}}));
    runner.script_worker("check", fail);
    let mut pass = worker_result(proposal_to("done", "actually fine now"));
    pass.vars = Vars::from_value(json!({"qa": {"result": "pass"}}));
    runner.script_worker("check", pass);

    let guards = reg.evaluator();
    let mut ledger = FakeLedger::default();
    let outcome = run(&m, &guards, &runner, &mut ledger);

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
        loop_core::EventPayload::GuardChecked { when, .. } => {
            assert_eq!(*when, loop_core::GuardOutcome::Fail)
        }
        _ => unreachable!(),
    }
    match checks[1] {
        loop_core::EventPayload::GuardChecked { when, .. } => {
            assert_eq!(*when, loop_core::GuardOutcome::Pass)
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

    let guards = GuardRegistry::default().evaluator();
    let mut ledger = FakeLedger::default();
    let outcome = run(&m, &guards, &runner, &mut ledger);

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

    let guards = GuardRegistry::default().evaluator();
    let mut ledger = FakeLedger::default();
    let outcome = run(&m, &guards, &runner, &mut ledger);

    assert_eq!(outcome.status, RunStatus::Failed);
    assert_eq!(outcome.terminal_state.as_deref(), Some("implement"));
}

// ── docs/07 #2: trusted vs untrusted vars ───────────────────────────────

#[test]
fn worker_declared_var_cannot_open_a_gate_that_a_tool_emitted_var_can() {
    let reg = GuardRegistry::default();
    let mut m = base_machine();
    m.entry = "check".into();
    m.terminals.insert("done".into());
    m.states.insert("check".into(), state("check"));
    m.transitions
        .push(reg.edge_when("check", "done", "qa.result == 'pass'", |v| {
            v.get_path("qa.result") == Some(&json!("pass"))
        }));

    let runner = FakeRunner::default();
    // Attempt 1: the worker *claims* pass via the untrusted `transition(vars=...)`
    // channel. `WorkerResult.vars` (the trusted, tool-emitted channel) is empty.
    let mut claimed = worker_result(loop_core::Proposal {
        vars: Vars::from_value(json!({"qa": {"result": "pass"}})),
        ..proposal_to("done", "looks good to me")
    });
    claimed.vars = Vars::new(); // no trusted assertion
    runner.script_worker("check", claimed);

    // Attempt 2: a tool actually asserts it via LOOP_VARS (WorkerResult.vars).
    let mut real = worker_result(proposal_to("done", "tool confirms pass"));
    real.vars = Vars::from_value(json!({"qa": {"result": "pass"}}));
    runner.script_worker("check", real);

    let guards = reg.evaluator();
    let mut ledger = FakeLedger::default();
    let outcome = run(&m, &guards, &runner, &mut ledger);

    assert_eq!(
        outcome.status,
        RunStatus::Done,
        "should eventually pass on attempt 2"
    );

    let checks = ledger.payloads_of("guard_checked");
    assert_eq!(checks.len(), 2);
    match checks[0] {
        loop_core::EventPayload::GuardChecked { when, .. } => {
            assert_eq!(
                *when,
                loop_core::GuardOutcome::Fail,
                "the worker's own untrusted vars claim must not open the gate"
            )
        }
        _ => unreachable!(),
    }
    match checks[1] {
        loop_core::EventPayload::GuardChecked { when, .. } => {
            assert_eq!(*when, loop_core::GuardOutcome::Pass)
        }
        _ => unreachable!(),
    }

    let vars_events = ledger.payloads_of("vars_set");
    let trust_flags: Vec<bool> = vars_events
        .iter()
        .map(|p| match p {
            loop_core::EventPayload::VarsSet { trusted, .. } => *trusted,
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(trust_flags, vec![false, true]);
}

// ── select_edge: multi-edge disambiguation ──────────────────────────────

#[test]
fn select_edge_picks_first_passing_when_among_same_from_and_to() {
    let reg = GuardRegistry::default();
    let mut m = base_machine();
    m.states.insert("a".into(), state("a"));
    m.states.insert("b".into(), state("b"));
    let e1 = reg.edge_when("a", "b", "always false", |_| false);
    let e2 = reg.edge_when("a", "b", "always true", |_| true);
    m.transitions.push(e1);
    m.transitions.push(e2.clone());

    let guards = reg.evaluator();
    let found = select_edge(&m, &guards, "a", "b", &Vars::new())
        .unwrap()
        .unwrap();
    assert_eq!(found.when_src.as_deref(), Some("always true"));

    // If none pass, `select_edge` returns `None`.
    m.transitions.pop();
    m.transitions.push(loop_core::Transition {
        when: e2.when,
        ..reg.edge_when("a", "b", "always false 2", |_| false)
    });
    // rebuild with two always-false guards
    let reg2 = GuardRegistry::default();
    let mut m2 = base_machine();
    m2.states.insert("a".into(), state("a"));
    m2.states.insert("b".into(), state("b"));
    m2.transitions
        .push(reg2.edge_when("a", "b", "f1", |_| false));
    m2.transitions
        .push(reg2.edge_when("a", "b", "f2", |_| false));
    let guards2 = reg2.evaluator();
    let none = select_edge(&m2, &guards2, "a", "b", &Vars::new()).unwrap();
    assert!(none.is_none());
}

// ── docs/06: transient vs real routing, self-loop with backoff ─────────

#[test]
fn transient_vs_real_routing_drives_self_loop_then_real_failure_to_debug() {
    let reg = GuardRegistry::default();
    let mut m = base_machine();
    m.entry = "start".into();
    m.terminals.insert("validate".into());
    m.states.insert("start".into(), state("start"));
    m.states.insert("qa_staging".into(), state("qa_staging"));
    m.states.insert("debug".into(), state("debug"));

    m.transitions.push(edge("start", "qa_staging"));
    m.transitions.push(
        reg.edge_when("qa_staging", "validate", "qa.result == 'pass'", |v| {
            v.get_path("qa.result") == Some(&json!("pass"))
        }),
    );
    m.transitions.push(loop_core::Transition {
        backoff_s: Some(0),
        ..reg.edge_when("qa_staging", "qa_staging", "fail && transient", |v| {
            v.get_path("qa.result") == Some(&json!("fail"))
                && v.get_path("qa.error_class") == Some(&json!("transient"))
        })
    });
    m.transitions
        .push(reg.edge_when("qa_staging", "debug", "fail && real", |v| {
            v.get_path("qa.result") == Some(&json!("fail"))
                && v.get_path("qa.error_class") == Some(&json!("real"))
        }));
    m.transitions.push(edge("debug", "qa_staging"));
    m.loops
        .push(loop_spec("qa", &["qa_staging"], 5, OnExhausted::Escalate));

    let runner = FakeRunner::default();
    runner.script_worker("start", worker_result(proposal_to("qa_staging", "go")));
    let mut cycle1 = worker_result(proposal_to("qa_staging", "transient, retry"));
    cycle1.vars = Vars::from_value(json!({"qa": {"result": "fail", "error_class": "transient"}}));
    runner.script_worker("qa_staging", cycle1);
    let mut cycle2 = worker_result(proposal_to("debug", "real failure"));
    cycle2.vars = Vars::from_value(json!({"qa": {"result": "fail", "error_class": "real"}}));
    runner.script_worker("qa_staging", cycle2);
    runner.script_worker(
        "debug",
        worker_result(proposal_to("qa_staging", "fixed it")),
    );
    let mut cycle3 = worker_result(proposal_to("validate", "all green"));
    cycle3.vars = Vars::from_value(json!({"qa": {"result": "pass"}}));
    runner.script_worker("qa_staging", cycle3);

    let guards = reg.evaluator();
    let mut ledger = FakeLedger::default();
    let outcome = run(&m, &guards, &runner, &mut ledger);

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
    let reg = GuardRegistry::default();
    let mut m = base_machine();
    m.entry = "start".into();
    m.terminals.insert("blocked".into());
    m.escalation_state = Some("blocked".into());
    m.states.insert("start".into(), state("start"));
    m.states.insert("qa_staging".into(), state("qa_staging"));
    m.transitions.push(edge("start", "qa_staging"));
    m.transitions.push(loop_core::Transition {
        backoff_s: Some(0),
        ..reg.edge_when("qa_staging", "qa_staging", "always retry", |_| true)
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

    let guards = reg.evaluator();
    let mut ledger = FakeLedger::default();
    let outcome = run(&m, &guards, &runner, &mut ledger);

    assert_eq!(outcome.status, RunStatus::Done);
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
    let reg = GuardRegistry::default();
    let mut m = base_machine();
    m.entry = "start".into();
    m.terminals.insert("done".into());
    m.states.insert("start".into(), state("start"));
    m.states.insert("qa_staging".into(), state("qa_staging"));
    m.transitions.push(edge("start", "qa_staging"));
    m.transitions.push(loop_core::Transition {
        backoff_s: Some(0),
        ..reg.edge_when("qa_staging", "qa_staging", "always retry", |_| true)
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

    let guards = reg.evaluator();
    let mut ledger = FakeLedger::default();
    let outcome = run(&m, &guards, &runner, &mut ledger);

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

    let guards = GuardRegistry::default().evaluator();
    let mut ledger = FakeLedger::default();
    let outcome = run(&m, &guards, &runner, &mut ledger);

    assert_eq!(outcome.status, RunStatus::Done);
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

    let guards = GuardRegistry::default().evaluator();
    let mut ledger = FakeLedger::default();
    let outcome = run(&m, &guards, &runner, &mut ledger);

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

    let guards = GuardRegistry::default().evaluator();
    let mut ledger = FakeLedger::default();
    let outcome = run(&m, &guards, &runner, &mut ledger);

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

    let guards = GuardRegistry::default().evaluator();
    let mut ledger = FakeLedger::default();
    let cfg = config();
    let artifacts = FakeArtifacts;
    let stage = FakeStageBuilder { machine: &m };
    let mut engine = Engine {
        machine: &m,
        config: &cfg,
        guards: &guards,
        runner: &runner,
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

    let guards = GuardRegistry::default().evaluator();
    let mut ledger = FakeLedger::default();
    let outcome = run(&m, &guards, &runner, &mut ledger);

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

    let guards = GuardRegistry::default().evaluator();
    let mut ledger = FakeLedger::default();
    let outcome = run(&m, &guards, &runner, &mut ledger);

    assert_eq!(outcome.status, RunStatus::Done);
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

#[test]
fn validate_warns_on_ungrounded_when_gate() {
    let reg = GuardRegistry::default();
    let mut m = base_machine();
    m.entry = "a".into();
    m.terminals.insert("done".into());
    m.states
        .insert("a".into(), state_with_tools("a", &["read", "bash"]));
    m.transitions
        .push(reg.edge_when("a", "done", "qa.result == 'pass'", |v| {
            v.get_path("qa.result") == Some(&json!("pass"))
        }));
    let diags = crate::validate(&m, &always_resolves);
    assert!(
        diags
            .iter()
            .any(|d| d.severity == crate::Severity::Warning && d.message.contains("`qa`"))
    );
}

#[test]
fn validate_warns_on_qa_shaped_state_with_edit_or_write() {
    let mut m = base_machine();
    m.entry = "a".into();
    m.terminals.insert("done".into());
    m.states
        .insert("a".into(), state_with_tools("a", &["read", "edit"]));
    m.transitions
        .push(judged_edge("a", "done", "looks correct"));
    let diags = crate::validate(&m, &always_resolves);
    assert!(
        diags
            .iter()
            .any(|d| d.severity == crate::Severity::Warning && d.message.contains("allowlists"))
    );
}
