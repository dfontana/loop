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
//!   "default": { "summary": "did the thing", "transition": {"to": "review", "rationale": "…"} },
//!   "steps": [
//!     { "match": {"role": "worker", "state": "implement", "cycle": 1},
//!       "vars":  {"build": {"status": "pass", "id": "b-1"}},
//!       "summary": "implemented",
//!       "transition": {"to": "review", "rationale": "plan items done"},
//!       "usage": {"tokens": 100, "cost_usd": 0.01} },
//!     { "match": {"role": "worker", "state": "qa-staging", "cycle": 1},
//!       "vars": {"qa": {"result": "fail", "error_class": "transient"}},
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
//! - `vars`: a JSON object, emitted as a single `LOOP_VARS <json>` line
//!   (wrapped in a bit of surrounding prose, inside a synthetic
//!   `spark_build`-style tool call) — this is how a step feeds trusted,
//!   tool-emitted vars into the harness.
//! - `transition`: `{"to": string|null, "blocked": bool, "rationale": string,
//!   "artifacts": [{"name","path"}], "vars": object}` — worker only. Emits a
//!   `transition` tool call whose result is `LOOP_TRANSITION <json>`. Omit it
//!   to simulate a worker that ends its turn without transitioning.
//! - `verdict`: `{"pass": bool, "rationale": string}` — judge only. Emits
//!   `LOOP_VERDICT <json>`.
//! - `choice`: `{"to": string, "entry_prompt": string}` — navigator only.
//!   Emits `LOOP_CHOICE <json>`.
//! - `exit`: `"ok"` (default) | `"crash"`. `"crash"` emits the session header
//!   and `agent_start`/`turn_start`, then a deliberately truncated, invalid
//!   final line with **no trailing newline** (simulating a process killed
//!   mid-write) and exits 1. No `message_end`, no markers, no `agent_end`.
//!
//! If `LOOP_MOCK_SCRIPT` is unset, or the file can't be read/parsed, mock-pi
//! prints an error to stderr and exits 1 (emitting no stdout at all) — that
//! is a harness misconfiguration, not something `parse_stream` needs to
//! tolerate.

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
    vars: Option<Value>,
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

fn empty_object() -> Value {
    Value::Object(Default::default())
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
    #[serde(default = "empty_object")]
    vars: Value,
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
        let _ = writeln!(self.out, "{}", v);
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

/// Emits a `tool_execution_start`/`tool_execution_end` pair whose result text
/// is exactly `text` — mirroring the shape `crates/loop-toolbox/ext/*.ts`
/// extensions actually return (`{content:[{type:"text",text}]}`), and
/// appends the corresponding `ToolResultMessage`-shaped entry to
/// `tool_results` for the eventual `turn_end`.
fn emit_tool_call<W: Write>(
    em: &mut Emitter<W>,
    tool_results: &mut Vec<Value>,
    call_id: &str,
    tool_name: &str,
    args: Value,
    text: &str,
) {
    em.line(&json!({
        "type": "tool_execution_start",
        "toolCallId": call_id,
        "toolName": tool_name,
        "args": args,
    }));
    let result = json!({"content": [{"type": "text", "text": text}]});
    em.line(&json!({
        "type": "tool_execution_end",
        "toolCallId": call_id,
        "toolName": tool_name,
        "result": result,
        "isError": false,
    }));
    tool_results.push(json!({
        "role": "toolResult",
        "toolCallId": call_id,
        "toolName": tool_name,
        "content": [{"type": "text", "text": text}],
        "isError": false,
        "timestamp": now_ms(),
    }));
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
        // line, with no trailing newline at all.
        em.raw(r#"{"type":"message_start","message":{"role":"assistant","con"#);
        em.flush();
        return ExitCode::from(1);
    }

    let usage = step.usage.clone().unwrap_or_default();
    let summary = step.summary.clone().unwrap_or_default();
    let assistant = assistant_message(&summary, &usage);
    em.line(&json!({"type": "message_start", "message": assistant}));
    em.line(&json!({"type": "message_end", "message": assistant}));

    let mut tool_results = Vec::new();
    let mut next_call_id = 0usize;
    let mut call_id = || {
        next_call_id += 1;
        format!("call_{next_call_id}")
    };

    if let Some(vars) = &step.vars {
        let payload = serde_json::to_string(vars).unwrap_or_else(|_| "{}".to_string());
        let text = format!("Running checks...\nLOOP_VARS {payload}\nDone.");
        emit_tool_call(
            &mut em,
            &mut tool_results,
            &call_id(),
            "spark_build",
            json!({}),
            &text,
        );
    }

    match role {
        "worker" => {
            if let Some(t) = &step.transition {
                let payload = json!({
                    "to": t.to,
                    "blocked": t.blocked,
                    "rationale": t.rationale,
                    "artifacts": t.artifacts,
                    "vars": t.vars,
                });
                let text = format!("LOOP_TRANSITION {payload}");
                emit_tool_call(
                    &mut em,
                    &mut tool_results,
                    &call_id(),
                    "transition",
                    json!({"to": t.to, "blocked": t.blocked, "rationale": t.rationale}),
                    &text,
                );
            }
        }
        "judge" => {
            if let Some(v) = &step.verdict {
                let payload = json!({"pass": v.pass, "rationale": v.rationale});
                let text = format!("LOOP_VERDICT {payload}");
                emit_tool_call(
                    &mut em,
                    &mut tool_results,
                    &call_id(),
                    "verdict",
                    json!({"pass": v.pass, "rationale": v.rationale}),
                    &text,
                );
            }
        }
        "navigator" => {
            if let Some(c) = &step.choice {
                let payload = json!({"to": c.to, "entry_prompt": c.entry_prompt});
                let text = format!("LOOP_CHOICE {payload}");
                emit_tool_call(
                    &mut em,
                    &mut tool_results,
                    &call_id(),
                    "choose",
                    json!({"to": c.to, "entry_prompt": c.entry_prompt}),
                    &text,
                );
            }
        }
        _ => {}
    }

    em.line(&json!({
        "type": "turn_end",
        "message": assistant,
        "toolResults": tool_results,
    }));
    em.line(&json!({"type": "agent_end", "messages": [assistant]}));
    em.flush();

    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    // Accept (and ignore) every real pi flag: we drive entirely off env vars.
    let _ = std::env::args();

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

    #[test]
    fn run_stream_worker_emits_valid_ndjson_with_transition_and_vars() {
        let step: Step = serde_json::from_str(
            r#"{"summary":"done","vars":{"build":{"status":"pass"}},
                "transition":{"to":"review","rationale":"ok"}}"#,
        )
        .unwrap();
        let inv = Invocation {
            role: "worker".into(),
            state: Some("implement".into()),
            cycle: Some(1),
            attempt: Some(1),
        };
        let mut buf = Vec::new();
        let code = run_stream(&mut buf, "worker", &inv, &step);
        assert_eq!(code, ExitCode::SUCCESS);

        let text = String::from_utf8(buf).unwrap();
        let mut saw_transition = false;
        let mut saw_vars = false;
        for line in text.lines() {
            let v: Value = serde_json::from_str(line).expect("every mock-pi line is valid JSON");
            if v.get("type").and_then(Value::as_str) == Some("tool_execution_end") {
                let result_text = v["result"]["content"][0]["text"].as_str().unwrap_or("");
                if result_text.starts_with("LOOP_TRANSITION ") {
                    saw_transition = true;
                }
                if result_text.contains("LOOP_VARS ") {
                    saw_vars = true;
                }
            }
        }
        assert!(saw_transition);
        assert!(saw_vars);
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
