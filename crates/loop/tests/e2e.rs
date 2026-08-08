//! End-to-end tests: the real `loop` binary, driving the real engine, ledger,
//! Fennel VM and toolbox, against `mock-pi` instead of a live agent.
//!
//! These are the tests that catch what unit tests structurally cannot — every
//! bug found while wiring wave 1 together (a torn ledger line becoming
//! unreadable once appended past, a crashed worker escalating instead of
//! retrying) was a seam between two of the layers that each passed their own
//! suite.
//!
//! Hermetic for free: everything loop reads or writes lives under the project
//! directory, which is a tempdir per fixture. `LOOP_PI_BIN` points at
//! `LOOP_PI_BIN` at the fixture binary. No network, no API key, no `~`.

use std::path::PathBuf;
use std::process::{Command, Output};

mod common;

use common::mock_pi;
use r#loop::core::fixtures::{self, EventExt};
use r#loop::core::{Event, GuardOutcome, RunStatus, Totals, Usage, sanitize_component};

struct Fixture {
    _tmp: tempfile::TempDir,
    project: PathBuf,
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
        let project = root.join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let sessions = root.join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let script_path = root.join("script.json");
        std::fs::write(&script_path, script).unwrap();
        let argv_log = root.join("pi-argv.jsonl");
        Self {
            _tmp: tmp,
            project,
            script: script_path,
            sessions,
            argv_log,
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_loop"))
            .args(args)
            .current_dir(&self.project)
            .env("LOOP_PI_BIN", mock_pi())
            .env("LOOP_MOCK_SCRIPT", &self.script)
            .env("LOOP_MOCK_SESSIONS", &self.sessions)
            .env("LOOP_MOCK_ARGV_LOG", &self.argv_log)
            .output()
            .expect("failed to spawn loop")
    }

    fn ledger(&self) -> Vec<serde_json::Value> {
        jsonl(&self.project.join(".loop/ledger.jsonl"))
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
        jsonl(&self.argv_log)
    }

    /// `loop init <TICKET>`, then the smallest machine that runs. The three
    /// lines a dozen tests opened with, in the order they have to happen:
    /// `init` refuses to overwrite an existing `machine.fnl`, so the template
    /// is replaced after it, not before.
    fn init_tiny(&self, ticket: &str) {
        self.run(&["init", ticket]);
        self.machine(TINY_MACHINE);
    }

    /// Write a file under the project, creating parents. Relative to the
    /// project root, so `.loop/stage-prompts/x.md` and `.loop/skills/y.md` both
    /// go through it.
    fn write(&self, rel: &str, body: &str) -> PathBuf {
        let path = self.project.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
        path
    }
}

/// A fixture whose mock-pi never has anything to say — for the commands that
/// spawn nothing. Still a *readable* script: `LOOP_MOCK_SCRIPT` naming a file
/// that isn't there is a harness misconfiguration, and mock-pi exits 1 on it.
fn unscripted() -> Fixture {
    Fixture::new(r#"{"steps":[]}"#)
}

/// Every parseable line of a JSONL file, or nothing if it isn't there.
/// The ledger and the argv log are both read this way.
fn jsonl(path: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
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

// ── ledger lines ─────────────────────────────────────────────────────────────
//
// A test that plants a ledger writes it in the wire format, but does not get to
// *decide* the wire format: these build a typed `Event` with the library's own
// fixtures and let the harness's serde impls spell it. They used to be six
// hand-written format strings, ~60 lines of them, and a field added to
// `StateEntered` desynced every one of them silently — the run would write
// ledgers these tests could no longer produce, and nothing would fail.

/// One ledger line: a fixture event, serialized the way the harness writes it.
fn line(event: Event) -> String {
    serde_json::to_string(&event).expect("a fixture event serializes")
}

fn run_started_line(ticket: &str) -> String {
    line(fixtures::started(ticket).at("2026-07-26T12:00:00.000Z"))
}

fn entered_line(ts: &str, state: &str, cycle: u32, attempt: u32, session: Option<&str>) -> String {
    // The `Option` goes straight through: "no session id recorded" and "an
    // empty one" stay distinguishable at the call site, and the setter itself
    // takes the `Option` so this is no longer a match.
    line(
        fixtures::entered(state, cycle, attempt)
            .at(ts)
            .session(session),
    )
}

fn output_line(ts: &str, state: &str, cycle: u32, summary: &str) -> String {
    line(
        fixtures::output(state, cycle)
            .at(ts)
            .summary(summary)
            .usage(10, 0.02),
    )
}

fn proposed_line(ts: &str, from: &str, to: &str, rationale: &str) -> String {
    line(fixtures::proposed(from, to).at(ts).rationale(rationale))
}

fn guard_line(
    ts: &str,
    from: &str,
    to: &str,
    check: GuardOutcome,
    criteria: GuardOutcome,
    check_output: Option<&str>,
    judge: Option<&str>,
) -> String {
    line(
        fixtures::guard_checked(from, to)
            .at(ts)
            .guards(check, criteria)
            .evidence(check_output, judge)
            .usage(10, 0.01),
    )
}

fn committed_line(ts: &str, from: &str, to: &str, cycle: u32) -> String {
    line(fixtures::committed(from, to, cycle).at(ts))
}

/// A ledger being planted, with the clock advancing one minute per event.
///
/// The timestamps in a planted ledger are load-bearing in two tests below and
/// scaffolding in every other — but they were typed out by hand everywhere, and
/// kept monotonic by the author, which is one more thing to get wrong when a
/// line is inserted in the middle. Tests that care about a particular stamp
/// still build their lines explicitly; the rest describe the *shape* of the
/// run and let this assign the clock.
#[derive(Default)]
struct Plant {
    lines: Vec<String>,
    minute: u32,
}

impl Plant {
    /// A ledger that opens with `run_started` for this ticket.
    fn started(ticket: &str) -> Self {
        let mut p = Plant::default();
        p.push(fixtures::started(ticket));
        p
    }

    fn push(&mut self, event: Event) -> &mut Self {
        let ts = format!("2026-07-26T12:{:02}:00.000Z", self.minute);
        self.minute += 1;
        self.lines.push(line(event.at(&ts)));
        self
    }

    /// One finished attempt: `state_entered` under the deterministic session
    /// id `stage.rs` would assign, then the `worker_output` that ended it.
    fn attempt(&mut self, ticket: &str, state: &str, cycle: u32, summary: &str) -> &mut Self {
        let id = session_id(ticket, state, cycle, 1);
        self.push(fixtures::entered(state, cycle, 1).session(Some(&id)));
        self.push(
            fixtures::output(state, cycle)
                .summary(summary)
                .usage(10, 0.02),
        )
    }

    fn lines(&self) -> &[String] {
        &self.lines
    }
}

fn note_lines(count: usize) -> Vec<String> {
    (0..count)
        .map(|i| {
            line(
                fixtures::note(&format!("event {i}"))
                    .at(&format!("2026-07-24T22:00:{i:02}.000Z"))
                    .elapsed(i as u64),
            )
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
    fx.init_tiny("TINY-1");

    let run = fx.run(&["run"]);
    assert!(run.status.success(), "run failed: {}", combined(&run));

    let render_dir = fx.project.join(".loop/run");
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

    // Derived, and marked as such: `run/` is gitignored by `loop init`.
    let ignore = std::fs::read_to_string(fx.project.join(".loop/.gitignore")).unwrap();
    assert!(ignore.contains("run/"), "got: {ignore}");
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
    fx.init_tiny("TINY-1");

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
    fx.init_tiny("TINY-1");

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

/// The crash-resume contract of skills/loop-authoring/references/runtime.md: a ledger whose last write was torn
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
    fx.init_tiny("TINY-1");

    // A run killed by SIGKILL partway through writing `worker_output`. The
    // last line stays hand-written: a torn line is the one thing a serializer
    // cannot produce, and reproducing it exactly is the point of this test.
    fx.write_ledger(&[
        run_started_line("TINY-1"),
        line(
            fixtures::entered("implement", 1, 1)
                .at("2026-07-24T22:00:01.000Z")
                .elapsed(41),
        ),
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
    fx.init_tiny("TINY-1");

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
    let fx = unscripted();
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
    let fx = unscripted();
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
    let fx = unscripted();
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
    let fx = unscripted();
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
    let fx = unscripted();
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
    let fx = unscripted();
    let out = fx.run(&["logs", "--raw", "-n", "3"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stdout(&out).is_empty());
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot be used"));
}

#[test]
fn logs_empty_ledger_has_human_message_but_raw_is_empty() {
    let fx = unscripted();
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

/// A reader that walks away mid-write must kill the writer quietly. Rust sets
/// `SIGPIPE` to `SIG_IGN` before `main`, so a closed stdout came back as `EPIPE`
/// and surfaced two different ways: the `println!` commands panicked outright
/// (`loop status | head -1` printed `failed printing to stdout: Broken pipe` and
/// exited 101), while the ones writing through an `io::Result` reported
/// `error: Broken pipe (os error 32)` and exited 1. Every listing here is
/// documented as a pipeline's input (`loop sessions | fzf | awk`), so `main.rs`
/// puts the default disposition back — and both shapes are checked, since one
/// fix covers a `println!` and a bare write alike.
///
/// The output has to outlast the pipe buffer, or the writer finishes before the
/// reader closes and no signal is ever raised — hence a ledger far bigger than
/// the 64 KiB a pipe holds, and `-n` large enough to print all of it.
#[cfg(unix)]
#[test]
fn a_closed_pipe_kills_the_writer_without_a_panic() {
    use std::io::Read as _;
    use std::os::unix::process::ExitStatusExt as _;
    use std::process::Stdio;

    let fx = unscripted();
    fx.run(&["init", "TINY-1"]);
    fx.write_ledger(&note_lines(8_000));
    assert!(
        std::fs::metadata(fx.project.join(".loop/ledger.jsonl"))
            .unwrap()
            .len()
            > 64 * 1024,
        "the writer has to still be writing when the reader goes away"
    );

    // `--raw` writes the ledger through an `io::Result`; the human tail is a
    // `println!` per event, which is the path that used to panic.
    for args in [&["logs", "--raw"][..], &["logs", "-n", "8000"][..]] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_loop"))
            .args(args)
            .current_dir(&fx.project)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn loop");

        // `| head -1`: take a little, then close the read end. `read_exact`
        // rather than `read`, so this cannot pass on a short first chunk —
        // the ledger is ~900 KiB, so 128 bytes are certainly there.
        let mut first = [0u8; 128];
        child
            .stdout
            .as_mut()
            .unwrap()
            .read_exact(&mut first)
            .expect("the first bytes arrive before the pipe closes");
        drop(child.stdout.take());

        // Drained to EOF before `wait`, so a full stderr pipe cannot deadlock.
        let mut stderr = String::new();
        child
            .stderr
            .as_mut()
            .unwrap()
            .read_to_string(&mut stderr)
            .unwrap();
        let status = child.wait().unwrap();

        assert_eq!(
            stderr, "",
            "a closed pipe is not an error to report ({args:?})"
        );
        // 13 is `SIGPIPE`. Killed by the signal, not exited: the status a shell
        // reports as 141, and one that cannot be confused with the 0 that
        // `loop run` reserves for a finished ticket.
        assert_eq!(
            status.signal(),
            Some(13),
            "expected death by SIGPIPE for {args:?}, got {status:?}"
        );
    }
}

#[test]
fn logs_does_not_load_a_missing_or_invalid_machine() {
    let fx = unscripted();
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
    let fx = unscripted();
    fx.init_tiny("TINY-1");

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
    fx.init_tiny("TINY-1");

    // An interrupted run that had already burned an hour.
    fx.write_ledger(&[
        run_started_line("TINY-1"),
        line(
            fixtures::entered("implement", 1, 1)
                .at("2026-07-24T23:00:00.000Z")
                .elapsed(3600),
        ),
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

// ── `loop sessions` / `loop session` ─────────────────────────────────────────
//
// The pure half is unit-tested in `sessions`: candidate construction, the exact
// state filter, id resolution, and the column layout the listing promises a
// pipeline. What can only be checked through the real binary is what these
// cover — that the listing's own ids round-trip back into `loop session`, that
// the ledger's recorded id reaches pi as `--session` in the project directory,
// and that every way this can go wrong is a loud failure rather than a wrong
// session.

/// The deterministic id `stage.rs` assigns, built from the harness's own
/// sanitizer rather than from a second one written here. The copy this
/// replaces dropped `_`, which `sanitize_component` deliberately keeps — the
/// exact disagreement `config.rs` has a test pinning as the bug it once was.
fn session_id(ticket: &str, state: &str, cycle: u32, attempt: u32) -> String {
    format!(
        "{}-{}-{cycle}-{attempt}",
        sanitize_component(ticket, "ticket"),
        sanitize_component(state, "state")
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
    fx.init_tiny("TINY-1");
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
    let fx = unscripted();
    for (state, cycle, attempt) in [
        ("implement", 1, 1),
        ("implement-hotfix", 1, 1),
        ("implement", 2, 1),
        ("review", 2, 1),
    ] {
        fx.plant_session(&session_id("PROJ-9", state, cycle, attempt));
    }
    let mut plant = Plant::started("PROJ-9");
    plant
        .attempt("PROJ-9", "implement", 1, "first pass")
        .attempt("PROJ-9", "implement-hotfix", 1, "patched around it")
        .attempt("PROJ-9", "implement", 2, "second pass")
        .attempt("PROJ-9", "review", 2, "clean");
    fx.write_ledger(plant.lines());
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

/// The listing is the picker's replacement, so it has to carry everything a
/// choice needs — every attempt, oldest first, with the id that reopens it — and
/// the ids it prints must round-trip back into `loop session` unedited. Field 6
/// is the session id in every row; that is the promise `loop sessions | fzf`
/// and every `awk` after it rests on.
#[test]
fn sessions_lists_every_attempt_oldest_first_and_its_ids_reopen_them() {
    let fx = unscripted();
    let ids: Vec<String> = [
        ("implement", 1, 1),
        ("implement", 1, 2),
        ("review", 1, 1),
        ("implement", 2, 1),
    ]
    .iter()
    .map(|(state, cycle, attempt)| {
        let id = session_id("PROJ-9", state, *cycle, *attempt);
        fx.plant_session(&id);
        id
    })
    .collect();
    fx.write_ledger(&[
        run_started_line("PROJ-9"),
        entered_line("2026-07-26T12:01:00.000Z", "implement", 1, 1, Some(&ids[0])),
        line(fixtures::error("implement", "executor lost").at("2026-07-26T12:02:00.000Z")),
        entered_line("2026-07-26T12:03:00.000Z", "implement", 1, 2, Some(&ids[1])),
        output_line("2026-07-26T12:04:00.000Z", "implement", 1, "second pass"),
        entered_line("2026-07-26T12:05:00.000Z", "review", 1, 1, Some(&ids[2])),
        output_line("2026-07-26T12:06:00.000Z", "review", 1, "found a defect"),
        entered_line("2026-07-26T12:07:00.000Z", "implement", 2, 1, Some(&ids[3])),
    ]);

    let out = fx.run(&["sessions"]);
    assert!(out.status.success(), "{}", combined(&out));
    let text = stdout(&out);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 4, "one row per attempt:\n{text}");

    let field = |line: &str, n: usize| line.split_whitespace().nth(n).unwrap().to_string();
    assert_eq!(
        lines.iter().map(|l| field(l, 5)).collect::<Vec<_>>(),
        ids,
        "ledger order, oldest first:\n{text}"
    );
    // State, cycle, attempt and outcome are readable off the same fields in
    // every row, and the evidence for the outcome trails the id.
    assert_eq!(field(lines[0], 1), "implement");
    assert_eq!(
        (field(lines[0], 2), field(lines[0], 3)),
        ("1".into(), "1".into())
    );
    assert_eq!(field(lines[0], 4), "crashed");
    assert!(lines[0].ends_with("error: executor lost"), "{}", lines[0]);
    assert!(lines[1].ends_with("second pass"), "{}", lines[1]);
    assert_eq!(field(lines[3], 4), "incomplete");

    // Nothing is spawned by listing.
    assert!(fx.pi_launches().is_empty(), "{:?}", fx.pi_launches());

    // The state filter is exact, and narrows to the same rows.
    let filtered = fx.run(&["sessions", "implement"]);
    assert!(filtered.status.success(), "{}", combined(&filtered));
    assert_eq!(stdout(&filtered).lines().count(), 3);
    assert!(
        !stdout(&filtered).contains("review"),
        "{}",
        stdout(&filtered)
    );

    // …and an id read out of the listing opens exactly that attempt.
    let chosen = field(lines[2], 5);
    let opened = fx.run(&["session", &chosen]);
    assert!(opened.status.success(), "{}", combined(&opened));
    assert!(
        stdout(&opened).contains("review — cycle 1, attempt 1"),
        "the opening line names the attempt, not just the id: {}",
        stdout(&opened)
    );
    assert_eq!(
        fx.pi_launches()[0]["argv"],
        serde_json::json!(["--session", chosen])
    );
}

/// A removed way of invoking a command must name what replaced it. `loop
/// session` with no argument used to open a picker; it must not now mean
/// anything else, and it must not fail with a bare usage error either.
#[test]
fn session_without_an_id_names_the_command_that_replaced_the_picker() {
    let fx = unscripted();
    let id = session_id("PROJ-9", "implement", 1, 1);
    fx.plant_session(&id);
    fx.write_ledger(
        Plant::started("PROJ-9")
            .attempt("PROJ-9", "implement", 1, "did it")
            .lines(),
    );

    let out = fx.run(&["session"]);
    assert!(!out.status.success(), "{}", combined(&out));
    let text = combined(&out);
    assert!(text.contains("no longer opens a picker"), "{text}");
    assert!(
        text.contains("loop sessions"),
        "name the replacement: {text}"
    );
    assert!(text.contains("--latest"), "and the scripted path: {text}");
    assert!(
        fx.pi_launches().is_empty(),
        "nothing may be launched: {:?}",
        fx.pi_launches()
    );

    // A state name is what the old positional took, so it is the likeliest
    // thing to arrive here — and it gets told the two commands that work.
    let stale = fx.run(&["session", "implement"]);
    assert!(!stale.status.success(), "{}", combined(&stale));
    let text = combined(&stale);
    assert!(
        text.contains("`implement` is a state, not a session id"),
        "{text}"
    );
    assert!(text.contains("loop sessions implement"), "{text}");
    assert!(text.contains("loop session --latest implement"), "{text}");
    assert!(fx.pi_launches().is_empty());

    // An id that is neither a state nor recorded still says where to look.
    let bogus = fx.run(&["session", "no-such-id"]);
    assert!(!bogus.status.success());
    let text = combined(&bogus);
    assert!(text.contains("has session id `no-such-id`"), "{text}");
    assert!(
        text.contains("ledger.jsonl"),
        "name the ledger read: {text}"
    );
}

/// No usable candidate is a specific error naming the ledger and the filter —
/// including when the ledger *has* entries but none recorded an id, which is
/// what a pre-session ledger looks like. An empty listing is never silence: a
/// `loop sessions | fzf` that prints nothing has to say why.
#[test]
fn session_with_no_usable_candidate_names_the_filter() {
    let fx = unscripted();

    // An empty ledger.
    let empty = fx.run(&["sessions"]);
    assert!(!empty.status.success());
    assert!(stdout(&empty).is_empty(), "{}", stdout(&empty));
    let text = combined(&empty);
    assert!(text.contains("no Worker session"), "{text}");
    assert!(text.contains("any state"), "{text}");

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
    assert!(
        text.contains("state_entered.session_id"),
        "say where ids come from, so `nothing here` is distinguishable from a \
         ledger written before they existed: {text}"
    );

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
    assert!(combined(&fx.run(&["sessions", "deploy"])).contains("state `deploy`"));
    assert!(fx.pi_launches().is_empty());
}

/// An attempt with no `worker_output` still opens — that is precisely the
/// transcript you want after a crash — but it warns, on stderr, so the warning
/// survives a piped stdout and never contaminates it.
#[test]
fn session_warns_when_the_chosen_attempt_never_reported() {
    let fx = unscripted();
    let id = session_id("PROJ-9", "implement", 1, 1);
    fx.plant_session(&id);
    fx.write_ledger(&[
        run_started_line("PROJ-9"),
        entered_line("2026-07-26T12:01:00.000Z", "implement", 1, 1, Some(&id)),
        line(fixtures::error("implement", "executor lost").at("2026-07-26T12:02:00.000Z")),
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
    let fx = unscripted();
    let id = session_id("PROJ-9", "implement", 1, 1);
    fx.plant_session(&id);
    fx.write_ledger(
        Plant::started("PROJ-9")
            .attempt("PROJ-9", "implement", 1, "did it")
            .lines(),
    );

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
/// whose thinking, stage prompt frontmatter and machine defaults each supply a
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
 {:implement {:stage-prompt "implement"
              :thinking "max"
              :skills ["local-only"]
              :mcp ["warehouse"]
              :description "Do the work."}
  :verify {:stage-prompt "verify" :description "Check it."}}

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

/// Scaffold the fixture PREVIEW_MACHINE expects: a local stage prompt that shadows
/// a toolbox one, a toolbox stage prompt with no frontmatter model, and a skill at
/// each level.
fn preview_fixture() -> Fixture {
    let fx = unscripted();
    fx.run(&["init", "PREV-1"]);
    fx.machine(PREVIEW_MACHINE);

    fx.write(
        ".loop/stage-prompts/implement.md",
        "---\nname: implement\ndescription: Local override.\nmodel: frontmatter-model\n---\n\
         Work on $TICKET_ID, cycle $CYCLE.\n\n$TASK\n\nDigest: $LEDGER_DIGEST\n\nHome is $HOME.\n",
    );
    fx.write(
        ".loop/stage-prompts/verify.md",
        "---\nname: verify\n---\nCheck it.\n",
    );
    fx.write(".loop/skills/shared.md", "# shared\n");
    fx.write(".loop/skills/local-only.md", "# local-only\n");
    fx
}

/// The whole-machine report: every layered override resolved the way the run
/// would resolve it, every reference named by path, and each edge's real
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
        // state, `model` off the stage prompt frontmatter (beating the machine
        // default), `provider` off the built-in floor.
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

    // Every resolved reference names the file that would actually be loaded,
    // and all of them are inside the ticket directory.
    for expected in [
        fx.project.join(".loop/stage-prompts/implement.md"),
        fx.project.join(".loop/skills/local-only.md"),
        fx.project.join(".loop/skills/shared.md"),
    ] {
        assert!(text.contains(&expected.display().to_string()), "{text}");
    }
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
        "reference         `implement` (name)",
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
    let fx = unscripted();
    fx.run(&["init", "PREV-2"]);
    fx.machine(&TINY_MACHINE.replace(
        r#":stage-prompt "implement""#,
        r#":stage-prompt "nonexistent""#,
    ));

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
        text.contains("error  implement: stage prompt for state `implement` does not resolve"),
        "{text}"
    );
    assert!(text.contains("error: 1 error(s)"), "{text}");

    // And the same diagnostic comes out of `validate`, which is the guarantee
    // that preview is not linting with a weaker rule set.
    let validate = fx.run(&["validate"]);
    assert!(!validate.status.success());
    assert!(
        combined(&validate).contains("stage prompt for state `implement` does not resolve"),
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
        fx.project.join(".loop/run"),
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
        "`loop sessions implement`",
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
    let fx = unscripted();
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
    let fx = unscripted();
    fx.init_tiny("TINY-1");
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
    let fx = unscripted();
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
            GuardOutcome::Pass,
            GuardOutcome::Skip,
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
    fx.init_tiny("TINY-1");
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
    let fx = unscripted();
    fx.init_tiny("TINY-1");
    fx.write_ledger(&[
        run_started_line("TINY-1"),
        entered_line("2026-07-26T12:00:01Z", "implement", 1, 1, Some("s-1")),
        output_line("2026-07-26T12:01:00Z", "implement", 1, "first try"),
        proposed_line("2026-07-26T12:01:01Z", "implement", "done", "looks done"),
        guard_line(
            "2026-07-26T12:01:02Z",
            "implement",
            "done",
            GuardOutcome::Fail,
            GuardOutcome::Skip,
            Some("cargo test\nFAILED: 3 tests"),
            None,
        ),
        entered_line("2026-07-26T12:02:00Z", "implement", 1, 2, Some("s-2")),
        output_line("2026-07-26T12:03:00Z", "implement", 1, "second try"),
        line(
            fixtures::blocked("implement", "I cannot get the suite green")
                .at("2026-07-26T12:03:01Z"),
        ),
        line(
            fixtures::navigator("implement", "blocked")
                .at("2026-07-26T12:03:02Z")
                .routing(
                    "blocked: I cannot get the suite green",
                    Some("summarize what you tried"),
                )
                .usage(50, 0.001),
        ),
        line(
            fixtures::error("implement", "escalated: the suite never went green")
                .at("2026-07-26T12:03:03Z")
                .fatal(),
        ),
        line(
            fixtures::finished(RunStatus::Aborted, "blocked")
                .at("2026-07-26T12:03:04Z")
                .no_terminal()
                .totals(Totals {
                    usage: Usage {
                        cost_usd: 0.05,
                        tokens: 0,
                    },
                    wallclock_s: 184,
                    transitions: 0,
                }),
        ),
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
 :states {:implement {:stage-prompt "implement" :description "Do the work."}}
 :transitions [{:from "implement" :to "done" :criteria "The work is done."}]}
"#;
