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
    /// Stands in for pi's session store: `mock-pi` writes a file here per
    /// `--session-id` spawn and requires one per `--session` reopen.
    sessions: PathBuf,
    /// One JSON line per `pi --session` invocation, so `loop session` can be
    /// checked on what it actually handed pi.
    argv_log: PathBuf,
}

impl Fixture {
    fn new(script: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let (project, config, state) = (root.join("proj"), root.join("config"), root.join("state"));
        std::fs::create_dir_all(&project).unwrap();
        let sessions = root.join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let script_path = root.join("script.json");
        std::fs::write(&script_path, script).unwrap();
        let argv_log = root.join("pi-argv.jsonl");
        Self {
            _tmp: tmp,
            project,
            config,
            state,
            script: script_path,
            sessions,
            argv_log,
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
            .env("LOOP_MOCK_SESSIONS", &self.sessions)
            .env("LOOP_MOCK_ARGV_LOG", &self.argv_log)
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
        std::fs::create_dir_all(self.project.join(".loop")).unwrap();
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

    /// Pretend pi has a session under this id.
    fn plant_session(&self, id: &str) {
        std::fs::write(self.sessions.join(format!("{id}.jsonl")), "{}\n").unwrap();
    }

    /// …and pretend it has been garbage-collected.
    fn forget_session(&self, id: &str) {
        std::fs::remove_file(self.sessions.join(format!("{id}.jsonl"))).unwrap();
    }

    /// Every `pi --session` launch, in order.
    fn pi_launches(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(&self.argv_log)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    /// Write a file under the project, creating parents. Relative to the
    /// project root, so `.loop/playbooks/x.md` and `.loop/skills/y.md` both
    /// go through it.
    fn write(&self, rel: &str, body: &str) -> PathBuf {
        let path = self.project.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
        path
    }

    /// Same, under the toolbox root — the loser in every local-first race.
    fn write_toolbox(&self, rel: &str, body: &str) -> PathBuf {
        let path = self.config.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
        path
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

fn note_lines(count: usize) -> Vec<String> {
    (0..count)
        .map(|i| {
            serde_json::json!({
                "ts": format!("2026-07-24T22:00:{i:02}.000Z"),
                "elapsed_s": i,
                "type": "note",
                "text": format!("event {i}"),
            })
            .to_string()
        })
        .collect()
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

/// The handoff protocol, end to end through the real CLI.
///
/// The Worker's decision reaches the harness through a file it writes, so the
/// prompt has to name that file and its valid targets — a protocol block that
/// silently stopped being appended would leave every stage unable to end
/// itself, and every run would limp forward on synthesized blocked proposals.
/// This asserts on the prompt actually handed to pi, not on an intermediate.
#[test]
fn the_rendered_prompt_carries_the_handoff_protocol() {
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

    let run = fx.run(&["run"]);
    assert!(run.status.success(), "run failed: {}", combined(&run));

    let render_dir = fx.state.join("render/TINY-1");
    let prompt_path = render_dir.join("implement-1-1-system.md");
    let prompt = std::fs::read_to_string(&prompt_path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", prompt_path.display()));

    // The path it must write, and the targets it may name.
    let handoff = render_dir.join("implement-1-1-handoff.json");
    assert!(
        prompt.contains(&handoff.display().to_string()),
        "prompt must name the handoff file:\n{prompt}"
    );
    assert!(prompt.contains("Ending this stage"), "{prompt}");
    assert!(prompt.contains("`done`"), "{prompt}");
    assert!(prompt.contains("\"blocked\""), "{prompt}");

    // And the worker really did write it — the proposal on the ledger came
    // out of that file, not out of anything scraped off the event stream.
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&handoff).unwrap()).unwrap();
    assert_eq!(written["to"], "done");

    let events = fx.ledger();
    let proposed = events
        .iter()
        .find(|e| e["type"] == "transition_proposed")
        .expect("a proposal");
    assert_eq!(proposed["to"], "done");
    assert_eq!(proposed["rationale"], "done");

    // Nothing was vendored into the toolbox: loop no longer ships extensions.
    assert!(!fx.config.join("ext").exists());
}

/// A worker that leaves no handoff is a *blocked* worker, not a crashed one.
/// This is the path that used to be "ended its turn without calling
/// transition", and it has to keep behaving identically — synthesize a blocked
/// proposal and let the Navigator route it, rather than failing the run.
#[test]
fn a_worker_that_writes_no_handoff_is_treated_as_blocked() {
    let fx = Fixture::new(
        r#"{"steps":[
          {"match":{"role":"worker"},"summary":"I did some work but never handed off"},
          {"match":{"role":"navigator"},"choice":{"to":"done","entry_prompt":"wrap it up"}},
          {"match":{"role":"judge"},"repeat":true,"verdict":{"pass":true,"rationale":"done"}}
        ]}"#,
    );
    fx.run(&["init", "TINY-1"]);
    fx.machine(TINY_MACHINE);

    let run = fx.run(&["run"]);
    assert!(run.status.success(), "run failed: {}", combined(&run));

    let events = fx.ledger();
    let proposed = events
        .iter()
        .find(|e| e["type"] == "transition_proposed")
        .expect("a synthesized proposal");
    assert_eq!(proposed["blocked"], true);
    assert!(proposed["to"].is_null());

    // Blocked means the Navigator ran; the run still reached its terminal.
    assert!(kinds(&events).contains(&"navigator_invoked"));
    assert_eq!(events.last().unwrap()["status"], "done");
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
        r#"{"ts":"2026-07-24T22:00:00.000Z","elapsed_s":0,"type":"run_started","ticket":"TINY-1","machine_hash":"x","budgets":{"usd":null,"wallclock_s":null,"max_transitions":null}}"#.into(),
        r#"{"ts":"2026-07-24T22:00:01.000Z","elapsed_s":41,"type":"state_entered","state":"implement","cycle":1,"attempt":1,"session_id":null,"model":"claude-sonnet-5","thinking":"medium","skills":[],"mcp":[]}"#.into(),
        r#"{"ts":"2026-07-24T22:00:02.000Z","elapsed_s":95,"type":"worker_ou"#.into(),
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

#[test]
fn logs_default_tail_and_n_override_are_oldest_first() {
    let fx = Fixture::new(r#"{"steps":[]}"#);
    fx.run(&["init", "TINY-1"]);
    fx.write_ledger(&note_lines(25));

    let default = fx.run(&["logs"]);
    assert!(default.status.success(), "{}", combined(&default));
    let default_stdout = stdout(&default);
    let lines: Vec<_> = default_stdout.lines().collect();
    assert_eq!(lines.len(), 20, "{default_stdout}");
    assert!(lines[0].contains("note: event 5"), "{}", lines[0]);
    assert!(lines[19].contains("note: event 24"), "{}", lines[19]);
    assert!(!default_stdout.contains("recent:"));

    let fewer = fx.run(&["logs", "-n", "3"]);
    assert!(fewer.status.success(), "{}", combined(&fewer));
    let fewer_stdout = stdout(&fewer);
    let lines: Vec<_> = fewer_stdout.lines().collect();
    assert_eq!(lines.len(), 3);
    assert!(lines[0].contains("note: event 22"), "{}", lines[0]);
    assert!(lines[2].contains("note: event 24"), "{}", lines[2]);

    let short = fx.run(&["logs", "-n", "50"]);
    assert!(short.status.success(), "{}", combined(&short));
    assert_eq!(stdout(&short).lines().count(), 25);
    assert!(stdout(&short).lines().next().unwrap().contains("event 0"));
}

#[test]
fn logs_raw_is_parseable_and_preserves_repaired_bytes() {
    let fx = Fixture::new(r#"{"steps":[]}"#);
    fx.run(&["init", "TINY-1"]);
    fx.write_ledger(&note_lines(2));
    let expected = std::fs::read(fx.project.join(".loop/ledger.jsonl")).unwrap();

    let path = fx.project.join(".loop/ledger.jsonl");
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    std::io::Write::write_all(
        &mut file,
        br#"{"ts":"2026-07-24T22:00:02.000Z","type":"note","tex"#,
    )
    .unwrap();
    file.sync_data().unwrap();

    let raw = fx.run(&["logs", "--raw"]);
    assert!(raw.status.success(), "{}", combined(&raw));
    assert_eq!(raw.stdout, expected);
    for line in String::from_utf8(raw.stdout).unwrap().lines() {
        serde_json::from_str::<serde_json::Value>(line)
            .unwrap_or_else(|e| panic!("raw output is not JSONL: {e}: {line}"));
    }
    assert_eq!(std::fs::read(path).unwrap(), expected);
}

#[test]
fn logs_rejects_corrupt_interior_content_without_printing_it() {
    let fx = Fixture::new(r#"{"steps":[]}"#);
    fx.run(&["init", "TINY-1"]);
    fx.write_ledger(&note_lines(1));
    let path = fx.project.join(".loop/ledger.jsonl");
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    std::io::Write::write_all(&mut file, b"not-json\n{\"type\":\"note\"").unwrap();
    file.sync_data().unwrap();

    let out = fx.run(&["logs", "--raw"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    assert!(String::from_utf8_lossy(&out.stderr).contains("corrupt ledger line"));
    assert!(std::fs::read_to_string(path).unwrap().contains("not-json"));
}

#[test]
fn logs_raw_rejects_an_explicit_n() {
    let fx = Fixture::new(r#"{"steps":[]}"#);
    let out = fx.run(&["logs", "--raw", "-n", "3"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stdout(&out).is_empty());
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot be used"));
}

#[test]
fn logs_empty_ledger_has_human_message_but_raw_is_empty() {
    let fx = Fixture::new(r#"{"steps":[]}"#);
    fx.run(&["init", "TINY-1"]);

    let human = fx.run(&["logs"]);
    assert!(human.status.success(), "{}", combined(&human));
    assert_eq!(stdout(&human), "no run yet — `loop run` starts one\n");

    let raw = fx.run(&["logs", "--raw"]);
    assert!(raw.status.success(), "{}", combined(&raw));
    assert!(raw.stdout.is_empty());
    assert!(
        raw.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&raw.stderr)
    );
}

#[test]
fn logs_does_not_load_a_missing_or_invalid_machine() {
    let fx = Fixture::new(r#"{"steps":[]}"#);
    fx.run(&["init", "TINY-1"]);
    fx.write_ledger(&note_lines(1));
    let expected = std::fs::read(fx.project.join(".loop/ledger.jsonl")).unwrap();

    std::fs::remove_file(fx.project.join(".loop/machine.fnl")).unwrap();
    let missing = fx.run(&["logs"]);
    assert!(missing.status.success(), "{}", combined(&missing));
    assert!(stdout(&missing).contains("note: event 0"));

    fx.machine("{:ticket \"BROKEN\"\n");
    let invalid = fx.run(&["logs", "--raw"]);
    assert!(invalid.status.success(), "{}", combined(&invalid));
    assert_eq!(invalid.stdout, expected);
}

/// A two-state machine with no toolbox dependencies, for the tests that care
/// about harness mechanics rather than the shipped template's shape.
/// `--json` has to be JSON in every state the command can be in, including the
/// one you hit first. The empty-ledger message used to print before the mode
/// branch, so `loop status --json` on a fresh project handed a parser prose.
#[test]
fn status_json_is_parseable_on_an_empty_ledger() {
    let fx = Fixture::new(r#"{"steps":[]}"#);
    fx.run(&["init", "TINY-1"]);
    fx.machine(TINY_MACHINE);

    let out = fx.run(&["status", "--json"]);
    assert!(out.status.success(), "{}", combined(&out));
    let parsed: serde_json::Value = serde_json::from_str(&stdout(&out))
        .unwrap_or_else(|e| panic!("not JSON: {e}: {}", stdout(&out)));
    assert!(parsed["current"].is_null());
    assert!(parsed["status"].is_null());
    assert_eq!(parsed["totals"]["transitions"], 0);

    // The human view still says something a human wants to read.
    let human = fx.run(&["status"]);
    assert!(stdout(&human).contains("no run yet"), "{}", stdout(&human));
}

/// The time budget bounds the *run*, not the process. A resumed run used to
/// get a brand-new wallclock allowance, so an hour-long run could be resumed
/// into a second hour under a one-hour budget, indefinitely.
#[test]
fn a_resumed_run_keeps_the_wallclock_it_had_already_spent() {
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

    // An interrupted run that had already burned an hour.
    fx.write_ledger(&[
        r#"{"ts":"2026-07-24T22:00:00.000Z","elapsed_s":0,"type":"run_started","ticket":"TINY-1","machine_hash":"x","budgets":{"usd":null,"wallclock_s":null,"max_transitions":null}}"#.into(),
        r#"{"ts":"2026-07-24T23:00:00.000Z","elapsed_s":3600,"type":"state_entered","state":"implement","cycle":1,"attempt":1,"session_id":null,"model":"claude-sonnet-5","thinking":"medium","skills":[],"mcp":[]}"#.into(),
    ]);

    // `status` reports the run's clock, not zero, before anything resumes.
    let status = fx.run(&["status", "--json"]);
    let parsed: serde_json::Value = serde_json::from_str(&stdout(&status)).unwrap();
    assert_eq!(parsed["totals"]["wallclock_s"], 3600);

    let resume = fx.run(&["resume"]);
    assert!(resume.status.success(), "{}", combined(&resume));

    let events = fx.ledger();
    // Everything the resumed process appended — the two pre-existing lines are
    // the fixture's own — carries the clock forward instead of restarting it.
    for e in events.iter().skip(2) {
        assert!(
            e["elapsed_s"].as_u64().unwrap() >= 3600,
            "the resumed run must keep counting from what it had spent: {e}"
        );
    }
    assert_eq!(events.last().unwrap()["status"], "done");
    assert!(
        events.last().unwrap()["totals"]["wallclock_s"]
            .as_u64()
            .unwrap()
            >= 3600
    );
}

// ── `loop session` ────────────────────────────────────────────────────────────
//
// The interactive picker itself is unit-tested in `session_picker`, which owns
// every decision it makes — candidate construction, the exact state prefilter,
// all three `Ctrl+O` modes, fuzzy ranking, and the row→session mapping that
// keeps two identical-looking rows from opening each other's session. Those
// tests need no PTY because the reducer is pure. What can only be checked
// through the real binary is what these cover: that the ledger's recorded id
// reaches pi as `--session` in the project directory, and that every way this
// can go wrong is a loud failure rather than a wrong session.

/// The deterministic id `stage.rs` assigns, mirrored here so a change to that
/// scheme breaks these tests loudly instead of silently reopening nothing.
fn session_id(ticket: &str, state: &str, cycle: u32, attempt: u32) -> String {
    let slug = |s: &str| {
        s.chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect::<String>()
    };
    format!("{}-{}-{cycle}-{attempt}", slug(ticket), slug(state))
}

fn entered_line(ts: &str, state: &str, cycle: u32, attempt: u32, session: Option<&str>) -> String {
    let session = match session {
        Some(s) => format!("\"{s}\""),
        None => "null".to_string(),
    };
    format!(
        r#"{{"ts":"{ts}","elapsed_s":0,"type":"state_entered","state":"{state}","cycle":{cycle},"attempt":{attempt},"session_id":{session},"model":"claude-sonnet-5","thinking":"medium","skills":[],"mcp":[]}}"#
    )
}

fn output_line(ts: &str, state: &str, cycle: u32, summary: &str) -> String {
    format!(
        r#"{{"ts":"{ts}","elapsed_s":0,"type":"worker_output","state":"{state}","cycle":{cycle},"summary":"{summary}","artifacts":[],"usage":{{"tokens":10,"cost_usd":0.02}}}}"#
    )
}

fn run_started_line(ticket: &str) -> String {
    format!(
        r#"{{"ts":"2026-07-26T12:00:00.000Z","elapsed_s":0,"type":"run_started","ticket":"{ticket}","machine_hash":"x","budgets":{{"usd":null,"wallclock_s":null,"max_transitions":null}}}}"#
    )
}

/// The end-to-end shape of the whole feature: a real run records a session id,
/// and `loop session --latest` hands exactly that id to pi as `--session`, in
/// the project directory — without printing the opaque id as the selection.
#[test]
fn session_latest_hands_the_recorded_id_to_pi_as_a_session_reopen() {
    let fx = Fixture::new(
        r#"{"steps":[
          {"match":{"role":"worker"},"repeat":true,"summary":"Added the retry guard.",
           "usage":{"tokens":10,"cost_usd":0.01},
           "transition":{"to":"done","rationale":"done"}},
          {"match":{"role":"judge"},"repeat":true,"verdict":{"pass":true,"rationale":"done"}}
        ]}"#,
    );
    fx.run(&["init", "TINY-1"]);
    fx.machine(TINY_MACHINE);
    let run = fx.run(&["run"]);
    assert!(run.status.success(), "run failed: {}", combined(&run));

    let out = fx.run(&["session", "--latest"]);
    assert!(out.status.success(), "session failed: {}", combined(&out));

    // The selection line names the attempt the way a human recognizes it.
    let opening = stdout(&out)
        .lines()
        .find(|l| l.starts_with("opening "))
        .unwrap_or_default()
        .to_string();
    for part in [
        "TINY-1",
        "implement",
        "cycle 1, attempt 1",
        "finished",
        "Added the retry guard.",
    ] {
        assert!(opening.contains(part), "missing `{part}` in: {opening}");
    }

    let id = session_id("TINY-1", "implement", 1, 1);
    // The opaque id is a key for pi, not the name a human is shown.
    assert!(
        !opening.contains(&id),
        "the selection line must not lead with the session id: {opening}"
    );

    let launches = fx.pi_launches();
    assert_eq!(launches.len(), 1, "{launches:?}");
    assert_eq!(
        launches[0]["argv"],
        serde_json::json!(["--session", id]),
        "loop must reopen with `--session <id>` and nothing else"
    );
    // `--session-id` would create an empty replacement instead of failing.
    assert_ne!(launches[0]["argv"][0], "--session-id");
    assert_eq!(
        std::fs::canonicalize(launches[0]["cwd"].as_str().unwrap()).unwrap(),
        std::fs::canonicalize(&fx.project).unwrap()
    );
}

/// The state positional is an exact prefilter, and `--latest` within it means
/// the newest attempt at *that* state — not the newest attempt overall, and not
/// a state that merely starts with the same letters.
///
/// Also the missing-machine case: there is no `machine.fnl` here at all. Reading
/// what the last Worker did is often exactly what you want while the machine is
/// being rewritten, so it must not be a prerequisite.
#[test]
fn session_state_filter_is_exact_and_works_without_a_machine() {
    let fx = Fixture::new(r#"{"steps":[]}"#);
    for (state, cycle, attempt) in [
        ("implement", 1, 1),
        ("implement-hotfix", 1, 1),
        ("implement", 2, 1),
        ("review", 2, 1),
    ] {
        fx.plant_session(&session_id("PROJ-9", state, cycle, attempt));
    }
    fx.write_ledger(&[
        run_started_line("PROJ-9"),
        entered_line(
            "2026-07-26T12:01:00.000Z",
            "implement",
            1,
            1,
            Some(&session_id("PROJ-9", "implement", 1, 1)),
        ),
        output_line("2026-07-26T12:02:00.000Z", "implement", 1, "first pass"),
        entered_line(
            "2026-07-26T12:03:00.000Z",
            "implement-hotfix",
            1,
            1,
            Some(&session_id("PROJ-9", "implement-hotfix", 1, 1)),
        ),
        output_line(
            "2026-07-26T12:04:00.000Z",
            "implement-hotfix",
            1,
            "patched around it",
        ),
        entered_line(
            "2026-07-26T12:05:00.000Z",
            "implement",
            2,
            1,
            Some(&session_id("PROJ-9", "implement", 2, 1)),
        ),
        output_line("2026-07-26T12:06:00.000Z", "implement", 2, "second pass"),
        entered_line(
            "2026-07-26T12:07:00.000Z",
            "review",
            2,
            1,
            Some(&session_id("PROJ-9", "review", 2, 1)),
        ),
        output_line("2026-07-26T12:08:00.000Z", "review", 2, "clean"),
    ]);
    assert!(
        !fx.project.join(".loop/machine.fnl").exists(),
        "this test is about surviving without one"
    );

    // Filtered: the newest `implement`, not the newest overall, and never the
    // state whose name merely shares a prefix.
    let filtered = fx.run(&["session", "implement", "--latest"]);
    assert!(filtered.status.success(), "{}", combined(&filtered));
    assert!(
        stdout(&filtered).contains("second pass"),
        "{}",
        stdout(&filtered)
    );
    assert_eq!(
        fx.pi_launches()[0]["argv"][1],
        session_id("PROJ-9", "implement", 2, 1)
    );

    // Unfiltered: the last usable candidate in reverse ledger order.
    let unfiltered = fx.run(&["session", "--latest"]);
    assert!(unfiltered.status.success(), "{}", combined(&unfiltered));
    assert!(
        stdout(&unfiltered).contains("review"),
        "{}",
        stdout(&unfiltered)
    );
    assert_eq!(
        fx.pi_launches()[1]["argv"][1],
        session_id("PROJ-9", "review", 2, 1)
    );

    // And the prefix state is reachable only by naming it exactly.
    let hotfix = fx.run(&["session", "implement-hotfix", "--latest"]);
    assert!(hotfix.status.success(), "{}", combined(&hotfix));
    assert_eq!(
        fx.pi_launches()[2]["argv"][1],
        session_id("PROJ-9", "implement-hotfix", 1, 1)
    );
}

/// A piped invocation must never quietly pick a session because there is no
/// human to ask. `Output` gives the child a pipe for stdout, so this is the
/// non-interactive path by construction.
#[test]
fn session_without_latest_refuses_to_choose_non_interactively() {
    let fx = Fixture::new(r#"{"steps":[]}"#);
    let id = session_id("PROJ-9", "implement", 1, 1);
    fx.plant_session(&id);
    fx.write_ledger(&[
        run_started_line("PROJ-9"),
        entered_line("2026-07-26T12:01:00.000Z", "implement", 1, 1, Some(&id)),
        output_line("2026-07-26T12:02:00.000Z", "implement", 1, "did it"),
    ]);

    let out = fx.run(&["session"]);
    assert!(!out.status.success(), "{}", combined(&out));
    let text = combined(&out);
    assert!(text.contains("needs a terminal"), "{text}");
    assert!(text.contains("--latest"), "hint the escape hatch: {text}");
    assert!(
        fx.pi_launches().is_empty(),
        "nothing may be launched: {:?}",
        fx.pi_launches()
    );

    // The same refusal, with the state filter echoed into the suggested command.
    let filtered = fx.run(&["session", "implement"]);
    assert!(!filtered.status.success());
    assert!(
        combined(&filtered).contains("loop session implement --latest"),
        "{}",
        combined(&filtered)
    );
}

/// No usable candidate is a specific error naming the filter and the selection
/// mode — including when the ledger *has* entries but none recorded an id, which
/// is what a pre-session ledger looks like.
#[test]
fn session_with_no_usable_candidate_names_the_filter_and_the_mode() {
    let fx = Fixture::new(r#"{"steps":[]}"#);

    // An empty ledger, picker mode.
    let empty = fx.run(&["session"]);
    assert!(!empty.status.success());
    let text = combined(&empty);
    assert!(text.contains("no Worker session"), "{text}");
    assert!(text.contains("any state"), "{text}");
    assert!(text.contains("All attempts"), "{text}");

    // Entries, but every one sessionless — nothing to reopen.
    fx.write_ledger(&[
        run_started_line("PROJ-9"),
        entered_line("2026-07-26T12:01:00.000Z", "implement", 1, 1, None),
        output_line("2026-07-26T12:02:00.000Z", "implement", 1, "did it"),
    ]);
    let no_ids = fx.run(&["session", "--latest"]);
    assert!(!no_ids.status.success());
    let text = combined(&no_ids);
    assert!(text.contains("no Worker session"), "{text}");
    assert!(text.contains("--latest"), "the mode must be named: {text}");

    // A state that never ran.
    let planted = session_id("PROJ-9", "implement", 1, 1);
    fx.plant_session(&planted);
    fx.write_ledger(&[
        run_started_line("PROJ-9"),
        entered_line(
            "2026-07-26T12:01:00.000Z",
            "implement",
            1,
            1,
            Some(&planted),
        ),
    ]);
    let wrong_state = fx.run(&["session", "deploy", "--latest"]);
    assert!(!wrong_state.status.success());
    let text = combined(&wrong_state);
    assert!(text.contains("state `deploy`"), "{text}");
    assert!(fx.pi_launches().is_empty());
}

/// An attempt with no `worker_output` still opens — that is precisely the
/// transcript you want after a crash — but it warns, on stderr, so the warning
/// survives a piped stdout and never contaminates it.
#[test]
fn session_warns_when_the_chosen_attempt_never_reported() {
    let fx = Fixture::new(r#"{"steps":[]}"#);
    let id = session_id("PROJ-9", "implement", 1, 1);
    fx.plant_session(&id);
    fx.write_ledger(&[
        run_started_line("PROJ-9"),
        entered_line("2026-07-26T12:01:00.000Z", "implement", 1, 1, Some(&id)),
        r#"{"ts":"2026-07-26T12:02:00.000Z","elapsed_s":0,"type":"error","state":"implement","kind":"transient","detail":"executor lost"}"#.into(),
    ]);

    let out = fx.run(&["session", "--latest"]);
    assert!(out.status.success(), "{}", combined(&out));

    let err = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(err.contains("warning"), "{err}");
    assert!(err.contains("no worker_output"), "{err}");
    assert!(
        err.contains("executor lost"),
        "the evidence, verbatim: {err}"
    );
    assert!(
        !stdout(&out).contains("warning"),
        "warnings belong on stderr: {}",
        stdout(&out)
    );
    // Still opened: a crashed attempt's history is the point.
    assert_eq!(fx.pi_launches()[0]["argv"][1], id);
    assert!(stdout(&out).contains("crashed"), "{}", stdout(&out));
}

/// A session pi no longer holds must fail the command. This is the whole reason
/// loop passes `--session` and not `--session-id`: the alternative is pi
/// cheerfully creating an empty session under the same id, which looks exactly
/// like a Worker that did nothing.
#[test]
fn session_propagates_a_failed_pi_launch() {
    let fx = Fixture::new(r#"{"steps":[]}"#);
    let id = session_id("PROJ-9", "implement", 1, 1);
    fx.plant_session(&id);
    fx.write_ledger(&[
        run_started_line("PROJ-9"),
        entered_line("2026-07-26T12:01:00.000Z", "implement", 1, 1, Some(&id)),
        output_line("2026-07-26T12:02:00.000Z", "implement", 1, "did it"),
    ]);

    // It works while the session exists…
    assert!(fx.run(&["session", "--latest"]).status.success());
    // …and fails once it is gone, rather than silently opening an empty one.
    fx.forget_session(&id);
    let out = fx.run(&["session", "--latest"]);
    assert!(!out.status.success(), "{}", combined(&out));
    let text = combined(&out);
    assert!(text.contains("exited 1"), "{text}");
    assert!(
        text.contains(&id),
        "name the session that is missing: {text}"
    );
}

// ── loop preview ─────────────────────────────────────────────────────────────

/// A machine built to make every resolution rule visible at once: a state
/// whose thinking, playbook frontmatter and machine defaults each supply a
/// different layer of the model; skills arriving from two levels; a check with
/// a non-default timeout; both `on-fail` shapes; a backoff; and a loop.
const PREVIEW_MACHINE: &str = r#"
{:ticket "PREV-1"
 :task "Ship the thing."
 :plan "1. Ship it."
 :qa-cases [{:id "smoke" :desc "It starts."}]

 :defaults {:model "machine-default-model" :thinking "low" :skills ["shared"]}
 :budgets {:usd 3 :wallclock-s 90 :max-transitions 7}

 :entry "implement"
 :terminals ["done" "blocked"]
 :escalation-state "blocked"

 :states
 {:implement {:playbook "implement"
              :thinking "max"
              :skills ["local-only"]
              :mcp ["warehouse"]
              :description "Do the work."}
  :verify {:playbook "verify" :description "Check it."}}

 :transitions
 [{:from "implement" :to "verify"
   :check {:cmd "test -f marker.txt" :timeout-s 45}
   :criteria "The work is done."
   :on-fail {:route "implement"}}
  {:from "verify" :to "implement" :backoff-s 30 :criteria "Try again." :on-fail "abort"}
  {:from "verify" :to "done" :criteria "Verified."}]

 :loops
 [{:name "fix" :states ["implement" "verify"] :max-cycles 3 :on-exhausted "escalate"}]}
"#;

/// Scaffold the fixture PREVIEW_MACHINE expects: a local playbook that shadows
/// a toolbox one, a toolbox playbook with no frontmatter model, and a skill at
/// each level.
fn preview_fixture() -> Fixture {
    let fx = Fixture::new(r#"{"steps":[]}"#);
    fx.run(&["init", "PREV-1"]);
    fx.machine(PREVIEW_MACHINE);

    fx.write_toolbox(
        "playbooks/implement.md",
        "---\nname: implement\nmodel: frontmatter-model\n---\nTOOLBOX COPY — must not be used.\n",
    );
    fx.write(
        ".loop/playbooks/implement.md",
        "---\nname: implement\ndescription: Local override.\nmodel: frontmatter-model\n---\n\
         Work on $TICKET_ID, cycle $CYCLE.\n\n$TASK\n\nDigest: $LEDGER_DIGEST\n\nHome is $HOME.\n",
    );
    fx.write_toolbox("playbooks/verify.md", "---\nname: verify\n---\nCheck it.\n");
    fx.write_toolbox("skills/shared.md", "# shared\n");
    fx.write(".loop/skills/local-only.md", "# local-only\n");
    fx
}

/// The whole-machine report: every layered override resolved the way the run
/// would resolve it, local-first winners named by path, and each edge's real
/// gate. This is the command's entire promise — a pre-run answer computed by
/// the run's own resolver, not a second one that can drift from it.
#[test]
fn preview_reports_the_stage_a_run_would_actually_build() {
    let fx = preview_fixture();

    let out = fx.run(&["preview"]);
    assert!(out.status.success(), "preview failed: {}", combined(&out));
    let text = stdout(&out);

    for expected in [
        "PREV-1 — 2 state(s), 3 transition(s), 1 loop(s)",
        "entry             implement",
        "terminals         blocked, done",
        "escalation        blocked",
        // Tightened by the machine, not the config's $15 / 7200s / 60.
        "budgets           $3.00, 1m30s, 7 transition(s)",
        "navigator         anthropic/claude-haiku-4-5:low (max 5 invocation(s))",
        "qa cases          1",
        // Four-layer resolution, merged field by field: `thinking` off the
        // state, `model` off the playbook frontmatter (beating the machine
        // default), `provider` off config.fnl.
        "model           anthropic/frontmatter-model:max",
        // ...and a state with no overrides of its own falls all the way to
        // the machine defaults for both fields.
        "model           anthropic/machine-default-model:low",
        "mcp             warehouse",
        // Edge detail: the check's command and its non-default timeout, both
        // failure shapes, and the backoff.
        "check         test -f marker.txt",
        "timeout       45s",
        "criteria      The work is done.",
        "on fail       route to `implement`",
        "on fail       abort the run",
        "backoff       30s",
        // Loops, heads, limits, exhaustion.
        "fix — head `implement`",
        "max cycles      3",
        "on exhausted    escalate to `blocked`",
        "no problems found",
    ] {
        assert!(text.contains(expected), "missing `{expected}` in:\n{text}");
    }

    // Local-first, both kinds: the report names the file that would actually
    // be loaded, and never the toolbox copy it shadows.
    let local_playbook = fx.project.join(".loop/playbooks/implement.md");
    let local_skill = fx.project.join(".loop/skills/local-only.md");
    let toolbox_skill = fx.config.join("skills/shared.md");
    assert!(
        text.contains(&local_playbook.display().to_string()),
        "{text}"
    );
    assert!(text.contains(&local_skill.display().to_string()), "{text}");
    assert!(
        text.contains(&toolbox_skill.display().to_string()),
        "{text}"
    );
    assert!(
        !text.contains("TOOLBOX COPY"),
        "the shadowed toolbox playbook must not appear: {text}"
    );
}

/// The state form: the exact inputs the Worker gets, plus a render that is
/// labelled as representative rather than passed off as the future prompt.
#[test]
fn preview_of_one_state_shows_the_worker_inputs_and_a_labelled_render() {
    let fx = preview_fixture();

    let out = fx.run(&["preview", "implement"]);
    assert!(out.status.success(), "preview failed: {}", combined(&out));
    let text = stdout(&out);

    for expected in [
        "PREV-1 — state `implement`",
        "reference         `implement` (name, local-first)",
        "description       Local override.",
        "model flag        --model frontmatter-model:max",
        "provider          anthropic",
        "reachable         verify",
        "env               TICKET_ID, STATE, CYCLE, ATTEMPT",
        "session id        PREV-1-implement-1-1",
        // Which loop variables this body actually writes — and which `$NAME`s
        // it writes that substitution will leave alone.
        "referenced        $TICKET_ID, $CYCLE, $TASK, $LEDGER_DIGEST",
        "passed through    $HOME",
        // The render, and the limits printed next to it.
        "representative render",
        "Cycle 1, attempt 1, no previous state, no artifacts, empty ledger digest.",
        "NOT the prompt a future run will send",
        "--- system prompt ---",
        "Work on PREV-1, cycle 1.",
        "Ship the thing.",
        // Unknown names survive rendering, exactly as they will at run time.
        "Home is $HOME.",
        "--- entry message ---",
        "You are entering **implement**, cycle 1.",
        // The stage names an MCP server, so the entry message leads with the
        // connect instruction the Worker would really receive.
        r#"mcp({connect: "warehouse"})"#,
    ] {
        assert!(text.contains(expected), "missing `{expected}` in:\n{text}");
    }

    // The skills the Worker is handed, by the path pi's `--skill` would take.
    assert!(
        text.contains(
            &fx.project
                .join(".loop/skills/local-only.md")
                .display()
                .to_string()
        ),
        "{text}"
    );
    // The prompt file the run *would* write is named, but preview does not
    // write it — the path carries placeholders rather than a cycle.
    assert!(
        text.contains("implement-<cycle>-<attempt>-system.md"),
        "{text}"
    );
}

/// Validation errors are shown, then fatal: preview must not hand back an
/// exit-0 report for a machine that cannot run.
#[test]
fn preview_prints_diagnostics_then_fails_on_a_validation_error() {
    let fx = Fixture::new(r#"{"steps":[]}"#);
    fx.run(&["init", "PREV-2"]);
    fx.machine(&TINY_MACHINE.replace(r#":playbook "implement""#, r#":playbook "nonexistent""#));

    let out = fx.run(&["preview"]);
    assert!(
        !out.status.success(),
        "a machine with errors must not preview clean: {}",
        combined(&out)
    );
    let text = combined(&out);
    // The report still prints — the diagnostics are the point of running it.
    assert!(text.contains("TINY-1 — 1 state(s)"), "{text}");
    // ...reusing `validate`'s own wording, not a preview-only paraphrase.
    assert!(
        text.contains("error  implement: playbook for state `implement` does not resolve"),
        "{text}"
    );
    assert!(text.contains("error: 1 error(s)"), "{text}");

    // And the same diagnostic comes out of `validate`, which is the guarantee
    // that preview is not linting with a weaker rule set.
    let validate = fx.run(&["validate"]);
    assert!(!validate.status.success());
    assert!(
        combined(&validate).contains("playbook for state `implement` does not resolve"),
        "{}",
        combined(&validate)
    );
}

#[test]
fn preview_rejects_an_unknown_state_and_lists_the_real_ones() {
    let fx = preview_fixture();

    let out = fx.run(&["preview", "implementt"]);
    assert!(!out.status.success(), "{}", combined(&out));
    let text = combined(&out);
    assert!(text.contains("no state `implementt`"), "{text}");
    assert!(text.contains("states: implement, verify"), "{text}");
}

/// Both forms are pure reads: identical output run twice, and not one byte
/// written to the ledger, the artifact store, or the render directory under
/// `LOOP_STATE_DIR`.
#[test]
fn preview_is_deterministic_and_creates_no_run_or_render_files() {
    let fx = preview_fixture();

    let first = fx.run(&["preview"]);
    let second = fx.run(&["preview"]);
    assert!(first.status.success(), "{}", combined(&first));
    assert_eq!(
        stdout(&first),
        stdout(&second),
        "the whole-machine preview must be byte-identical across runs"
    );

    let first_state = fx.run(&["preview", "implement"]);
    let second_state = fx.run(&["preview", "implement"]);
    assert!(first_state.status.success(), "{}", combined(&first_state));
    assert_eq!(
        stdout(&first_state),
        stdout(&second_state),
        "the state preview must be byte-identical across runs"
    );

    for must_not_exist in [
        fx.project.join(".loop/ledger.jsonl"),
        fx.project.join(".loop/artifacts"),
        fx.config.join("ext"),
        fx.state.join("render"),
    ] {
        assert!(
            !must_not_exist.exists(),
            "preview created {}",
            must_not_exist.display()
        );
    }
}

// ── `loop recap` ─────────────────────────────────────────────────────────────
//
// The grouping recap is built on — which events belong to which attempt — is
// unit-tested in `report`, where it is a pure function of a slice of events.
// What only the real binary can show is what these cover: that a real run
// produces a report naming every attempt and labelling its evidence by author,
// that a partial run is reported rather than refused, and that a machine edited
// since the run cannot quietly explain it.

fn guard_line(
    ts: &str,
    from: &str,
    to: &str,
    check: &str,
    criteria: &str,
    check_output: Option<&str>,
    judge: Option<&str>,
) -> String {
    // `Option`, matching `entered_line`'s session id, so "no Judge ran" and "an
    // empty rationale" stay distinguishable at the call site.
    let opt = |s: Option<&str>| {
        s.map_or("null".to_string(), |s| {
            serde_json::Value::from(s).to_string()
        })
    };
    format!(
        r#"{{"ts":"{ts}","elapsed_s":0,"type":"guard_checked","from":"{from}","to":"{to}","structural":"pass","check":"{check}","criteria":"{criteria}","check_output":{},"judge_rationale":{},"usage":{{"tokens":10,"cost_usd":0.01}}}}"#,
        opt(check_output),
        opt(judge)
    )
}

fn proposed_line(ts: &str, from: &str, to: &str, rationale: &str) -> String {
    format!(
        r#"{{"ts":"{ts}","elapsed_s":0,"type":"transition_proposed","from":"{from}","to":"{to}","blocked":false,"rationale":"{rationale}","by":"worker"}}"#
    )
}

fn committed_line(ts: &str, from: &str, to: &str, cycle: u32) -> String {
    format!(
        r#"{{"ts":"{ts}","elapsed_s":0,"type":"transition_committed","from":"{from}","to":"{to}","cycle":{cycle}}}"#
    )
}

/// The end-to-end shape of the feature: a real run of the shipped template,
/// recapped. Every attempt gets a section, the Worker's own account is labelled
/// as testimony rather than as fact, and the harness check and the Judge appear
/// beside it as separate evidence.
#[test]
fn recap_of_a_finished_run_names_every_attempt_and_labels_its_evidence() {
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
          {"match":{"role":"judge"},"repeat":true,"verdict":{"pass":true,"rationale":"good"}},

          {"match":{"role":"worker","state":"test"},"summary":"128 passed",
           "usage":{"tokens":1800,"cost_usd":0.04},
           "transition":{"to":"open-pr","rationale":"suite green"}},

          {"match":{"role":"worker","state":"open-pr"},"summary":"opened PR #412",
           "usage":{"tokens":900,"cost_usd":0.02},
           "transition":{"to":"done","rationale":"PR open"}}
        ]}"#,
    );
    fx.run(&["init", "PROJ-9"]);
    let run = fx.run(&["run"]);
    assert!(run.status.success(), "{}", combined(&run));

    let out = fx.run(&["recap"]);
    assert!(out.status.success(), "{}", combined(&out));
    assert!(
        out.stderr.is_empty(),
        "an unmodified machine warns about nothing: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = stdout(&out);

    for expected in [
        "# PROJ-9 — recap",
        "## Run summary",
        "- machine on disk: unchanged since the run started",
        "- outcome: finished — Done at `done`",
        "## Attempt timeline",
        // Both attempts at the state that was routed back into — the second
        // one is the whole reason a recap exists, and the first must not be
        // overwritten by it.
        "### 1. implement — cycle 1, attempt 1",
        "### 3. implement — cycle 2, attempt 1",
        // Evidence, attributed. A Worker summary is testimony; the Judge's
        // verdict and the harness's commit are not.
        "**Worker** — the Worker's own account of what it did",
        "implemented the plan",
        "**Judge** rationale",
        "backfill covers 29 days",
        "**Committed** `implement` → `review` (cycle 1) — the harness's decision.",
        "## Why it ended",
        "- status: Done",
        "- terminal state: done",
        "## Inspecting further",
        "`loop session implement`",
        "loop logs --raw | jq",
    ] {
        assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
    }

    // A failed guard is not omitted just because it produced no commit.
    assert!(text.contains("- criteria: fail"), "{text}");

    // Every `state_entered` in the ledger gets exactly one section.
    let attempts = fx
        .ledger()
        .iter()
        .filter(|e| e["type"] == "state_entered")
        .count();
    assert_eq!(
        text.matches("\n### ").count(),
        attempts,
        "one section per attempt, got {} for {attempts} attempts:\n{text}",
        text.matches("\n### ").count()
    );

    // Deterministic: no LLM, no clock, no machine state. Two runs of the
    // command over the same ledger are the same bytes.
    assert_eq!(stdout(&fx.run(&["recap"])), text);
}

/// A recap of nothing is a request that cannot be served. `status` and `logs`
/// can truthfully answer "nowhere"; a report file containing only headings is
/// worse than an error.
#[test]
fn recap_of_an_empty_ledger_is_an_error() {
    let fx = Fixture::new(r#"{"steps":[]}"#);
    fx.run(&["init", "TINY-1"]);

    let out = fx.run(&["recap"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).is_empty(), "{}", stdout(&out));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("no run to recap"), "{err}");
    assert!(err.contains("ledger.jsonl"), "{err}");
}

/// Completion is not required. An interrupted run is reported to date, with the
/// folded resume point and the last durable event standing in for the
/// `run_finished` that never arrived.
#[test]
fn recap_of_an_interrupted_run_reports_it_to_date() {
    let fx = Fixture::new(r#"{"steps":[]}"#);
    fx.run(&["init", "TINY-1"]);
    fx.machine(TINY_MACHINE);
    fx.write_ledger(&[
        run_started_line("TINY-1"),
        entered_line("2026-07-26T12:00:01Z", "implement", 1, 1, Some("s-1")),
        output_line("2026-07-26T12:05:00Z", "implement", 1, "half of it"),
        proposed_line("2026-07-26T12:05:01Z", "implement", "done", "think so"),
    ]);

    let out = fx.run(&["recap"]);
    assert!(out.status.success(), "{}", combined(&out));
    let text = stdout(&out);
    for expected in [
        "- outcome: unfinished — last at `implement`",
        "unfinished — no `run_finished` in this ledger",
        "- resume point: re-run the guards on `implement` → `done`",
        "- last durable event: 2026-07-26T12:05:01Z",
        "`loop resume` continues from the resume point above.",
        // The attempt itself is still reported in full.
        "### 1. implement — cycle 1, attempt 1",
        "half of it",
    ] {
        assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
    }
}

/// The recap must still work with no machine at all, and must not lose early
/// attempts the way `status`'s recent window does.
#[test]
fn recap_needs_no_machine_and_keeps_the_earliest_attempts() {
    let fx = Fixture::new(r#"{"steps":[]}"#);
    fx.run(&["init", "TINY-1"]);

    let mut lines = vec![run_started_line("TINY-1")];
    for n in 1..=8u32 {
        let ts = format!("2026-07-26T12:{n:02}:00Z");
        lines.push(entered_line(&ts, "implement", n, 1, None));
        lines.push(output_line(&ts, "implement", n, &format!("pass {n}")));
        lines.push(proposed_line(&ts, "implement", "implement", "again"));
        lines.push(guard_line(
            &ts,
            "implement",
            "implement",
            "pass",
            "skip",
            Some("ok"),
            None,
        ));
        lines.push(committed_line(&ts, "implement", "implement", n));
    }
    fx.write_ledger(&lines);
    std::fs::remove_file(fx.project.join(".loop/machine.fnl")).unwrap();

    let out = fx.run(&["recap"]);
    assert!(out.status.success(), "{}", combined(&out));
    let text = stdout(&out);

    // `status` shows the last 12 events; the first attempt here is 35 events
    // back, and is the one a recap is asked for.
    assert!(
        text.contains("pass 1"),
        "the earliest attempt survives:\n{text}"
    );
    assert!(text.contains("pass 8"), "{text}");
    assert_eq!(text.matches("\n### ").count(), 8, "{text}");
    // No machine loaded, so nothing claims to know what a cycle is.
    assert!(text.contains("- machine on disk: not loaded"), "{text}");
    assert!(text.contains("re-entries of every state"), "{text}");
    // No session ids in this ledger, so there is nothing to offer reopening.
    assert!(
        text.contains("No attempt recorded a pi session id"),
        "{text}"
    );
}

/// A machine edited since the run cannot be used to explain the run. The recap
/// says so in the report and on stderr, and still reports everything the ledger
/// holds.
#[test]
fn recap_refuses_to_explain_a_run_with_a_machine_that_has_since_changed() {
    let fx = Fixture::new(
        r#"{"steps":[
          {"match":{"role":"worker"},"repeat":true,"summary":"did the work",
           "usage":{"tokens":10,"cost_usd":0.01},
           "transition":{"to":"done","rationale":"done"}},
          {"match":{"role":"judge"},"repeat":true,"verdict":{"pass":true,"rationale":"ok"}}
        ]}"#,
    );
    fx.run(&["init", "TINY-1"]);
    fx.machine(TINY_MACHINE);
    assert!(fx.run(&["run"]).status.success());

    // The description a reader would otherwise see attached to the attempt.
    let before = stdout(&fx.run(&["recap"]));
    assert!(before.contains("implement — cycle 1, attempt 1 — Do the work."));

    fx.machine(&TINY_MACHINE.replace("Do the work.", "Something else entirely."));
    let out = fx.run(&["recap"]);
    assert!(out.status.success(), "{}", combined(&out));
    let text = stdout(&out);
    let err = String::from_utf8_lossy(&out.stderr);

    assert!(err.contains("has changed since this run started"), "{err}");
    assert!(text.contains("- machine on disk: CHANGED"), "{text}");
    assert!(
        !text.contains("Something else entirely"),
        "a description written after the run must not label it:\n{text}"
    );
    // …but the ledger's own account is untouched.
    assert!(text.contains("did the work"), "{text}");
    assert!(text.contains("- status: Done"), "{text}");
}

/// Guard failure, a retry, a Navigator route, an error, and an aborted finish —
/// the shapes a healthy run never produces and a recap exists to explain.
#[test]
fn recap_reports_guard_failures_navigator_routes_and_the_fatal_error() {
    let fx = Fixture::new(r#"{"steps":[]}"#);
    fx.run(&["init", "TINY-1"]);
    fx.machine(TINY_MACHINE);
    fx.write_ledger(&[
        run_started_line("TINY-1"),
        entered_line("2026-07-26T12:00:01Z", "implement", 1, 1, Some("s-1")),
        output_line("2026-07-26T12:01:00Z", "implement", 1, "first try"),
        proposed_line("2026-07-26T12:01:01Z", "implement", "done", "looks done"),
        guard_line(
            "2026-07-26T12:01:02Z",
            "implement",
            "done",
            "fail",
            "skip",
            Some("cargo test\nFAILED: 3 tests"),
            None,
        ),
        entered_line("2026-07-26T12:02:00Z", "implement", 1, 2, Some("s-2")),
        output_line("2026-07-26T12:03:00Z", "implement", 1, "second try"),
        r#"{"ts":"2026-07-26T12:03:01Z","elapsed_s":0,"type":"transition_proposed","from":"implement","to":null,"blocked":true,"rationale":"I cannot get the suite green","by":"worker"}"#.into(),
        r#"{"ts":"2026-07-26T12:03:02Z","elapsed_s":0,"type":"navigator_invoked","from":"implement","proposal":"blocked: I cannot get the suite green","chosen_to":"blocked","entry_prompt":"summarize what you tried","usage":{"tokens":50,"cost_usd":0.001}}"#.into(),
        r#"{"ts":"2026-07-26T12:03:03Z","elapsed_s":0,"type":"error","state":"implement","kind":"fatal","detail":"escalated: the suite never went green"}"#.into(),
        r#"{"ts":"2026-07-26T12:03:04Z","elapsed_s":0,"type":"run_finished","status":"aborted","terminal_state":null,"totals":{"cost_usd":0.05,"wallclock_s":184,"transitions":0}}"#.into(),
    ]);

    let out = fx.run(&["recap"]);
    assert!(out.status.success(), "{}", combined(&out));
    let text = stdout(&out);

    for expected in [
        // The failing tier, with the harness's own evidence beside it.
        "- check: fail",
        "- criteria: skip (not configured on this edge)",
        "**Check** output — harness evidence, not the Worker's:",
        "FAILED: 3 tests",
        // The retry is its own section, not a footnote on the first attempt.
        "### 2. implement — cycle 1, attempt 2",
        "second try",
        // A blocked proposal names no target, and the Navigator's choice is
        // attributed to the Navigator.
        "blocked — no target proposed",
        "**Navigator** was asked to route out of `implement` and chose `blocked`",
        "summarize what you tried",
        // The guardrail that ended it, repeated where the reader looks for it.
        "- status: Aborted",
        "- terminal state: (none — the run stopped without reaching one)",
        "The last fatal error recorded before it stopped:",
        "escalated: the suite never went green",
    ] {
        assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
    }

    // A recap of a failed run is still a successful report: `loop run` owns
    // the exit code a CI wrapper gates on.
    assert_eq!(out.status.code(), Some(0));
}

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
