//! Drives the real `PiRunner` (worker, judge, navigator, and a crash) against
//! the `mock-pi` fixture binary — the offline, $0 substitute for real `pi`
//! that `crates/mock-pi/src/main.rs` implements.
//!
//! `mock-pi` is a sibling workspace member, not a dependency of this crate
//! (and we're not allowed to touch `Cargo.toml` to make it one), so
//! `CARGO_BIN_EXE_mock-pi` isn't available here. Instead we shell out to
//! `cargo build -p mock-pi` once and locate the resulting binary under the
//! workspace's target directory — just process spawning, same as `PiRunner`
//! itself does for real `pi`.
//!
//! All four scenarios live in **one** `#[test]` function. `run_judge` and
//! `run_navigator` have no per-spawn `env` field on their specs (unlike
//! `WorkerSpec`), so pointing them at a script requires setting
//! `LOOP_MOCK_SCRIPT` in this test process's own environment and letting the
//! child inherit it — `std::env::set_var` is process-global and `unsafe` in
//! this edition, so keeping every scenario sequential in a single test avoids
//! any race with a sibling test mutating the same variable.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use r#loop::core::{
    AgentRunner, Config, JudgeSpec, ModelSpec, NavigatorSpec, Paths, Thinking, WorkerSpec,
};
use r#loop::runner::PiRunner;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent() // crates/
        .unwrap()
        .parent() // workspace root
        .unwrap()
        .to_path_buf()
}

fn mock_pi_bin() -> PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let root = workspace_root();
        let status = std::process::Command::new(env!("CARGO"))
            .args(["build", "-p", "mock-pi"])
            .current_dir(&root)
            .status()
            .expect("failed to run `cargo build -p mock-pi`");
        assert!(status.success(), "cargo build -p mock-pi failed");

        let target_dir = std::env::var("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| root.join("target"));
        let bin = target_dir.join("debug").join("mock-pi");
        assert!(
            bin.exists(),
            "expected a mock-pi binary at {}",
            bin.display()
        );
        bin
    })
    .clone()
}

fn cheap_model() -> ModelSpec {
    ModelSpec {
        provider: "anthropic".into(),
        model: "claude-haiku-4-5".into(),
        thinking: Thinking::Low,
    }
}

fn test_config(pi_bin: &Path) -> Config {
    let paths = Paths {
        project_dir: PathBuf::from("/tmp/loop-e2e-project"),
    };
    let mut cfg = Config::defaults(paths);
    cfg.pi_bin = pi_bin.display().to_string();
    cfg
}

#[test]
fn mock_pi_drives_worker_judge_navigator_and_a_crash() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let script_path = tmp.path().join("script.json");
    std::fs::write(
        &script_path,
        r#"{
            "steps": [
                { "match": {"role": "worker", "state": "implement", "cycle": 1},
                  "summary": "implemented the thing",
                  "transition": {"to": "review", "rationale": "plan items done"},
                  "usage": {"tokens": 100, "cost_usd": 0.05} },
                { "match": {"role": "worker", "state": "debug"}, "exit": "crash" },
                { "match": {"role": "judge"},
                  "verdict": {"pass": true, "rationale": "evidence checks out"} },
                { "match": {"role": "navigator"},
                  "choice": {"to": "qa-staging", "entry_prompt": "re-run QA"} }
            ]
        }"#,
    )
    .unwrap();

    let bin = mock_pi_bin();
    let cfg = test_config(&bin);
    let runner = PiRunner::new(&cfg);

    // Every spawn in this test inherits this from the process environment;
    // see the module doc for why that's safe here.
    // SAFETY: this whole test is single-threaded end to end (one #[test] fn,
    // no spawned threads), and no other test in this binary touches
    // LOOP_MOCK_SCRIPT, so there is no concurrent access to race.
    unsafe {
        std::env::set_var("LOOP_MOCK_SCRIPT", &script_path);
    }

    // ---- worker -----------------------------------------------------
    let worker_spec = WorkerSpec {
        state: "implement".into(),
        cycle: 1,
        attempt: 1,
        model: cheap_model(),
        skill_paths: vec![],
        system_prompt_path: tmp.path().join("stage-prompt.md"),
        entry_message: "Entering implement, cycle 1".into(),
        mcp: vec![],
        handoff_path: tmp.path().join("implement-1-1-handoff.json"),
        cwd: tmp.path().to_path_buf(),
        session_id: Some("PROJ-1-implement-1".into()),
        env: vec![],
    };

    let worker_result = runner.run_worker(&worker_spec).expect("worker spawn ok");
    assert!(worker_result.exit_ok);
    assert_eq!(worker_result.summary, "implemented the thing");
    assert_eq!(worker_result.usage.tokens, 100);
    assert!((worker_result.usage.cost_usd - 0.05).abs() < 1e-9);
    let proposal = worker_result.proposal.expect("worker proposed");
    assert_eq!(proposal.to.as_deref(), Some("review"));
    assert_eq!(proposal.rationale, "plan items done");

    // ---- worker crash -------------------------------------------------
    let crash_spec = WorkerSpec {
        state: "debug".into(),
        cycle: 1,
        attempt: 1,
        session_id: Some("PROJ-1-debug-1".into()),
        handoff_path: tmp.path().join("debug-1-1-handoff.json"),
        ..worker_spec.clone()
    };
    let crash_result = runner
        .run_worker(&crash_spec)
        .expect("a crashed spawn is not a hard error at this layer");
    assert!(!crash_result.exit_ok, "crash script must exit non-zero");
    assert!(crash_result.proposal.is_none());

    // ---- judge ----------------------------------------------------------
    let judge_spec = JudgeSpec {
        criteria: "All checklist items must be present.".into(),
        worker_digest: "Added churn_score column; build green.".into(),
        artifact_paths: vec![],
        check_output: None,
        model: cheap_model(),
        cwd: tmp.path().to_path_buf(),
    };
    let verdict = runner.run_judge(&judge_spec).expect("judge spawn ok");
    assert!(verdict.pass);
    assert_eq!(verdict.rationale, "evidence checks out");

    // ---- navigator --------------------------------------------------
    let navigator_spec = NavigatorSpec {
        graph_summary: "implement -> review -> qa-staging -> done".into(),
        ledger_digest: "worker blocked at review".into(),
        from: "review".into(),
        proposal: None,
        reachable: vec!["qa-staging".into(), "escalate".into()],
        model: cheap_model(),
        cwd: tmp.path().to_path_buf(),
    };
    let choice = runner
        .run_navigator(&navigator_spec)
        .expect("navigator spawn ok");
    assert_eq!(choice.to, "qa-staging");
    assert_eq!(choice.entry_prompt.as_deref(), Some("re-run QA"));

    // ---- judge with no matching step: must fail closed, never pass ------
    // A second script (empty, no default) simulates an "unavailable grader".
    // Kept in this same test function — not a separate #[test] — because
    // both mutate the process-global LOOP_MOCK_SCRIPT var; two #[test] fns
    // doing that would race under cargo's default parallel test threads.
    let empty_script_path = tmp.path().join("empty-script.json");
    std::fs::write(&empty_script_path, r#"{"steps": []}"#).unwrap();
    unsafe {
        std::env::set_var("LOOP_MOCK_SCRIPT", &empty_script_path);
    }
    let unavailable_judge_spec = JudgeSpec {
        criteria: "must be great".into(),
        worker_digest: "did stuff".into(),
        artifact_paths: vec![],
        check_output: None,
        model: cheap_model(),
        cwd: tmp.path().to_path_buf(),
    };
    let verdict = runner
        .run_judge(&unavailable_judge_spec)
        .expect("judge spawn ok");
    assert!(!verdict.pass, "a judge with no verdict must never pass");

    // SAFETY: see above — still single-threaded, cleaning up after ourselves.
    unsafe {
        std::env::remove_var("LOOP_MOCK_SCRIPT");
    }
}
