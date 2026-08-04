//! `mock-pi` — a scripted stand-in for the real `pi`, so the whole harness is
//! testable deterministically, offline, and for $0.
//!
//! Point the harness at it with `LOOP_PI_BIN=/path/to/mock-pi`. It accepts
//! (and entirely ignores) every CLI argument pi would take — `--print`,
//! `--mode json`, `--model`, `-e`, all of it — and instead reads its
//! invocation context from environment variables `loop-runner`'s command
//! builders always set (`crates/loop-runner/src/command.rs`), alongside pi's
//! real flags:
//!
//! - `LOOP_MOCK_SCRIPT` (required) — path to the script JSON file (below).
//! - `LOOP_MOCK_ROLE` — `"worker"` | `"judge"` | `"navigator"`.
//! - `LOOP_MOCK_STATE`, `LOOP_MOCK_CYCLE`, `LOOP_MOCK_ATTEMPT` — set for
//!   worker (and `LOOP_MOCK_STATE` for navigator, from `NavigatorSpec::from`);
//!   absent for the judge, which has no notion of state/cycle.
//! - `LOOP_HANDOFF` — where a worker step's `transition` is written, standing
//!   in for the file a real agent writes to end its stage.
//!
//! These are diagnostic-only: the real pi ignores environment variables it
//! doesn't recognize, so `loop-runner` sets them unconditionally regardless
//! of which binary `LOOP_PI_BIN` actually points at.
//!
//! # The script
//!
//! `LOOP_MOCK_SCRIPT` names a JSON file:
//!
//! ```json
//! {
//!   "default": { "summary": "did the thing", "transition": {"to": "review", "rationale": "..."} },
//!   "steps": [
//!     { "match": {"role": "worker", "state": "implement", "cycle": 1},
//!       "summary": "implemented",
//!       "transition": {"to": "review", "rationale": "plan items done"},
//!       "usage": {"tokens": 100, "cost_usd": 0.01} },
//!     { "match": {"role": "worker", "state": "qa-staging", "cycle": 1},
//!       "transition": {"to": "qa-staging", "rationale": "flaky executor"} },
//!     { "match": {"role": "worker", "state": "debug"}, "exit": "crash" },
//!     { "match": {"role": "judge"}, "verdict": {"pass": true, "rationale": "evidence checks out"} },
//!     { "match": {"role": "navigator"}, "choice": {"to": "qa-staging", "entry_prompt": "re-run QA"} }
//!   ]
//! }
//! ```
//!
//! Top level:
//! - `default` (optional) — a [`Step`] used when no `steps[]` entry matches
//!   (or matched but was already consumed). Defaults to an empty, successful,
//!   no-op run if omitted entirely.
//! - `steps` (optional, default `[]`) — matched **in order**; the first whose
//!   `match` fits the invocation wins.
//!
//! `match` (all keys optional; an absent key is a wildcard):
//! - `role`: `"worker" | "judge" | "navigator"`.
//! - `state`: exact `LOOP_MOCK_STATE` match.
//! - `cycle`, `attempt`: exact numeric match against `LOOP_MOCK_CYCLE` /
//!   `LOOP_MOCK_ATTEMPT`.
//!
//! A matched step is **consumed** (won't match again) unless it sets
//! `"repeat": true`. Consumption state is tracked in a sidecar JSON file next
//! to the script (`<script>.consumed.json`, a JSON array of consumed step
//! indices), so a run's successive spawns walk the script one step at a time.
//!
//! Step fields (all optional; unset ones behave as if absent — they do not
//! fall back to `default`'s fields individually, `default` is only used
//! wholesale when nothing in `steps` matches):
//! - `summary`: the assistant's final text.
//! - `usage`: `{"tokens": u64, "cost_usd": f64}`, emitted as one assistant
//!   `message_end`'s usage.
//! - `transition`: `{"to": string|null, "blocked": bool, "rationale": string,
//!   "artifacts": [{"name","path"}]}` — worker only. **Written as JSON to
//!   `$LOOP_HANDOFF`**, exactly as a real worker would. Omit it to simulate a
//!   worker that ends its turn without handing off.
//! - `verdict`: `{"pass": bool, "rationale": string}` — judge only. Rendered
//!   into the final assistant message as `PASS|FAIL\n<rationale>`.
//! - `choice`: `{"to": string, "entry_prompt": string}` — navigator only.
//!   Rendered into the final assistant message as `<to>\n<entry_prompt>`.
//!
//! Because the tool-less roles answer in prose, scripting `summary` *without*
//! `verdict`/`choice` is how a test drives an off-contract reply and asserts
//! that the harness fails closed.
//!
//! - `exit`: `"ok"` (default) | `"crash"`. `"crash"` emits the session header
//!   and `agent_start`/`turn_start`, then a deliberately truncated, invalid
//!   final line with **no trailing newline** (simulating a process killed
//!   mid-write) and exits 1. No `message_end`, no handoff, no `agent_end`.
//!
//! If `LOOP_MOCK_SCRIPT` is unset, or the file can't be read/parsed, mock-pi
//! prints an error to stderr and exits 1 (emitting no stdout at all) — that
//! is a harness misconfiguration, not something `parse_stream` needs to
//! tolerate.
//!
//! # The session store
//!
//! `loop session` does not spawn a stage; it reopens one, with
//! `pi --session <id>` and nothing else. Modelling that needs a store rather
//! than a script, so two more variables (both optional, both ignored unless
//! set) stand in for pi's own session directory:
//!
//! - `LOOP_MOCK_SESSIONS` — a directory. A spawn given `--session-id <id>`
//!   records `<dir>/<id>.jsonl`; a spawn given `--session <id>` requires it and
//!   **exits 1 if it is absent**, which is exactly why loop passes `--session`:
//!   reopening history that is gone must fail rather than silently create an
//!   empty session under the same id.
//! - `LOOP_MOCK_ARGV_LOG` — a file. Every `--session` invocation appends one
//!   JSON line, `{"argv": [...], "cwd": "..."}`, so a test can assert on the
//!   argv and working directory a real pi would have received.
//!
//! With neither set, `--session <id>` is a successful no-op.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Deserialize, Default)]
struct Script {
    #[serde(default)]
    default: Option<Step>,
    #[serde(default)]
    steps: Vec<ScriptedStep>,
}

#[derive(Debug, Deserialize, Clone)]
struct ScriptedStep {
    #[serde(rename = "match", default)]
    matcher: Matcher,
    #[serde(flatten)]
    step: Step,
    #[serde(default)]
    repeat: bool,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct Matcher {
    role: Option<String>,
    state: Option<String>,
    cycle: Option<u32>,
    attempt: Option<u32>,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct Step {
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    usage: Option<UsageSpec>,
    #[serde(default)]
    transition: Option<TransitionSpec>,
    #[serde(default)]
    verdict: Option<VerdictSpec>,
    #[serde(default)]
    choice: Option<ChoiceSpec>,
    #[serde(default)]
    exit: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Default, Clone)]
struct UsageSpec {
    #[serde(default)]
    tokens: u64,
    #[serde(default)]
    cost_usd: f64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct ArtifactSpec {
    name: String,
    path: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct TransitionSpec {
    #[serde(default)]
    to: Option<String>,
    #[serde(default)]
    blocked: bool,
    #[serde(default)]
    rationale: String,
    #[serde(default)]
    artifacts: Vec<ArtifactSpec>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct VerdictSpec {
    pass: bool,
    rationale: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct ChoiceSpec {
    to: String,
    entry_prompt: String,
}

struct Invocation {
    role: String,
    state: Option<String>,
    cycle: Option<u32>,
    attempt: Option<u32>,
}

fn env_str(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|s| !s.is_empty())
}

fn env_u32(name: &str) -> Option<u32> {
    env_str(name).and_then(|s| s.parse().ok())
}

fn matches(m: &Matcher, inv: &Invocation) -> bool {
    if let Some(r) = &m.role {
        if r != &inv.role {
            return false;
        }
    }
    if let Some(s) = &m.state {
        if Some(s.as_str()) != inv.state.as_deref() {
            return false;
        }
    }
    if let Some(c) = m.cycle {
        if Some(c) != inv.cycle {
            return false;
        }
    }
    if let Some(a) = m.attempt {
        if Some(a) != inv.attempt {
            return false;
        }
    }
    true
}

fn sidecar_path(script_path: &Path) -> PathBuf {
    let mut s = script_path.as_os_str().to_owned();
    s.push(".consumed.json");
    PathBuf::from(s)
}

fn load_consumed(path: &Path) -> BTreeSet<usize> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<usize>>(&s).ok())
        .map(|v| v.into_iter().collect())
        .unwrap_or_default()
}

fn save_consumed(path: &Path, consumed: &BTreeSet<usize>) {
    let v: Vec<usize> = consumed.iter().copied().collect();
    if let Ok(s) = serde_json::to_string(&v) {
        let _ = std::fs::write(path, s);
    }
}

/// Picks the step to run: the first unconsumed `steps[]` entry that matches
/// the invocation, falling back to `default` (or an empty step) if none do.
fn select_step(script: &Script, inv: &Invocation, consumed_path: &Path) -> Step {
    let mut consumed = load_consumed(consumed_path);

    let hit = script
        .steps
        .iter()
        .enumerate()
        .find(|(idx, s)| !consumed.contains(idx) && matches(&s.matcher, inv));

    match hit {
        Some((idx, s)) => {
            if !s.repeat {
                consumed.insert(idx);
                save_consumed(consumed_path, &consumed);
            }
            s.step.clone()
        }
        None => script.default.clone().unwrap_or_default(),
    }
}

/// A tiny line-writer that also tracks whether anything failed, so we don't
/// need to `.unwrap()` at every `writeln!`.
struct Emitter<W: Write> {
    out: W,
}

impl<W: Write> Emitter<W> {
    fn line(&mut self, v: &Value) {
        let _ = writeln!(self.out, "{v}");
    }

    fn raw(&mut self, s: &str) {
        let _ = write!(self.out, "{s}");
    }

    fn flush(&mut self) {
        let _ = self.out.flush();
    }
}

fn now_iso() -> String {
    // No chrono dependency here; a fixed-format placeholder is fine since
    // nothing in loop-runner parses this field's contents, only its presence.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("1970-01-01T00:00:{secs:02}Z")
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn assistant_message(summary: &str, usage: &UsageSpec) -> Value {
    let content = if summary.is_empty() {
        json!([])
    } else {
        json!([{"type": "text", "text": summary}])
    };
    json!({
        "role": "assistant",
        "content": content,
        "api": "mock",
        "provider": "mock",
        "model": "mock",
        "usage": {
            "input": 0,
            "output": 0,
            "cacheRead": 0,
            "cacheWrite": 0,
            "totalTokens": usage.tokens,
            "cost": {
                "input": 0.0,
                "output": 0.0,
                "cacheRead": 0.0,
                "cacheWrite": 0.0,
                "total": usage.cost_usd,
            },
        },
        "stopReason": "stop",
        "timestamp": now_ms(),
    })
}

/// What the spawn says in its final assistant message.
///
/// For a Worker this is just the scripted summary — its decision travels in
/// the handoff file, not in prose. For the two tool-less roles the final
/// message *is* the answer, so this renders the scripted verdict/choice into
/// the first-line contract `loop_runner::reply` parses. A step that scripts
/// neither falls back to `summary`, which is how a test exercises an
/// off-contract reply.
fn final_text(role: &str, step: &Step) -> String {
    let summary = step.summary.clone().unwrap_or_default();
    match role {
        "judge" => match &step.verdict {
            Some(v) => {
                let token = if v.pass { "PASS" } else { "FAIL" };
                format!("{token}\n{}", v.rationale).trim_end().to_string()
            }
            None => summary,
        },
        "navigator" => match &step.choice {
            Some(c) => format!("{}\n{}", c.to, c.entry_prompt)
                .trim_end()
                .to_string(),
            None => summary,
        },
        _ => summary,
    }
}

/// Write the Worker's handoff file, the way a real agent would during its turn.
///
/// Silently does nothing when `LOOP_HANDOFF` is unset, so a spawn built by
/// something other than `loop-runner` still runs.
fn write_handoff(step: &Step) {
    let Some(t) = &step.transition else { return };
    let Some(path) = std::env::var_os("LOOP_HANDOFF") else {
        return;
    };
    let payload = json!({
        "to": t.to,
        "blocked": t.blocked,
        "rationale": t.rationale,
        "artifacts": t.artifacts,
    });
    let path = PathBuf::from(path);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&path, payload.to_string()) {
        eprintln!("mock-pi: failed writing handoff {}: {e}", path.display());
    }
}

fn run_stream<W: Write>(out: W, role: &str, inv: &Invocation, step: &Step) -> ExitCode {
    let mut em = Emitter { out };

    let cwd = std::env::current_dir().unwrap_or_default();
    let session_id = format!(
        "mock-{role}-{}-{}-{}",
        inv.state.as_deref().unwrap_or("-"),
        inv.cycle.unwrap_or(0),
        inv.attempt.unwrap_or(0),
    );
    em.line(&json!({
        "type": "session",
        "version": 3,
        "id": session_id,
        "timestamp": now_iso(),
        "cwd": cwd,
    }));
    em.line(&json!({"type": "agent_start"}));
    em.line(&json!({"type": "turn_start"}));

    if step.exit.as_deref() == Some("crash") {
        // Simulate a process killed mid-write: a syntactically broken final
        // line, with no trailing newline at all. No handoff is written — a
        // stage that died never decided anything.
        em.raw(r#"{"type":"message_start","message":{"role":"assistant","con"#);
        em.flush();
        return ExitCode::from(1);
    }

    if role == "worker" {
        write_handoff(step);
    }

    let usage = step.usage.clone().unwrap_or_default();
    let assistant = assistant_message(&final_text(role, step), &usage);
    em.line(&json!({"type": "message_start", "message": assistant}));
    em.line(&json!({"type": "message_end", "message": assistant}));

    em.line(&json!({
        "type": "turn_end",
        "message": assistant,
        "toolResults": [],
    }));
    em.line(&json!({"type": "agent_end", "messages": [assistant]}));
    em.flush();

    ExitCode::SUCCESS
}

/// The value following `flag` in `argv`, if present.
fn flag_value(argv: &[String], flag: &str) -> Option<String> {
    argv.iter()
        .position(|a| a == flag)
        .and_then(|i| argv.get(i + 1))
        .cloned()
}

fn session_file(id: &str) -> Option<PathBuf> {
    env_str("LOOP_MOCK_SESSIONS").map(|dir| PathBuf::from(dir).join(format!("{id}.jsonl")))
}

/// `pi --session <id>`: reopen a persisted session interactively.
///
/// No stream, no script — the whole point of `loop session` is that this process
/// inherits the terminal and takes over, so there is nothing for the harness to
/// parse. All mock-pi has to be faithful about is the two observable facts loop
/// depends on: what it was called with, and that a vanished session fails.
fn reopen_session(argv: &[String], id: &str) -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_default();
    if let Some(log) = env_str("LOOP_MOCK_ARGV_LOG") {
        let line = json!({"argv": argv, "cwd": cwd}).to_string();
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)
        {
            let _ = writeln!(f, "{line}");
        }
    }

    if let Some(path) = session_file(id) {
        if !path.exists() {
            eprintln!("mock-pi: no session named {id}");
            return ExitCode::from(1);
        }
    }
    println!("mock-pi: reopened session {id}");
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    // Accept (and ignore) every real pi flag: we drive entirely off env vars —
    // except the two that identify a session, which have no env equivalent.
    let argv: Vec<String> = std::env::args().skip(1).collect();

    if let Some(id) = flag_value(&argv, "--session") {
        return reopen_session(&argv, &id);
    }
    // A stage spawn: leave a session behind for `loop session` to reopen later.
    if let (Some(id), Some(_)) = (
        flag_value(&argv, "--session-id"),
        env_str("LOOP_MOCK_SESSIONS"),
    ) {
        if let Some(path) = session_file(&id) {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&path, format!("{{\"id\":{id:?}}}\n"));
        }
    }

    let role = env_str("LOOP_MOCK_ROLE").unwrap_or_else(|| "worker".to_string());
    let inv = Invocation {
        role: role.clone(),
        state: env_str("LOOP_MOCK_STATE"),
        cycle: env_u32("LOOP_MOCK_CYCLE"),
        attempt: env_u32("LOOP_MOCK_ATTEMPT"),
    };

    let script_path = match env_str("LOOP_MOCK_SCRIPT") {
        Some(p) => PathBuf::from(p),
        None => {
            eprintln!("mock-pi: LOOP_MOCK_SCRIPT is not set");
            return ExitCode::from(1);
        }
    };

    let script: Script = match std::fs::read_to_string(&script_path) {
        Ok(s) => match serde_json::from_str(&s) {
            Ok(script) => script,
            Err(e) => {
                eprintln!(
                    "mock-pi: invalid script JSON in {}: {e}",
                    script_path.display()
                );
                return ExitCode::from(1);
            }
        },
        Err(e) => {
            eprintln!(
                "mock-pi: could not read script {}: {e}",
                script_path.display()
            );
            return ExitCode::from(1);
        }
    };

    let consumed_path = sidecar_path(&script_path);
    let step = select_step(&script, &inv, &consumed_path);

    let stdout = std::io::stdout();
    run_stream(stdout.lock(), &role, &inv, &step)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step_matching(json: &str) -> Script {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn matcher_wildcards_absent_fields() {
        let script = step_matching(r#"{"steps":[{"match":{"role":"worker"},"summary":"hit"}]}"#);
        let inv = Invocation {
            role: "worker".into(),
            state: Some("implement".into()),
            cycle: Some(3),
            attempt: Some(1),
        };
        assert!(matches(&script.steps[0].matcher, &inv));
    }

    #[test]
    fn select_step_consumes_first_match_only_once() {
        let script = step_matching(
            r#"{"steps":[
                {"match":{"role":"worker"},"summary":"first"},
                {"match":{"role":"worker"},"summary":"second"}
            ]}"#,
        );
        let dir = std::env::temp_dir().join(format!("mockpi-test-{}", now_ms()));
        let sidecar = dir.with_extension("consumed.json");
        let _ = std::fs::remove_file(&sidecar);

        let inv = Invocation {
            role: "worker".into(),
            state: None,
            cycle: None,
            attempt: None,
        };

        let s1 = select_step(&script, &inv, &sidecar);
        assert_eq!(s1.summary.as_deref(), Some("first"));
        let s2 = select_step(&script, &inv, &sidecar);
        assert_eq!(s2.summary.as_deref(), Some("second"));

        let _ = std::fs::remove_file(&sidecar);
    }

    #[test]
    fn select_step_repeat_true_matches_forever() {
        let script = step_matching(
            r#"{"steps":[{"match":{"role":"judge"},"repeat":true,"verdict":{"pass":true,"rationale":"ok"}}]}"#,
        );
        let dir = std::env::temp_dir().join(format!("mockpi-test-{}", now_ms() + 1));
        let sidecar = dir.with_extension("consumed.json");
        let _ = std::fs::remove_file(&sidecar);

        let inv = Invocation {
            role: "judge".into(),
            state: None,
            cycle: None,
            attempt: None,
        };

        for _ in 0..3 {
            let s = select_step(&script, &inv, &sidecar);
            assert!(s.verdict.is_some());
        }

        let _ = std::fs::remove_file(&sidecar);
    }

    #[test]
    fn falls_back_to_default_when_nothing_matches() {
        let script = step_matching(
            r#"{"default":{"summary":"fallback"},"steps":[{"match":{"role":"judge"},"summary":"nope"}]}"#,
        );
        let dir = std::env::temp_dir().join(format!("mockpi-test-{}", now_ms() + 2));
        let sidecar = dir.with_extension("consumed.json");
        let _ = std::fs::remove_file(&sidecar);

        let inv = Invocation {
            role: "worker".into(),
            state: None,
            cycle: None,
            attempt: None,
        };

        let s = select_step(&script, &inv, &sidecar);
        assert_eq!(s.summary.as_deref(), Some("fallback"));

        let _ = std::fs::remove_file(&sidecar);
    }

    fn worker_inv() -> Invocation {
        Invocation {
            role: "worker".into(),
            state: Some("implement".into()),
            cycle: Some(1),
            attempt: Some(1),
        }
    }

    #[test]
    fn run_stream_worker_emits_valid_ndjson_and_writes_the_handoff() {
        let step: Step = serde_json::from_str(
            r#"{"summary":"done","transition":{"to":"review","rationale":"ok"}}"#,
        )
        .unwrap();

        let dir = std::env::temp_dir().join(format!("mock-pi-handoff-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let handoff = dir.join("h.json");
        // SAFETY: single-threaded test; `run_stream` reads this synchronously.
        unsafe { std::env::set_var("LOOP_HANDOFF", &handoff) };

        let mut buf = Vec::new();
        let code = run_stream(&mut buf, "worker", &worker_inv(), &step);
        assert_eq!(code, ExitCode::SUCCESS);

        let text = String::from_utf8(buf).unwrap();
        for line in text.lines() {
            serde_json::from_str::<Value>(line).expect("every mock-pi line is valid JSON");
        }

        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&handoff).unwrap()).unwrap();
        assert_eq!(written["to"], "review");
        assert_eq!(written["rationale"], "ok");

        unsafe { std::env::remove_var("LOOP_HANDOFF") };
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The tool-less roles answer in their final message, so their scripted
    /// verdict/choice has to land there in the shape `loop_runner::reply`
    /// parses — this is the seam between the two crates.
    #[test]
    fn tool_less_roles_answer_in_the_final_message() {
        let judge: Step =
            serde_json::from_str(r#"{"verdict":{"pass":false,"rationale":"not run"}}"#).unwrap();
        assert_eq!(final_text("judge", &judge), "FAIL\nnot run");

        let judge_pass: Step =
            serde_json::from_str(r#"{"verdict":{"pass":true,"rationale":"green"}}"#).unwrap();
        assert_eq!(final_text("judge", &judge_pass), "PASS\ngreen");

        let nav: Step =
            serde_json::from_str(r#"{"choice":{"to":"debug","entry_prompt":"isolate it"}}"#)
                .unwrap();
        assert_eq!(final_text("navigator", &nav), "debug\nisolate it");

        // No scripted answer: the raw summary goes out, which is how a test
        // drives an off-contract reply.
        let off: Step = serde_json::from_str(r#"{"summary":"I'm not sure"}"#).unwrap();
        assert_eq!(final_text("judge", &off), "I'm not sure");
    }

    /// A crashed stage never decided anything, so it must leave no handoff for
    /// the next attempt to pick up.
    #[test]
    fn a_crashed_worker_writes_no_handoff() {
        let step: Step = serde_json::from_str(
            r#"{"exit":"crash","transition":{"to":"review","rationale":"ok"}}"#,
        )
        .unwrap();

        let dir = std::env::temp_dir().join(format!("mock-pi-crash-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let handoff = dir.join("h.json");
        unsafe { std::env::set_var("LOOP_HANDOFF", &handoff) };

        let mut buf = Vec::new();
        assert_eq!(
            run_stream(&mut buf, "worker", &worker_inv(), &step),
            ExitCode::from(1)
        );
        assert!(!handoff.exists());

        unsafe { std::env::remove_var("LOOP_HANDOFF") };
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `--session` and `--session-id` differ by four characters and mean
    /// opposite things (reopen vs. create), so the lookup has to be exact.
    #[test]
    fn flag_value_reads_the_exact_flag_not_a_prefix_of_it() {
        let argv: Vec<String> = ["--print", "--session-id", "abc", "--model", "m"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(flag_value(&argv, "--session-id").as_deref(), Some("abc"));
        assert_eq!(flag_value(&argv, "--session"), None);

        let reopen: Vec<String> = vec!["--session".into(), "abc".into()];
        assert_eq!(flag_value(&reopen, "--session").as_deref(), Some("abc"));
        assert_eq!(flag_value(&reopen, "--session-id"), None);
        // A trailing flag with nothing after it is not a value.
        assert_eq!(flag_value(&["--session".to_string()], "--session"), None);
    }

    #[test]
    fn run_stream_crash_truncates_and_exits_nonzero() {
        let step: Step = serde_json::from_str(r#"{"exit":"crash"}"#).unwrap();
        let inv = Invocation {
            role: "worker".into(),
            state: Some("debug".into()),
            cycle: Some(1),
            attempt: Some(1),
        };
        let mut buf = Vec::new();
        let code = run_stream(&mut buf, "worker", &inv, &step);
        assert_eq!(code, ExitCode::from(1));

        let text = String::from_utf8(buf).unwrap();
        // The last "line" is not terminated by a newline and isn't valid JSON.
        assert!(!text.ends_with('\n'));
        let last = text.lines().last().unwrap();
        assert!(serde_json::from_str::<Value>(last).is_err());
    }
}
