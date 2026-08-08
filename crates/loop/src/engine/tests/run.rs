//! Driving the control loop: guard tiers, `on_fail`, bounds, the Navigator,
//! crashes, artifact claims, and what reaches a stage.

use crate::core::{GuardOutcome, OnExhausted, OnFail, RunStatus, Usage, Verdict};

use super::{Rig, crashed_worker, drive, drive_checked, drive_with};
use crate::engine::test_support::*;

// ── happy path ────────────────────────────────────────────────────────────

#[test]
fn happy_path_entry_to_terminal_with_right_ledger_sequence() {
    let m = machine()
        .entry("implement")
        .terminal("done")
        .edge(judged_edge(
            "implement",
            "review",
            "plan addressed, build green",
        ))
        .edge(judged_edge("review", "done", "no blocking defects remain"))
        .build();

    let runner = FakeRunner::default();
    runner.script_worker(
        "implement",
        worker_result(proposal_to("review", "plan done, build green")),
    );
    runner.script_worker("review", worker_result(proposal_to("done", "clean review")));
    runner.script_judge(verdict(true, "checklist covered, no TODOs"));
    runner.script_judge(verdict(true, "no defects worth blocking on"));

    let mut ledger = FakeLedger::default();
    let outcome = drive(&m, &runner, &mut ledger);

    assert_eq!(outcome.status, RunStatus::Done);
    assert_eq!(outcome.terminal_state.as_deref(), Some("done"));

    assert_eq!(
        ledger.kinds(),
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

    let criteria: Vec<_> = ledger.guards().iter().map(|g| g.criteria).collect();
    assert_eq!(criteria, vec![GuardOutcome::Pass, GuardOutcome::Pass]);
}

/// One read of the ledger per iteration of the loop, however many events that
/// iteration appends.
///
/// The loop body used to re-`read_all` every time it needed a fresh fold —
/// after the worker's events landed, and again on an `on_fail` retry — so a
/// single step read and re-parsed the whole growing file three or four times.
/// `append` hands the event back, so the body keeps its own copy current
/// instead.
#[test]
fn one_ledger_read_per_step_however_many_events_it_writes() {
    let m = machine()
        .entry("implement")
        .terminal("done")
        .edge(judged_edge("implement", "review", "plan addressed"))
        .edge(judged_edge("review", "done", "no defects"))
        .build();

    let runner = FakeRunner::default();
    runner.script_worker("implement", worker_result(proposal_to("review", "done")));
    runner.script_worker("review", worker_result(proposal_to("done", "clean")));
    runner.script_judge(verdict(true, "ok"));
    runner.script_judge(verdict(true, "ok"));

    let mut ledger = FakeLedger::default();
    drive(&m, &runner, &mut ledger);

    // Three iterations: implement, review, and the one that finds `done`
    // terminal. Twelve events written between them.
    assert_eq!(ledger.events.len(), 12);
    assert_eq!(
        ledger.reads(),
        3,
        "one read per step, not one per fold — got {} reads for 12 events",
        ledger.reads()
    );
}

// ── guard tiers & on_fail ────────────────────────────────────────────────

#[test]
fn judge_fail_retries_source_state() {
    let m = machine()
        .entry("check")
        .terminal("done")
        .edge(judged_edge("check", "done", "the suite is green"))
        .build();

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
    let outcome = drive(&m, &runner, &mut ledger);

    assert_eq!(outcome.status, RunStatus::Done);
    assert_eq!(ledger.attempts(), vec![1, 2]);

    let criteria: Vec<_> = ledger.guards().iter().map(|g| g.criteria).collect();
    assert_eq!(criteria, vec![GuardOutcome::Fail, GuardOutcome::Pass]);
}

#[test]
fn judge_fail_with_on_fail_route_sends_elsewhere_without_further_guard_checks() {
    let m = machine()
        .entry("implement")
        .terminal("done")
        .terminal("needs_human")
        .edge(crate::core::Transition {
            on_fail: OnFail::Route("needs_human".into()),
            ..judged_edge("implement", "review", "clean and green")
        })
        .build();

    let runner = FakeRunner::default();
    runner.script_worker(
        "implement",
        worker_result(proposal_to("review", "done, I think")),
    );
    runner.script_judge(verdict(false, "TODOs remain"));

    let mut ledger = FakeLedger::default();
    let outcome = drive(&m, &runner, &mut ledger);

    assert_eq!(outcome.status, RunStatus::Done);
    assert_eq!(outcome.terminal_state.as_deref(), Some("needs_human"));

    // No second guard check for the routed commit.
    assert_eq!(ledger.guards().len(), 1);
    assert_eq!(
        ledger.commits(),
        vec![("implement".to_string(), "needs_human".to_string())]
    );
}

#[test]
fn on_fail_abort_finishes_run_failed() {
    let m = machine()
        .entry("implement")
        // An unreachable terminal, irrelevant here.
        .terminal("review")
        .edge(crate::core::Transition {
            on_fail: OnFail::Abort,
            ..judged_edge("implement", "review", "clean and green")
        })
        .build();

    let runner = FakeRunner::default();
    runner.script_worker(
        "implement",
        worker_result(proposal_to("review", "done, I think")),
    );
    runner.script_judge(verdict(false, "not clean"));

    let mut ledger = FakeLedger::default();
    let outcome = drive(&m, &runner, &mut ledger);

    assert_eq!(outcome.status, RunStatus::Failed);
    assert_eq!(outcome.terminal_state.as_deref(), Some("implement"));
}

// ── the check tier ───────────────────────────────────────────────────────

/// The whole point of the tier: the check runs in the harness's own
/// subprocess, and its exit code decides. Nothing the worker said is consulted.
#[test]
fn a_failing_check_blocks_the_edge_and_never_reaches_the_judge() {
    let m = machine()
        .entry("implement")
        .terminal("done")
        .edge(crate::core::Transition {
            criteria: Some("the plan is done".into()),
            on_fail: OnFail::Abort,
            ..checked_edge("implement", "done", "cargo test")
        })
        .build();

    let runner = FakeRunner::default();
    runner.script_worker(
        "implement",
        worker_result(proposal_to("done", "all green, promise")),
    );
    let checks = FakeChecks::default();
    checks.script_fail("2 tests failed");

    let mut ledger = FakeLedger::default();
    let outcome = drive_checked(&m, &runner, &checks, &mut ledger);

    assert_eq!(outcome.status, RunStatus::Failed);
    assert_eq!(
        runner.judge_calls.borrow().len(),
        0,
        "a failed check must not be appealable to the Judge"
    );

    let g = &ledger.guards()[0];
    assert_eq!(g.check, GuardOutcome::Fail);
    assert_eq!(g.criteria, GuardOutcome::Skip);
    assert_eq!(g.check_output.as_deref(), Some("2 tests failed"));
}

/// A passing check does not by itself pass the edge — it is a precondition,
/// and the semantic criterion still gets its say.
#[test]
fn a_passing_check_still_defers_to_the_judge_and_hands_it_the_output() {
    let m = machine()
        .entry("implement")
        .terminal("done")
        .edge(crate::core::Transition {
            criteria: Some("every plan item is addressed".into()),
            on_fail: OnFail::Abort,
            ..checked_edge("implement", "done", "cargo test")
        })
        .build();

    let runner = FakeRunner::default();
    runner.script_worker("implement", worker_result(proposal_to("done", "done")));
    runner.script_judge(verdict(false, "item 3 was never touched"));
    let checks = FakeChecks::default();
    checks.script_pass("test result: ok. 41 passed");

    let mut ledger = FakeLedger::default();
    let outcome = drive_checked(&m, &runner, &checks, &mut ledger);

    assert_eq!(outcome.status, RunStatus::Failed);
    assert_eq!(
        runner.judge_calls.borrow()[0].check_output.as_deref(),
        Some("test result: ok. 41 passed"),
        "the Judge must see the one piece of evidence the worker didn't author"
    );

    let g = &ledger.guards()[0];
    assert_eq!(g.check, GuardOutcome::Pass);
    assert_eq!(g.criteria, GuardOutcome::Fail);
}

/// An edge with no `:check` skips the tier entirely — no subprocess, no
/// spurious `check_output` on the ledger line.
#[test]
fn an_edge_without_a_check_skips_the_tier() {
    let m = machine()
        .entry("implement")
        .terminal("done")
        .edge(judged_edge("implement", "done", "the plan is done"))
        .build();

    let runner = FakeRunner::default();
    runner.script_worker("implement", worker_result(proposal_to("done", "done")));
    runner.script_judge(verdict(true, "checklist covered"));
    let checks = FakeChecks::default();

    let mut ledger = FakeLedger::default();
    let outcome = drive_checked(&m, &runner, &checks, &mut ledger);

    assert_eq!(outcome.status, RunStatus::Done);
    assert!(checks.commands().is_empty());

    let g = &ledger.guards()[0];
    assert_eq!(g.check, GuardOutcome::Skip);
    assert!(g.check_output.is_none());
}

/// The check runs with the identity of the stage that just finished, so a
/// cycle-scoped command inspects the namespace that stage actually deployed to.
#[test]
fn a_check_runs_with_the_finishing_stages_cycle_and_attempt() {
    let m = machine()
        .entry("start")
        .terminal("done")
        .edge(edge("start", "qa"))
        .edge(crate::core::Transition {
            backoff_s: Some(0),
            ..checked_edge("qa", "qa", "contract-check --ns loop-$CYCLE")
        })
        .edge(checked_edge("qa", "done", "contract-check --final"))
        .loop_over(loop_spec("qa", &["qa"], 4, OnExhausted::Escalate))
        .build();

    let runner = FakeRunner::default();
    runner.script_worker("start", worker_result(proposal_to("qa", "go")));
    runner.script_worker("qa", worker_result(proposal_to("qa", "flaky, retry")));
    runner.script_worker("qa", worker_result(proposal_to("done", "green")));
    let checks = FakeChecks::default();

    let mut ledger = FakeLedger::default();
    let outcome = drive_checked(&m, &runner, &checks, &mut ledger);
    assert_eq!(outcome.status, RunStatus::Done);

    let positions: Vec<(String, u32)> = checks
        .ran
        .borrow()
        .iter()
        .map(|(_, from, cycle, _)| (from.clone(), *cycle))
        .collect();
    assert_eq!(
        positions,
        vec![("qa".to_string(), 1), ("qa".to_string(), 2)],
        "the self-loop's second check runs on cycle 2, not cycle 1"
    );
}

// ── Machine::edge ───────────────────────────────────────────────────────

/// Parallel edges used to be told apart by their `when` guards. Without those,
/// the first declared edge wins and the rest are dead — which is why
/// [`crate::engine::validate`] rejects the duplicate rather than letting it
/// silently decide which `criteria` applies.
#[test]
fn edge_takes_the_first_declared_edge_and_none_when_absent() {
    let m = machine()
        .edge(judged_edge("a", "b", "first"))
        .edge(judged_edge("a", "b", "second"))
        .build();

    let found = m.edge("a", "b").expect("an edge exists");
    assert_eq!(found.criteria.as_deref(), Some("first"));

    assert!(m.edge("b", "a").is_none());
}

// ── examples/: transient vs real routing, self-loop with backoff ─────────

#[test]
fn transient_vs_real_routing_drives_self_loop_then_real_failure_to_debug() {
    let m = machine()
        .entry("start")
        .terminal("validate")
        .edge(edge("start", "qa_staging"))
        .edge(judged_edge("qa_staging", "validate", "the run passed"))
        .edge(crate::core::Transition {
            backoff_s: Some(0),
            ..judged_edge(
                "qa_staging",
                "qa_staging",
                "the failure was infrastructural",
            )
        })
        .edge(judged_edge(
            "qa_staging",
            "debug",
            "the failure was a real defect",
        ))
        .edge(edge("debug", "qa_staging"))
        .loop_over(loop_spec("qa", &["qa_staging"], 5, OnExhausted::Escalate))
        .build();

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
    let outcome = drive(&m, &runner, &mut ledger);

    assert_eq!(outcome.status, RunStatus::Done);
    assert_eq!(outcome.terminal_state.as_deref(), Some("validate"));

    assert_eq!(
        ledger.state_cycles(),
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
    let m = machine()
        .entry("start")
        .escalate_to("blocked")
        .edge(edge("start", "qa_staging"))
        .edge(crate::core::Transition {
            backoff_s: Some(0),
            ..edge("qa_staging", "qa_staging")
        })
        .loop_over(loop_spec("qa", &["qa_staging"], 2, OnExhausted::Escalate))
        .build();

    let runner = FakeRunner::default();
    runner.script_worker("start", worker_result(proposal_to("qa_staging", "go")));
    for _ in 0..5 {
        runner.script_worker(
            "qa_staging",
            worker_result(proposal_to("qa_staging", "still flaky")),
        );
    }

    let mut ledger = FakeLedger::default();
    let outcome = drive(&m, &runner, &mut ledger);

    // Escalation is a failed run, not a successful one: the ticket did not
    // go through, and the exit status must say so.
    assert_eq!(outcome.status, RunStatus::Failed);
    assert_eq!(outcome.terminal_state.as_deref(), Some("blocked"));

    // cycle 1 (from `start`), cycle 2 (self-loop) both run; the 3rd would
    // exceed max_cycles=2, so it's escalated instead of spawning a 3rd worker.
    let qa_entries = ledger
        .entered()
        .iter()
        .filter(|(s, _, _)| s == "qa_staging")
        .count();
    assert_eq!(qa_entries, 2);
    assert_eq!(ledger.errors().len(), 1);
}

#[test]
fn max_cycles_exhaustion_aborts_when_configured() {
    let m = machine()
        .entry("start")
        .terminal("done")
        .edge(edge("start", "qa_staging"))
        .edge(crate::core::Transition {
            backoff_s: Some(0),
            ..edge("qa_staging", "qa_staging")
        })
        .loop_over(loop_spec("qa", &["qa_staging"], 1, OnExhausted::Abort))
        .build();

    let runner = FakeRunner::default();
    runner.script_worker("start", worker_result(proposal_to("qa_staging", "go")));
    for _ in 0..3 {
        runner.script_worker(
            "qa_staging",
            worker_result(proposal_to("qa_staging", "still flaky")),
        );
    }

    let mut ledger = FakeLedger::default();
    assert_eq!(drive(&m, &runner, &mut ledger).status, RunStatus::Aborted);
}

/// The bound that `max_cycles` cannot supply. A retry writes no
/// `transition_committed`, so the loop head's cycle counter never advances —
/// which left `:on-fail "retry"` spinning on the dollar budget when a guard
/// failed for a reason no re-run would fix (a `:check` pointed at a command
/// that cannot pass, say).
#[test]
fn retry_is_bounded_by_the_edges_max_attempts() {
    let m = machine()
        .entry("implement")
        .terminal("done")
        .escalate_to("blocked")
        .edge(crate::core::Transition {
            max_attempts: 2,
            // The default, spelled out: this is the path under test.
            on_fail: OnFail::Retry,
            ..checked_edge("implement", "done", "cargo test")
        })
        // The loop that cannot save us: `implement` is its head, and no retry
        // ever increments the cycle it bounds.
        .loop_over(loop_spec("fix", &["implement"], 4, OnExhausted::Escalate))
        .build();

    let runner = FakeRunner::default();
    let checks = FakeChecks::default();
    for _ in 0..5 {
        runner.script_worker(
            "implement",
            worker_result(proposal_to("done", "green this time, honest")),
        );
        checks.script_fail("2 tests failed");
    }

    let mut ledger = FakeLedger::default();
    let outcome = drive_checked(&m, &runner, &checks, &mut ledger);

    assert_eq!(outcome.status, RunStatus::Failed);
    assert_eq!(outcome.terminal_state.as_deref(), Some("blocked"));

    // Two attempts, then escalation — not a third spawn, and not the five the
    // runner was willing to give.
    assert_eq!(ledger.count_of("state_entered"), 2);
    assert_eq!(runner.worker_calls.borrow().len(), 2);
    assert_eq!(ledger.attempts(), vec![1, 2]);
    // The cycle counter stayed at 1 throughout, which is precisely why
    // `max_cycles: 4` never had anything to bite on.
    assert_eq!(ledger.state_cycles(), vec![("implement".to_string(), 1); 2]);

    let errors = ledger.errors();
    assert_eq!(errors.len(), 1, "one fatal, naming the exhausted edge");
    assert!(
        errors[0].contains("max_attempts=2"),
        "the error has to say what bound was hit: {:?}",
        errors[0]
    );
}

/// `max_attempts` bounds retries only. A route is not a second attempt at the
/// same edge, so it keeps running against the loop's `max_cycles` as before.
#[test]
fn max_attempts_does_not_bound_an_on_fail_route() {
    let m = machine()
        .entry("implement")
        .terminal("done")
        .escalate_to("blocked")
        .edge(crate::core::Transition {
            max_attempts: 1,
            on_fail: OnFail::Route("review".into()),
            ..judged_edge("implement", "done", "the plan is done")
        })
        .edge(edge("review", "implement"))
        .loop_over(loop_spec("fix", &["implement"], 2, OnExhausted::Escalate))
        .build();

    let runner = FakeRunner::default();
    for _ in 0..4 {
        runner.script_worker("implement", worker_result(proposal_to("done", "done-ish")));
        runner.script_worker(
            "review",
            worker_result(proposal_to("implement", "findings")),
        );
        runner.script_judge(verdict(false, "not addressed"));
    }

    let mut ledger = FakeLedger::default();
    let outcome = drive(&m, &runner, &mut ledger);

    // `max_cycles: 2` is what stops this, at the second re-entry of the head —
    // `max_attempts: 1` never fires, because a route never retries.
    assert_eq!(outcome.status, RunStatus::Failed);
    assert_eq!(outcome.terminal_state.as_deref(), Some("blocked"));
    let implement_entries = ledger
        .entered()
        .iter()
        .filter(|(s, _, _)| s == "implement")
        .count();
    assert_eq!(implement_entries, 2);
}

#[test]
fn navigator_cap_exceeded_escalates_without_spawning() {
    // An unconditional self-loop, so a navigator choice of `debug` is a valid
    // declared edge and re-enters the same state, exercising the per-state cap.
    let m = machine()
        .entry("debug")
        .escalate_to("blocked")
        .navigator_cap(2)
        .edge(edge("debug", "debug"))
        .build();

    let runner = FakeRunner::default();
    for _ in 0..3 {
        runner.script_worker("debug", worker_result(proposal_blocked("stuck")));
    }
    // Only two navigator calls should ever actually happen (the cap).
    runner.script_navigator(choice("debug"));
    runner.script_navigator(choice("debug"));

    let mut ledger = FakeLedger::default();
    let outcome = drive(&m, &runner, &mut ledger);

    assert_eq!(outcome.status, RunStatus::Failed);
    assert_eq!(outcome.terminal_state.as_deref(), Some("blocked"));
    assert_eq!(runner.navigator_calls.borrow().len(), 2);
    assert_eq!(ledger.count_of("navigator_invoked"), 2);
}

#[test]
fn max_transitions_budget_aborts() {
    let m = machine()
        .entry("ping")
        .terminal("done")
        .budget_transitions(1)
        .edge(edge("ping", "pong"))
        .edge(edge("pong", "ping"))
        .build();

    let runner = FakeRunner::default();
    runner.script_worker("ping", worker_result(proposal_to("pong", "go")));
    runner.script_worker("pong", worker_result(proposal_to("ping", "back")));

    let mut ledger = FakeLedger::default();
    let outcome = drive(&m, &runner, &mut ledger);

    assert_eq!(outcome.status, RunStatus::Aborted);
    assert_eq!(outcome.totals.transitions, 1);
}

#[test]
fn budget_usd_aborts() {
    let m = machine()
        .entry("spend")
        .terminal("done")
        .budget_usd(1.0)
        .edge(edge("spend", "more"))
        .edge(edge("more", "spend"))
        .build();

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
    let outcome = drive(&m, &runner, &mut ledger);

    assert_eq!(outcome.status, RunStatus::Aborted);
    assert!(outcome.totals.usage.cost_usd > 1.0);
}

#[test]
fn wallclock_budget_aborts() {
    let m = machine()
        .entry("spend")
        .terminal("done")
        .budget_wallclock_s(1)
        .build();

    let runner = FakeRunner::default();
    // Should never even be reached: the wallclock check happens before spawn.
    runner.script_worker("spend", worker_result(proposal_to("done", "go")));

    let mut ledger = FakeLedger::default();
    let (outcome, _) = drive_with(Rig::new(&m, &runner, &mut ledger).started_s_ago(10));

    assert_eq!(outcome.status, RunStatus::Aborted);
    assert_eq!(runner.worker_calls.borrow().len(), 0);
}

// ── navigator: blocked proposal, and navigator choosing escalate ────────

#[test]
fn blocked_proposal_routes_through_navigator_to_chosen_state() {
    // The navigator's chosen target still needs to be a declared edge — the
    // graph is not waived just because the Navigator picked it.
    let m = machine()
        .entry("debug")
        .terminal("qa_staging")
        .edge(edge("debug", "qa_staging"))
        .build();

    let runner = FakeRunner::default();
    runner.script_worker("debug", worker_result(proposal_blocked("need more data")));
    runner.script_navigator(choice("qa_staging"));

    let mut ledger = FakeLedger::default();
    let outcome = drive(&m, &runner, &mut ledger);

    assert_eq!(outcome.status, RunStatus::Done);
    assert_eq!(outcome.terminal_state.as_deref(), Some("qa_staging"));
    assert_eq!(ledger.count_of("navigator_invoked"), 1);
}

#[test]
fn navigator_choosing_escalate_routes_to_escalation_state() {
    let m = machine().entry("debug").escalate_to("blocked").build();

    let runner = FakeRunner::default();
    runner.script_worker("debug", worker_result(proposal_blocked("truly stuck")));
    runner.script_navigator(choice_with_addendum(
        "blocked",
        "needs a human; nothing reachable fits",
    ));

    let mut ledger = FakeLedger::default();
    let outcome = drive(&m, &runner, &mut ledger);

    assert_eq!(outcome.status, RunStatus::Failed);
    assert_eq!(outcome.terminal_state.as_deref(), Some("blocked"));
    // Escalation is a harness override: no guard_checked for this commit.
    assert!(ledger.guards().is_empty());
}

/// The same ending, reached the way the real runner reaches it.
///
/// [`crate::core::ESCALATE`] is what `run_navigator` actually returns — both
/// when the Navigator picks the sentinel deliberately and when its reply was
/// unusable and the harness substituted one. The test above scripts the
/// escalation state's own name, which the engine resolves one branch earlier,
/// so it never exercised this path: the sentinel matches no declared edge, and
/// the arm that catches an unroutable target records a *fatal error*. That put
/// a line reading like an internal invariant breach into the ledger of every
/// run that ever escalated — and gave `report::last_fatal` a false cause of
/// death to quote in `loop recap` for any run that later ended some other way.
#[test]
fn the_escalate_sentinel_is_a_decision_not_a_fatal_error() {
    let m = machine()
        .entry("debug")
        .terminal("done")
        .escalate_to("blocked")
        .edge(edge("debug", "done"))
        .build();

    let runner = FakeRunner::default();
    runner.script_worker("debug", worker_result(proposal_blocked("truly stuck")));
    runner.script_navigator(choice(crate::core::ESCALATE));

    let mut ledger = FakeLedger::default();
    let outcome = drive(&m, &runner, &mut ledger);

    assert_eq!(outcome.status, RunStatus::Failed);
    assert_eq!(outcome.terminal_state.as_deref(), Some("blocked"));
    assert_eq!(
        ledger.count_of("error"),
        0,
        "escalating is an expected routing outcome, not an error: {:?}",
        ledger.kinds()
    );
    // And it is still an override rather than an edge — no guard tiers ran.
    assert!(ledger.guards().is_empty());
}

/// Reaching the escalation terminal is a failed run, not a successful one.
/// `blocked` is where an exhausted loop, a capped Navigator, or a worker that
/// cannot route itself ends up — reporting `done` there would tell a human, or
/// a CI wrapper reading the exit status, that the ticket went through.
#[test]
fn reaching_the_escalation_terminal_reports_failed_not_done() {
    let m = machine()
        .entry("work")
        .terminal("done")
        .escalate_to("blocked")
        .edge(edge("work", "done"))
        .build();

    let runner = FakeRunner::default();
    runner.script_worker("work", worker_result(proposal_blocked("cannot proceed")));
    runner.script_navigator(choice_with_addendum("blocked", "needs a human"));

    let mut ledger = FakeLedger::default();
    let outcome = drive(&m, &runner, &mut ledger);

    assert_eq!(outcome.terminal_state.as_deref(), Some("blocked"));
    assert_eq!(outcome.status, RunStatus::Failed);
}

/// A run that stopped without reaching a terminal — a blown budget, an
/// `on-fail: abort` — records `terminal_state: null`, and re-reading that
/// ledger has to give back the same answer the process that wrote it did.
///
/// It did not. `step` rebuilt the outcome out of the *fold*, whose `current`
/// still points at the last state it saw, so a resumed `loop run` reported a
/// terminal state for a run that recorded none. Two answers to "how did this
/// end", separated by one process boundary.
#[test]
fn an_already_finished_ledger_reports_the_terminal_it_actually_recorded() {
    use crate::core::fixtures::{self, EventExt};

    let m = machine()
        .entry("implement")
        .terminal("done")
        .edge(edge("implement", "done"))
        .build();

    let runner = FakeRunner::default();
    let mut ledger = FakeLedger::holding(vec![
        fixtures::started("T-1"),
        fixtures::entered("implement", 1, 1),
        fixtures::finished(RunStatus::Aborted, "unused").no_terminal(),
    ]);

    let outcome = drive(&m, &runner, &mut ledger);

    assert_eq!(outcome.status, RunStatus::Aborted);
    assert_eq!(
        outcome.terminal_state, None,
        "the ledger recorded no terminal; `implement` is merely where it stopped"
    );
}

// ── crashed vs stuck workers ─────────────────────────────────────────────

/// A worker whose *process* died must be re-entered, not escalated. The two
/// failures look alike at the proposal layer (both yield no transition) but
/// mean opposite things: a stuck worker asked for routing, a crashed one never
/// got to decide. Escalating on a crash throws away a run a retry would have
/// finished.
#[test]
fn a_crashed_worker_is_re_entered_not_escalated() {
    let m = machine()
        .entry("test")
        .terminal("done")
        .escalate_to("blocked")
        .edge(edge("test", "done"))
        .build();

    let runner = FakeRunner::default();
    runner.script_worker("test", crashed_worker());
    runner.script_worker("test", worker_result(proposal_to("done", "suite green")));

    let mut ledger = FakeLedger::default();
    let outcome = drive(&m, &runner, &mut ledger);

    assert_eq!(outcome.status, RunStatus::Done);
    assert_eq!(outcome.terminal_state.as_deref(), Some("done"));

    // Re-entered with a second attempt, and no navigator was consulted.
    assert_eq!(ledger.attempts(), vec![1, 2]);
    assert_eq!(ledger.count_of("navigator_invoked"), 0);

    // The crash left no `worker_output`, so the ledger tail keeps the shape
    // the fold reads as "crashed mid-flight" — an out-of-process `loop resume`
    // therefore recovers exactly as this in-process retry did.
    assert_eq!(ledger.count_of("worker_output"), 1);
    assert_eq!(ledger.errors().len(), 1);
}

/// ...but a stage that dies every time is a real fault, not a flake. It must
/// stop rather than spin on the budget.
#[test]
fn a_stage_that_always_crashes_escalates_after_the_cap() {
    let m = machine()
        .entry("test")
        .terminal("done")
        .escalate_to("blocked")
        .edge(edge("test", "done"))
        .build();

    let runner = FakeRunner::default();
    for _ in 0..crate::engine::MAX_CRASH_ATTEMPTS + 2 {
        runner.script_worker("test", crashed_worker());
    }

    let mut ledger = FakeLedger::default();
    let outcome = drive(&m, &runner, &mut ledger);

    assert_eq!(outcome.status, RunStatus::Failed);
    assert_eq!(outcome.terminal_state.as_deref(), Some("blocked"));
    assert_eq!(
        ledger.count_of("state_entered"),
        crate::engine::MAX_CRASH_ATTEMPTS as usize
    );
}

// ── what actually reaches the stage ──────────────────────────────────────

/// The Navigator's get-back-on-track note has to survive the ordinary route:
/// through a guarded edge, which puts a `guard_checked` between the
/// `navigator_invoked` and the commit. Reading only the line before the commit
/// meant the note arrived exactly when it could not be used — on the route to
/// the terminal escalation state, which never renders a stage prompt.
#[test]
fn the_navigators_addendum_reaches_the_state_it_routed_into() {
    let m = machine()
        .entry("implement")
        .terminal("done")
        .edge(judged_edge(
            "implement",
            "debug",
            "a real failure to diagnose",
        ))
        .edge(edge("debug", "done"))
        .build();

    let runner = FakeRunner::default();
    runner.script_worker("implement", worker_result(proposal_blocked("I am lost")));
    runner.script_navigator(choice_with_addendum(
        "debug",
        "the migration is half-applied; start there",
    ));
    runner.script_judge(verdict(true, "routing to debug is right"));
    runner.script_worker("debug", worker_result(proposal_to("done", "fixed")));

    let mut ledger = FakeLedger::default();
    let (outcome, contexts) = drive_with(Rig::new(&m, &runner, &mut ledger));

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
/// stage prompt gets told.
#[test]
fn a_re_entry_after_a_crash_is_marked_for_the_stage_prompt() {
    let m = machine()
        .entry("ship")
        .terminal("done")
        .edge(edge("ship", "done"))
        .build();

    let runner = FakeRunner::default();
    runner.script_worker("ship", crashed_worker());
    runner.script_worker("ship", worker_result(proposal_to("done", "pr is up")));

    let mut ledger = FakeLedger::default();
    let (outcome, contexts) = drive_with(Rig::new(&m, &runner, &mut ledger));

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
    let m = machine()
        .entry("implement")
        .terminal("done")
        .edge(edge("implement", "done"))
        .build();

    let runner = FakeRunner::default();
    let mut proposal = proposal_to("done", "wrote the diff");
    proposal.artifacts = vec![
        crate::core::Artifact {
            name: "diff".into(),
            path: "diff.patch".into(),
        },
        crate::core::Artifact {
            name: "notes".into(),
            path: "never-written.md".into(),
        },
    ];
    runner.script_worker("implement", worker_result(proposal));

    let artifacts = RefusingArtifacts {
        refuse: "never-written.md",
    };
    let mut ledger = FakeLedger::default();
    let (outcome, _) = drive_with(Rig::new(&m, &runner, &mut ledger).artifacts(&artifacts));

    assert_eq!(outcome.status, RunStatus::Done);
    assert_eq!(
        ledger.artifact_names(),
        vec![vec!["diff".to_string()]],
        "the good claim still lands"
    );

    let errors = ledger.errors();
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("never-written.md"), "got: {}", errors[0]);
    assert!(errors[0].contains("notes"), "got: {}", errors[0]);
}

// ── the Judge's spend ────────────────────────────────────────────────────

/// Criteria are not free, and a machine that leans on them was spending real
/// money the `:usd` budget could not see. The verdict's usage lands on the
/// guard event, which is what the fold adds up.
#[test]
fn the_judges_usage_lands_on_the_guard_event_and_in_the_totals() {
    let m = machine()
        .entry("implement")
        .terminal("done")
        .edge(judged_edge("implement", "done", "the plan is addressed"))
        .build();

    let runner = FakeRunner::default();
    runner.script_worker(
        "implement",
        worker_result_costing(proposal_to("done", "did it"), 1.0),
    );
    runner.script_judge(Verdict {
        pass: true,
        rationale: "addressed".into(),
        usage: Usage {
            tokens: 500,
            cost_usd: 0.25,
        },
    });

    let mut ledger = FakeLedger::default();
    let outcome = drive(&m, &runner, &mut ledger);

    let usage = ledger.guards()[0].usage;
    assert_eq!(usage.cost_usd, 0.25);
    assert_eq!(usage.tokens, 500);
    assert!(
        (outcome.totals.usage.cost_usd - 1.25).abs() < 1e-9,
        "the run's cost must include the judge: {}",
        outcome.totals.usage.cost_usd
    );
}

/// A judge-heavy machine has to be able to blow the dollar budget on judging
/// alone, or the guardrail is not a guardrail.
#[test]
fn judge_spend_alone_can_exhaust_the_usd_budget() {
    let m = machine()
        .entry("a")
        .terminal("done")
        .budget_usd(0.30)
        .edge(judged_edge("a", "b", "good enough"))
        .edge(judged_edge("b", "done", "good enough"))
        .build();

    let runner = FakeRunner::default();
    // Free workers; every dollar on this run comes from the criteria tier.
    runner.script_worker("a", worker_result_costing(proposal_to("b", "on"), 0.0));
    runner.script_worker("b", worker_result_costing(proposal_to("done", "on"), 0.0));
    for _ in 0..2 {
        runner.script_judge(Verdict {
            pass: true,
            rationale: "fine".into(),
            usage: Usage {
                tokens: 100,
                cost_usd: 0.4,
            },
        });
    }

    let mut ledger = FakeLedger::default();
    let outcome = drive(&m, &runner, &mut ledger);

    assert_eq!(outcome.status, RunStatus::Aborted);
    let errors = ledger.errors();
    let last = errors.last().expect("a budget error");
    assert!(last.contains("budget_usd exceeded"), "got: {last}");
}
