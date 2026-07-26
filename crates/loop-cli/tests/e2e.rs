//! End-to-end tests: the real `loop` binary, driving the real engine, ledger,
//! Fennel VM and toolbox, against `mock-pi` instead of a live agent.
//!
//! These are the tests that catch what unit tests structurally cannot — every
//! bug found while wiring wave 1 together (a torn ledger line becoming
//! unreadable once appended past, a crashed worker escalating instead of
//! retrying) was a seam between two crates that each passed their own suite.
//!
//! Hermetic: `LOOP_CONFIG_DIR` / `LOOP_STATE_DIR` point at tempdirs, and
//! `LOOP_PI_BIN` at the fixture binary. No network, no API key, no `~`.

use std::path::PathBuf;
use std::process::{Command, Output};

/// `mock-pi` is a sibling workspace member rather than a dependency, so cargo
/// exports no `CARGO_BIN_EXE_` for it; it sits beside the binary under test.
fn mock_pi() -> PathBuf {
    let loop_bin = PathBuf::from(env!("CARGO_BIN_EXE_loop"));
    let path = loop_bin.with_file_name("mock-pi");
    assert!(
        path.exists(),
        "mock-pi not built at {}; run `cargo build --workspace`",
        path.display()
    );
    path
}

struct Fixture {
    _tmp: tempfile::TempDir,
    project: PathBuf,
    config: PathBuf,
    state: PathBuf,
    script: PathBuf,
}

impl Fixture {
    fn new(script: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let (project, config, state) = (root.join("proj"), root.join("config"), root.join("state"));
        std::fs::create_dir_all(&project).unwrap();
        let script_path = root.join("script.json");
        std::fs::write(&script_path, script).unwrap();
        Self {
            _tmp: tmp,
            project,
            config,
            state,
            script: script_path,
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_loop"))
            .args(args)
            .current_dir(&self.project)
            .env("LOOP_CONFIG_DIR", &self.config)
            .env("LOOP_STATE_DIR", &self.state)
            .env("LOOP_PI_BIN", mock_pi())
            .env("LOOP_MOCK_SCRIPT", &self.script)
            .output()
            .expect("failed to spawn loop")
    }

    fn ledger(&self) -> Vec<serde_json::Value> {
        let path = self.project.join(".loop/ledger.jsonl");
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    fn write_ledger(&self, lines: &[String]) {
        std::fs::write(
            self.project.join(".loop/ledger.jsonl"),
            lines.join("\n") + "\n",
        )
        .unwrap();
    }

    fn machine(&self, body: &str) {
        std::fs::create_dir_all(self.project.join(".loop")).unwrap();
        std::fs::write(self.project.join(".loop/machine.fnl"), body).unwrap();
    }
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}

fn combined(o: &Output) -> String {
    format!("{}{}", stdout(o), String::from_utf8_lossy(&o.stderr))
}

fn kinds(events: &[serde_json::Value]) -> Vec<&str> {
    events.iter().filter_map(|e| e["type"].as_str()).collect()
}

/// The shipped `standard-ticket` template, driven to `done` — including a
/// Judge rejecting the review and routing back to `implement` for a second
/// cycle. This is the walkthrough in examples/, minus the pipeline stages.
#[test]
fn full_run_of_the_shipped_template_reaches_done() {
    let fx = Fixture::new(
        r#"{"steps":[
          {"match":{"role":"worker","state":"implement"},"summary":"implemented the plan",
           "usage":{"tokens":4200,"cost_usd":0.11},
           "transition":{"to":"review","rationale":"plan items done"}},
          {"match":{"role":"judge"},"verdict":{"pass":true,"rationale":"covers the checklist"}},

          {"match":{"role":"worker","state":"review"},"summary":"found a defect",
           "usage":{"tokens":3100,"cost_usd":0.08},
           "transition":{"to":"test","rationale":"reviewed"}},
          {"match":{"role":"judge"},"verdict":{"pass":false,"rationale":"backfill covers 29 days"}},

          {"match":{"role":"worker","state":"implement"},"summary":"fixed the window",
           "usage":{"tokens":2600,"cost_usd":0.07},
           "transition":{"to":"review","rationale":"fixed"}},
          {"match":{"role":"judge"},"verdict":{"pass":true,"rationale":"now 30 days"}},

          {"match":{"role":"worker","state":"review"},"summary":"clean",
           "usage":{"tokens":2900,"cost_usd":0.07},
           "transition":{"to":"test","rationale":"clean"}},
          {"match":{"role":"judge"},"verdict":{"pass":true,"rationale":"no blocking findings"}},

          {"match":{"role":"worker","state":"test"},"summary":"128 passed",
           "usage":{"tokens":1800,"cost_usd":0.04},
           "transition":{"to":"open-pr","rationale":"suite green"}},
          {"match":{"role":"judge"},"verdict":{"pass":true,"rationale":"suite output present"}},

          {"match":{"role":"worker","state":"open-pr"},"summary":"opened PR #412",
           "usage":{"tokens":900,"cost_usd":0.02},
           "transition":{"to":"done","rationale":"PR open"}},
          {"match":{"role":"judge"},"verdict":{"pass":true,"rationale":"PR exists"}}
        ]}"#,
    );

    let init = fx.run(&["init", "DEMO-1"]);
    assert!(init.status.success(), "init failed: {}", combined(&init));

    // The template it just scaffolded must lint clean — a template that fails
    // its own validator is worse than no template.
    let validate = fx.run(&["validate"]);
    assert!(
        validate.status.success(),
        "validate failed: {}",
        combined(&validate)
    );

    let run = fx.run(&["run"]);
    assert!(run.status.success(), "run failed: {}", combined(&run));
    assert!(stdout(&run).contains("Done"), "{}", stdout(&run));

    let events = fx.ledger();
    let ks = kinds(&events);
    assert_eq!(ks.first(), Some(&"run_started"));
    assert_eq!(ks.last(), Some(&"run_finished"));

    let commits: Vec<(&str, &str)> = events
        .iter()
        .filter(|e| e["type"] == "transition_committed")
        .map(|e| (e["from"].as_str().unwrap(), e["to"].as_str().unwrap()))
        .collect();
    assert_eq!(
        commits,
        vec![
            ("implement", "review"),
            ("review", "implement"), // the Judge rejected the review: on_fail route
            ("implement", "review"),
            ("review", "test"),
            ("test", "open-pr"),
            ("open-pr", "done"),
        ]
    );

    let finished = events.last().unwrap();
    assert_eq!(finished["status"], "done");
    assert_eq!(finished["terminal_state"], "done");

    // `implement` is the loop head and ran twice; status must say so.
    let status = fx.run(&["status"]);
    assert!(
        stdout(&status).contains("implement#2"),
        "{}",
        stdout(&status)
    );
}

/// A stage whose process dies is retried, not escalated — and the retry is
/// invisible in the outcome: the run still reaches `done`.
#[test]
fn a_crashed_stage_is_retried_and_the_run_still_completes() {
    let fx = Fixture::new(
        r#"{"steps":[
          {"match":{"role":"worker"},"exit":"crash"},
          {"match":{"role":"worker"},"repeat":true,"summary":"did the work",
           "usage":{"tokens":10,"cost_usd":0.01},
           "transition":{"to":"done","rationale":"done"}},
          {"match":{"role":"judge"},"repeat":true,"verdict":{"pass":true,"rationale":"done"}}
        ]}"#,
    );
    fx.run(&["init", "TINY-1"]);
    fx.machine(TINY_MACHINE);

    let run = fx.run(&["run"]);
    assert!(run.status.success(), "run failed: {}", combined(&run));

    let events = fx.ledger();
    let entered: Vec<u64> = events
        .iter()
        .filter(|e| e["type"] == "state_entered")
        .map(|e| e["attempt"].as_u64().unwrap())
        .collect();
    assert_eq!(entered, vec![1, 2], "the crashed stage must be re-entered");

    // A crash is transient infrastructure, not a stuck worker: no Navigator.
    assert!(!kinds(&events).contains(&"navigator_invoked"));
    assert_eq!(events.last().unwrap()["status"], "done");
}

/// The crash-resume contract of docs/02-how-it-works.md: a ledger whose last write was torn
/// off mid-JSON is repaired, the interrupted stage is re-entered, and the run
/// finishes. Before the ledger repaired its own tail, `resume` would append
/// one event past the torn line and then be permanently unreadable.
#[test]
fn resume_after_a_torn_write_re_enters_and_completes() {
    let fx = Fixture::new(
        r#"{"steps":[
          {"match":{"role":"worker"},"repeat":true,"summary":"did the work",
           "usage":{"tokens":10,"cost_usd":0.01},
           "transition":{"to":"done","rationale":"done"}},
          {"match":{"role":"judge"},"repeat":true,"verdict":{"pass":true,"rationale":"done"}}
        ]}"#,
    );
    fx.run(&["init", "TINY-1"]);
    fx.machine(TINY_MACHINE);

    // A run killed by SIGKILL partway through writing `worker_output`.
    fx.write_ledger(&[
        r#"{"ts":"2026-07-24T22:00:00.000Z","type":"run_started","ticket":"TINY-1","machine_hash":"x","budgets":{"usd":null,"wallclock_s":null,"max_transitions":null}}"#.into(),
        r#"{"ts":"2026-07-24T22:00:01.000Z","type":"state_entered","state":"implement","cycle":1,"attempt":1,"session_id":null,"model":"claude-sonnet-5","thinking":"medium","skills":[],"mcp":[]}"#.into(),
        r#"{"ts":"2026-07-24T22:00:02.000Z","type":"worker_ou"#.into(),
    ]);

    // `status` must survive the torn tail — you reach for it precisely here.
    let status = fx.run(&["status"]);
    assert!(status.status.success(), "{}", combined(&status));
    assert!(stdout(&status).contains("implement"), "{}", stdout(&status));

    let resume = fx.run(&["resume"]);
    assert!(
        resume.status.success(),
        "resume failed: {}",
        combined(&resume)
    );

    let events = fx.ledger();
    let entered: Vec<u64> = events
        .iter()
        .filter(|e| e["type"] == "state_entered")
        .map(|e| e["attempt"].as_u64().unwrap())
        .collect();
    assert_eq!(entered, vec![1, 2], "the interrupted stage re-enters");
    assert_eq!(events.last().unwrap()["status"], "done");
}

/// Escalation is not success. A run that ends at the escalation terminal must
/// exit non-zero, or `loop run && gh pr merge` merges a ticket that never got
/// past a blocked worker.
#[test]
fn an_escalated_run_exits_non_zero() {
    let fx = Fixture::new(
        r#"{"steps":[
          {"match":{"role":"worker"},"repeat":true,"summary":"stuck",
           "transition":{"blocked":true,"rationale":"cannot proceed"}},
          {"match":{"role":"navigator"},"repeat":true,
           "choice":{"to":"escalate","entry_prompt":"needs a human"}}
        ]}"#,
    );
    fx.run(&["init", "TINY-1"]);
    fx.machine(TINY_MACHINE);

    let run = fx.run(&["run"]);
    assert!(
        !run.status.success(),
        "an escalated run must not report success: {}",
        combined(&run)
    );
    assert!(stdout(&run).contains("Failed"), "{}", stdout(&run));

    let events = fx.ledger();
    let finished = events.last().unwrap();
    assert_eq!(finished["status"], "failed");
    assert_eq!(finished["terminal_state"], "blocked");
}

/// `loop diagram` draws the graph the engine actually walks. The shipped
/// template's back-edges exist only as `:on-fail {:route "implement"}`, so a
/// diagram that drew the `:transitions` list alone would render the template as
/// a straight line — the one thing this command must not do.
#[test]
fn diagram_draws_the_shipped_template_including_its_on_fail_back_edges() {
    let fx = Fixture::new(r#"{"steps":[]}"#);
    fx.run(&["init", "DEMO-2"]);

    let out = fx.run(&["diagram"]);
    assert!(out.status.success(), "diagram failed: {}", combined(&out));
    let mmd = stdout(&out);

    for line in [
        "stateDiagram-v2",
        "title: \"DEMO-2\"",
        "[*] --> implement",
        "implement --> review : judge",
        "review --> test : judge",
        "test --> open_pr : judge",
        "open_pr --> done : judge",
        // The two back-edges, present only as `on_fail` routes.
        "review --> implement : guard fails",
        "test --> implement : guard fails",
        // `open-pr` can't be a bare mermaid id; the alias carries the real name.
        "state \"open-pr\" as open_pr",
        "state \"blocked (escalation)\" as blocked",
        "loop \"fix\": max 4 cycles, then escalate to blocked",
    ] {
        assert!(mmd.contains(line), "missing `{line}` in:\n{mmd}");
    }

    // Nothing but the diagram on stdout — `loop diagram > machine.mmd` has to
    // produce a file a renderer will accept.
    assert!(mmd.starts_with("---\n"), "{mmd}");
    assert!(String::from_utf8_lossy(&out.stderr).is_empty());
}

/// A machine that cannot load must fail loudly at `validate`, naming the file —
/// the whole point of the Fennel error plumbing.
#[test]
fn validate_reports_a_fennel_syntax_error_against_the_source_file() {
    let fx = Fixture::new(r#"{"steps":[]}"#);
    fx.run(&["init", "TINY-1"]);
    fx.machine("{:ticket \"BROKEN\"\n :entry \"implement\"\n"); // unclosed table

    let out = fx.run(&["validate"]);
    assert!(!out.status.success());
    let text = combined(&out);
    assert!(
        text.contains("machine.fnl"),
        "error should name the .fnl source: {text}"
    );
}

/// The check tier, end to end through the real binary: a real `bash`
/// subprocess the harness spawns for itself, whose exit code overrules a
/// worker that claims success. Nothing else in the system can do this — every
/// other signal reaching a guard passed through the worker's session first.
#[test]
fn a_failing_check_overrules_a_worker_that_claims_success() {
    let fx = Fixture::new(
        r#"{"steps":[
          {"match":{"role":"worker","state":"implement"},"summary":"all green, promise",
           "transition":{"to":"done","rationale":"finished"},"repeat":true},
          {"match":{"role":"judge"},"verdict":{"pass":true,"rationale":"looks fine"},"repeat":true}
        ]}"#,
    );
    fx.run(&["init", "TINY-1"]);
    // `marker.txt` does not exist, so the check exits non-zero — the worker's
    // claim and the Judge's blessing both count for nothing.
    fx.machine(&TINY_MACHINE.replace(
        r#":criteria "The work is done.""#,
        r#":check "test -f marker.txt" :criteria "The work is done." :on-fail "abort""#,
    ));

    let run = fx.run(&["run"]);
    assert!(!run.status.success(), "a failed check must fail the run");

    let events = fx.ledger();
    let guard = events
        .iter()
        .find(|e| e["type"] == "guard_checked")
        .expect("a guard_checked line");
    assert_eq!(guard["check"], "fail");
    assert_eq!(
        guard["criteria"], "skip",
        "a failed check must not be appealable to the Judge"
    );

    // And with the file present, the same machine and the same script pass.
    std::fs::write(fx.project.join("marker.txt"), "x").unwrap();
    std::fs::remove_file(fx.project.join(".loop/ledger.jsonl")).unwrap();
    let _ = std::fs::remove_file(fx.script.with_extension("json.consumed.json"));

    let rerun = fx.run(&["run"]);
    assert!(rerun.status.success(), "rerun failed: {}", combined(&rerun));
    let guard = fx
        .ledger()
        .into_iter()
        .find(|e| e["type"] == "guard_checked")
        .expect("a guard_checked line");
    assert_eq!(guard["check"], "pass");
    assert_eq!(guard["criteria"], "pass");
}

/// A two-state machine with no toolbox dependencies, for the tests that care
/// about harness mechanics rather than the shipped template's shape.
const TINY_MACHINE: &str = r#"
{:ticket "TINY-1"
 :task "Make the thing."
 :plan "1. Make it."
 :entry "implement"
 :terminals ["done" "blocked"]
 :escalation-state "blocked"
 :states {:implement {:playbook "implement" :description "Do the work."}}
 :transitions [{:from "implement" :to "done" :criteria "The work is done."}]}
"#;
