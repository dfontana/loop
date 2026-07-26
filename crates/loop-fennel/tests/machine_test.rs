//! Loads the ported PROJ-1487 fixture end-to-end and checks every field of
//! the resulting `Machine`.

mod common;

use loop_core::{OnExhausted, OnFail, PlaybookRef, Thinking, TransitionMode};

#[test]
fn proj1487_machine_loads_completely() {
    let vm = common::vm();
    let config = common::default_config();
    let path = common::fixture("proj1487/machine.fnl");

    let machine = vm.load_machine(&path, &config).expect("load_machine");

    assert_eq!(machine.ticket, "PROJ-1487");
    assert!(machine.task.contains("churn_score"));
    assert!(machine.plan.contains("backfill migration"));

    assert_eq!(machine.qa_cases.len(), 2);
    assert_eq!(machine.qa_cases[0].id, "pipeline");
    assert_eq!(machine.qa_cases[1].id, "contract");

    assert_eq!(machine.entry, "implement");
    assert_eq!(
        machine.terminals,
        ["blocked", "done"].into_iter().map(String::from).collect()
    );
    assert_eq!(machine.escalation_state.as_deref(), Some("blocked"));
    assert_eq!(machine.transition_mode, TransitionMode::Constrained);

    // Defaults are left unresolved (no eager filling of state/playbook layers).
    assert_eq!(
        machine.defaults.model.provider.as_deref(),
        Some("anthropic")
    );
    assert_eq!(
        machine.defaults.model.model.as_deref(),
        Some("claude-sonnet-5")
    );
    assert_eq!(machine.defaults.model.thinking, Some(Thinking::Medium));
    assert!(machine.defaults.skills.is_empty());

    // Budgets: machine values are already tighter than the config defaults
    // (15 usd / 7200s / 60 transitions), so tighten() is a no-op here.
    assert_eq!(machine.budgets.usd, Some(8.0));
    assert_eq!(machine.budgets.wallclock_s, Some(5400));
    assert_eq!(machine.budgets.max_transitions, Some(40));

    // judge/navigator are fully resolved ModelSpecs: provider falls back to
    // config since the machine's `:judge`/`:navigator` tables omit it.
    assert_eq!(machine.judge.provider, "anthropic");
    assert_eq!(machine.judge.model, "claude-haiku-4-5");
    assert_eq!(machine.judge.thinking, Thinking::Low);
    assert_eq!(machine.navigator.provider, "anthropic");
    assert_eq!(machine.navigator.model, "claude-haiku-4-5");
    assert_eq!(machine.navigator.thinking, Thinking::Low);
    assert_eq!(machine.navigator_max_invocations, 5);

    assert_eq!(machine.states.len(), 6);
    let implement = machine.state("implement").expect("implement state");
    assert_eq!(implement.playbook, PlaybookRef::Named("implement".into()));
    assert_eq!(implement.skills, vec!["spark-build"]);
    assert_eq!(implement.model.thinking, Some(Thinking::High));
    assert_eq!(
        implement.description.as_deref(),
        Some("Implement the plan; keep the build green.")
    );

    let qa_staging = machine.state("qa-staging").expect("qa-staging state");
    assert_eq!(qa_staging.skills, vec!["staging-deploy", "spark-run"]);
    assert_eq!(qa_staging.playbook, PlaybookRef::Named("qa".into()));

    assert_eq!(machine.transitions.len(), 10);

    let implement_to_review = machine.edge("implement", "review").expect("edge exists");
    assert!(implement_to_review.criteria.is_some());
    assert_eq!(implement_to_review.on_fail, OnFail::Retry);

    let qa_self_loop = machine.edge("qa-staging", "qa-staging").expect("self loop");
    assert!(qa_self_loop.criteria.is_some());
    assert_eq!(qa_self_loop.backoff_s, Some(30));
    assert_eq!(qa_self_loop.on_fail, OnFail::Abort);

    assert_eq!(machine.loops.len(), 2);
    let qa_loop = machine.loops.iter().find(|l| l.name == "qa").unwrap();
    assert_eq!(qa_loop.states, vec!["qa-staging", "debug"]);
    assert_eq!(qa_loop.max_cycles, 4);
    assert_eq!(qa_loop.on_exhausted, OnExhausted::Escalate);
    assert_eq!(qa_loop.head(), Some(&"qa-staging".to_string()));

    // source hash: 64 lowercase hex chars, and matches a manual sha256 of the file.
    assert_eq!(machine.source_hash.len(), 64);
    assert!(machine.source_hash.chars().all(|c| c.is_ascii_hexdigit()));
    let raw = std::fs::read_to_string(&path).unwrap();
    let expected = {
        use sha2::{Digest, Sha256};
        hex::encode(Sha256::digest(raw.as_bytes()))
    };
    assert_eq!(machine.source_hash, expected);

    assert_eq!(machine.source_path, path);
    assert_eq!(machine.dir, path.parent().unwrap());
}

#[test]
fn missing_entry_with_multiple_states_is_a_clear_error() {
    let vm = common::vm();
    let config = common::default_config();
    let path = common::fixture("missing_entry.fnl");

    let err = vm.load_machine(&path, &config).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("entry"),
        "expected the error to mention `:entry`, got: {msg}"
    );
    assert!(matches!(err, loop_core::CoreError::Machine(_)));
}

/// `:check` accepts a bare command string (the common case) and a
/// `{:cmd .. :timeout-s ..}` table for the rest.
#[test]
fn check_parses_from_both_the_string_and_table_forms() {
    let vm = common::vm();
    let config = common::default_config();
    let path = common::fixture("checks.fnl");

    let machine = vm.load_machine(&path, &config).expect("load_machine");

    let bare = machine.edge("a", "b").expect("edge a->b");
    let check = bare.check.as_ref().expect("a bare-string check");
    assert_eq!(check.cmd, "cargo test");
    assert_eq!(check.timeout_s, loop_core::DEFAULT_CHECK_TIMEOUT_S);

    let table = machine.edge("b", "done").expect("edge b->done");
    let check = table.check.as_ref().expect("a table check");
    assert_eq!(check.cmd, "sbt -batch compile");
    assert_eq!(check.timeout_s, 600);

    assert!(
        machine
            .edge("a", "done")
            .expect("edge a->done")
            .check
            .is_none(),
        "an edge without `:check` carries none"
    );
}

/// An empty command would run `bash -c ''`, exit 0, and read as a passing
/// gate that checks nothing — the most dangerous way for this to fail.
#[test]
fn an_empty_check_command_is_rejected() {
    let vm = common::vm();
    let config = common::default_config();
    let path = common::fixture("check_empty.fnl");

    let err = vm.load_machine(&path, &config).unwrap_err();
    assert!(err.to_string().contains("empty"), "got: {err}");
}

/// A leftover `:when` must fail the load rather than being ignored — silently
/// dropping it would leave the edge with no guard at all.
#[test]
fn leftover_when_guard_is_rejected_with_a_migration_message() {
    let vm = common::vm();
    let config = common::default_config();
    let path = common::fixture("when_removed.fnl");

    let err = vm.load_machine(&path, &config).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains(":criteria"),
        "expected the error to name the replacement, got: {msg}"
    );
    assert!(matches!(err, loop_core::CoreError::Machine(_)));
}

#[test]
fn wrong_lua_type_for_a_string_field_is_rejected() {
    let vm = common::vm();
    let config = common::default_config();
    let path = common::fixture("bad_from_type.fnl");

    let err = vm.load_machine(&path, &config).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("string") && msg.contains("integer"),
        "expected a type-mismatch message, got: {msg}"
    );
    assert!(matches!(err, loop_core::CoreError::Machine(_)));
}

#[test]
fn syntax_error_points_at_fennel_source() {
    let vm = common::vm();
    let config = common::default_config();
    let path = common::fixture("syntax_error.fnl");

    let err = vm.load_machine(&path, &config).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("syntax_error.fnl"),
        "expected the .fnl filename in the error, got: {msg}"
    );
    // A "file:line:col: ..." position — assert some digit shows up right
    // after the filename, i.e. a plausible line number was reported.
    let after_name = msg.split("syntax_error.fnl").nth(1).unwrap_or("");
    assert!(
        after_name.starts_with(':')
            && after_name
                .chars()
                .nth(1)
                .is_some_and(|c| c.is_ascii_digit()),
        "expected `filename:line:...` in the error, got: {msg}"
    );
}
