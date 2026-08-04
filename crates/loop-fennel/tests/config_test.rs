//! `config.fnl` overlay: an empty file keeps every default; a partial file
//! overrides only what it names.

mod common;

use std::fs;

use loop_core::{Config, Paths, Thinking};

fn paths_with_config_dir(dir: &std::path::Path) -> Paths {
    Paths {
        config_dir: dir.to_path_buf(),
        state_dir: dir.join("state"),
        project_dir: dir.join("project"),
    }
}

#[test]
fn missing_config_file_keeps_defaults() {
    let vm = common::vm();
    let tmp = tempfile::tempdir().unwrap();
    let paths = paths_with_config_dir(tmp.path());

    let got = vm.load_config(paths.clone()).expect("load_config");
    let want = Config::defaults(paths);

    assert_eq!(
        serde_json::to_value(&got).unwrap(),
        serde_json::to_value(&want).unwrap()
    );
}

#[test]
fn empty_config_file_keeps_every_default() {
    let vm = common::vm();
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("config.fnl"), "").unwrap();
    let paths = paths_with_config_dir(tmp.path());

    let got = vm.load_config(paths.clone()).expect("load_config");
    let want = Config::defaults(paths);

    assert_eq!(
        serde_json::to_value(&got).unwrap(),
        serde_json::to_value(&want).unwrap()
    );
}

#[test]
fn partial_config_overrides_only_what_it_names() {
    let vm = common::vm();
    let tmp = tempfile::tempdir().unwrap();
    fs::copy(
        common::fixture("config_partial.fnl"),
        tmp.path().join("config.fnl"),
    )
    .unwrap();
    let paths = paths_with_config_dir(tmp.path());

    let defaults = Config::defaults(paths.clone());
    let got = vm.load_config(paths).expect("load_config");

    // Named overrides took effect.
    assert_eq!(got.worker.model, "claude-opus-5");
    assert_eq!(got.budgets.usd, Some(25.0));

    // Everything else — including the *rest* of the worker ModelSpec — is
    // untouched.
    assert_eq!(got.worker.provider, defaults.worker.provider);
    assert_eq!(got.worker.thinking, defaults.worker.thinking);
    assert_eq!(got.worker.thinking, Thinking::Medium);
    assert_eq!(
        serde_json::to_value(&got.judge).unwrap(),
        serde_json::to_value(&defaults.judge).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&got.navigator).unwrap(),
        serde_json::to_value(&defaults.navigator).unwrap()
    );
    assert_eq!(
        got.navigator_max_invocations,
        defaults.navigator_max_invocations
    );
    assert_eq!(got.default_skills, defaults.default_skills);
    assert_eq!(got.pi_extensions, defaults.pi_extensions);
    assert_eq!(got.budgets.wallclock_s, defaults.budgets.wallclock_s);
    assert_eq!(
        got.budgets.max_transitions,
        defaults.budgets.max_transitions
    );
    assert_eq!(got.digest_last_n, defaults.digest_last_n);
}

/// The top-level `:provider` is the base of all three role chains. It used to
/// be read into the `Config` and then consulted by nothing — every `--provider`
/// flag came from a role spec — so setting it alone changed nothing at all.
#[test]
fn top_level_provider_is_the_base_for_every_role() {
    let vm = common::vm();
    let tmp = tempfile::tempdir().unwrap();
    fs::write(
        tmp.path().join("config.fnl"),
        r#"{:provider "openai"
            :worker {:model "gpt-5"}
            :judge {:provider "anthropic" :model "claude-haiku-4-5"}}"#,
    )
    .unwrap();
    let paths = paths_with_config_dir(tmp.path());

    let got = vm.load_config(paths).expect("load_config");

    assert_eq!(got.provider, "openai");
    // A role that names no provider inherits the top-level one...
    assert_eq!(got.worker.provider, "openai");
    assert_eq!(got.navigator.provider, "openai");
    // ...and a role that names its own still wins.
    assert_eq!(got.judge.provider, "anthropic");
}

/// `:context` was accepted, stored, and never read — `"full"` silently got you
/// `"digest"`. It is gone, and a config still carrying it is told so rather
/// than quietly meaning something else.
#[test]
fn context_is_rejected_with_a_pointer_to_what_replaced_it() {
    let vm = common::vm();
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("config.fnl"), r#"{:context "full"}"#).unwrap();
    let paths = paths_with_config_dir(tmp.path());

    let err = vm.load_config(paths).expect_err("`:context` must not load");
    let msg = err.to_string();
    assert!(msg.contains("`:context` was removed"), "got: {msg}");
    assert!(msg.contains("$LEDGER_DIGEST"), "got: {msg}");
}
