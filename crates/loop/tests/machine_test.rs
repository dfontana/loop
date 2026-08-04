//! Loads the ported PROJ-1487 fixture end-to-end and checks every field of
//! the resulting `Machine`.

mod common;

use r#loop::core::{OnExhausted, OnFail, PlaybookRef, Thinking};

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
    assert!(matches!(err, r#loop::core::CoreError::Machine(_)));
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
    assert_eq!(check.timeout_s, r#loop::core::DEFAULT_CHECK_TIMEOUT_S);

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
    assert!(matches!(err, r#loop::core::CoreError::Machine(_)));
}

/// `:transition-mode` chose between two schemas for a tool that no longer
/// exists. Same rule as `:when` and `:context`: a key that has quietly stopped
/// meaning anything must say so, and name what replaced it.
#[test]
fn leftover_transition_mode_is_rejected_with_a_migration_message() {
    let vm = common::vm();
    let config = common::default_config();
    let path = common::fixture("transition_mode_removed.fnl");

    let err = vm.load_machine(&path, &config).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("$LOOP_HANDOFF"),
        "expected the error to name the replacement, got: {msg}"
    );
    assert!(matches!(err, r#loop::core::CoreError::Machine(_)));
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
    assert!(matches!(err, r#loop::core::CoreError::Machine(_)));
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

/// A misspelled key is an error that names it, not a key silently ignored.
///
/// This is the capability the serde loader added over the hand-written walker
/// it replaced: that one read the keys it recognized and dropped the rest, so
/// `:playbok` produced a baffling complaint that `:playbook` was missing —
/// about a line where something spelled almost exactly that is right there.
#[test]
fn a_misspelled_key_is_rejected_by_name() {
    let vm = common::vm();
    let config = common::default_config();
    let path = common::fixture("typo_key.fnl");

    let err = vm.load_machine(&path, &config).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("playbok"),
        "must name the offending key: {msg}"
    );
    assert!(
        msg.contains("playbook"),
        "must list the fields that do exist: {msg}"
    );
}

/// A bad value deep in the file reports *where* it is.
///
/// A machine with fifteen states and one bad `:thinking` is undebuggable if
/// the error says only "unknown variant". The path comes from
/// `serde_path_to_error` wrapping the deserializer, so it stays correct as
/// fields are added without anyone maintaining a context string.
#[test]
fn a_bad_value_reports_its_path_and_the_valid_ones() {
    let vm = common::vm();
    let config = common::default_config();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("machine.fnl");
    std::fs::write(
        &path,
        r#"{:ticket "T" :task "t" :plan "p" :entry "a" :terminals ["done"]
            :states {:a {:playbook "p"} :qa-staging {:playbook "q" :thinking "hihg"}}
            :transitions [{:from "a" :to "done"}]}"#,
    )
    .unwrap();

    let err = vm.load_machine(&path, &config).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("qa-staging") && msg.contains("thinking"),
        "must locate the bad value: {msg}"
    );
    assert!(msg.contains("high"), "must list the valid values: {msg}");
}

/// The top-level `:provider` is the base of all three role chains, not a
/// stored-and-ignored default. It moved here from `config.fnl` when that file
/// was merged into the machine.
#[test]
fn top_level_provider_is_the_base_for_every_role() {
    let vm = common::vm();
    let config = common::default_config();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("machine.fnl");
    std::fs::write(
        &path,
        r#"{:ticket "T" :task "t" :plan "p" :entry "a" :terminals ["done"]
            :provider "openai"
            :worker {:model "gpt-5"}
            :judge {:provider "anthropic" :model "claude-haiku-4-5"}
            :states {:a {:playbook "p"}}
            :transitions [{:from "a" :to "done"}]}"#,
    )
    .unwrap();

    let m = vm.load_machine(&path, &config).expect("load_machine");

    // A role that names no provider inherits the top-level one...
    assert_eq!(m.worker.provider, "openai");
    assert_eq!(m.worker.model, "gpt-5");
    assert_eq!(m.navigator.provider, "openai");
    // ...and a role that names its own still wins.
    assert_eq!(m.judge.provider, "anthropic");
}

/// A machine that names nothing gets loop's built-in floor. With `config.fnl`
/// gone this is the only source of defaults, so it has to actually work.
#[test]
fn an_unopinionated_machine_gets_the_built_in_defaults() {
    let vm = common::vm();
    let config = common::default_config();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("machine.fnl");
    std::fs::write(
        &path,
        r#"{:ticket "T" :task "t" :plan "p" :entry "a" :terminals ["done"]
            :states {:a {:playbook "p"}}
            :transitions [{:from "a" :to "done"}]}"#,
    )
    .unwrap();

    let m = vm.load_machine(&path, &config).expect("load_machine");
    assert_eq!(m.worker, config.worker);
    assert_eq!(m.judge, config.judge);
    assert_eq!(m.navigator, config.navigator);
    assert_eq!(
        m.navigator_max_invocations,
        config.navigator_max_invocations
    );
    assert_eq!(m.digest_last_n, config.digest_last_n);
    assert_eq!(m.pi_extensions, config.pi_extensions);
}

/// Keys that only ever lived in `config.fnl` must say where they went, rather
/// than tripping the generic `deny_unknown_fields` message — the author needs
/// to know the tier still exists under a different name.
#[test]
fn config_only_keys_are_rejected_with_a_migration_message() {
    let vm = common::vm();
    let config = common::default_config();
    let dir = tempfile::tempdir().unwrap();

    for (key, expect) in [
        (r#":context "full""#, "$LEDGER_DIGEST"),
        (r#":default-skills ["jj"]"#, ":defaults {:skills"),
        (r#":default-mcp ["warehouse"]"#, ":defaults {:mcp"),
    ] {
        let path = dir.path().join("machine.fnl");
        std::fs::write(
            &path,
            format!(
                r#"{{:ticket "T" :task "t" :plan "p" :entry "a" :terminals ["done"] {key}
                    :states {{:a {{:playbook "p"}}}}
                    :transitions [{{:from "a" :to "done"}}]}}"#
            ),
        )
        .unwrap();

        let err = vm.load_machine(&path, &config).expect_err("must not load");
        assert!(err.to_string().contains(expect), "for {key}: got {err}");
    }
}

/// The `:check` table was the one wire struct that hand-wrote a `rename`
/// instead of carrying `deny_unknown_fields`, so a misspelled `:timeout-s`
/// deserialized clean and silently took the default — on precisely the key
/// whose job is to stop a slow check being killed early.
#[test]
fn a_misspelled_check_key_is_rejected_by_name() {
    let vm = common::vm();
    let config = common::default_config();
    let path = common::fixture("typo_check_key.fnl");

    let err = vm.load_machine(&path, &config).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("timeut-s"),
        "must name the offending key: {msg}"
    );
    assert!(
        msg.contains("timeout-s"),
        "must list the fields that do exist: {msg}"
    );
}

/// The shipped example's ledger records the hash of the machine beside it, and
/// `loop recap` compares the two to decide whether it may explain the run with
/// the machine on disk. They are only equal if someone keeps them equal — so
/// this fails the moment `examples/proj-1487/machine.fnl` is edited without
/// the ledger being re-stamped, rather than letting the example quietly
/// degrade into demonstrating only the "machine has changed" path.
#[test]
fn the_example_ledger_records_the_hash_of_the_example_machine() {
    let example = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/proj-1487")
        .canonicalize()
        .expect("examples/proj-1487 exists");

    let source = std::fs::read_to_string(example.join("machine.fnl")).expect("machine.fnl");
    let on_disk = {
        use sha2::Digest as _;
        hex::encode(sha2::Sha256::digest(source.as_bytes()))
    };

    let first = std::fs::read_to_string(example.join("ledger.jsonl"))
        .expect("ledger.jsonl")
        .lines()
        .next()
        .expect("a run_started line")
        .to_string();
    let recorded =
        serde_json::from_str::<serde_json::Value>(&first).expect("valid JSON")["machine_hash"]
            .as_str()
            .expect("run_started carries machine_hash")
            .to_string();

    assert_eq!(
        recorded, on_disk,
        "examples/proj-1487/ledger.jsonl records a stale machine_hash — re-stamp it \
         with the sha256 of machine.fnl, or `loop recap` on the example only ever \
         demonstrates the CHANGED path"
    );
}
