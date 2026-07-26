//! Spawning `pi` and reading its JSON event stream.
//!
//! See docs/02-how-it-works.md. Three roles, three cost profiles: the Worker
//! does the stage's work and ends with `transition`; the Judge independently
//! grades a `criteria`; the Navigator reroutes a blocked worker. All three are
//! `pi --print --mode json` subprocesses whose newline-delimited events this
//! crate parses.
//!
//! TASK T4 implements this crate (and `mock-pi`, which lets everything else be
//! tested without an API key).

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use loop_core::{
    AgentRunner, Choice, Config, CoreError, JudgeSpec, LOOP_CHOICE_MARKER, LOOP_VERDICT_MARKER,
    NavigatorSpec, Result, Verdict, WorkerResult, WorkerSpec,
};

pub mod check;
pub mod command;
pub mod stream;

pub use check::exec_check;
pub use stream::{StreamOutcome, parse_stream};

/// How many trailing stderr lines to keep from a spawn. A pi crash puts its
/// diagnosis in the last few lines; keeping a bounded tail means a process
/// that logs a gigabyte still costs one small buffer.
const STDERR_TAIL_LINES: usize = 20;

/// Spawns real `pi` subprocesses.
pub struct PiRunner {
    pi_bin: String,
    /// Echo each spawn's stderr as it arrives, so a human watching `loop run
    /// --verbose` sees the worker working. Off by default and in tests — but
    /// the tail is captured either way, so a crash is diagnosable without it.
    pub verbose: bool,
}

/// One spawn's result: what the stream said, whether it exited clean, and the
/// tail of what it complained about on the way.
struct SpawnOutcome {
    stream: StreamOutcome,
    exit_ok: bool,
    stderr_tail: String,
}

impl PiRunner {
    pub fn new(config: &Config) -> Self {
        Self {
            pi_bin: config.pi_bin.clone(),
            verbose: false,
        }
    }

    pub fn verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self
    }

    pub fn pi_bin(&self) -> &str {
        &self.pi_bin
    }

    /// Spawn `cmd`, stream-parse its stdout, and wait for it to exit.
    ///
    /// stdin is closed (these are non-interactive `--print` spawns). stderr is
    /// piped and drained by its own thread — never piped-but-unread, which
    /// would deadlock the moment pi writes enough to fill the pipe buffer, and
    /// never discarded, which is what used to reduce a failed spawn to a bare
    /// non-zero exit code with nothing to debug from.
    fn spawn_and_parse(&self, mut cmd: Command, role: &str) -> Result<SpawnOutcome> {
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| CoreError::agent(role, format!("failed to spawn pi: {e}")))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| CoreError::agent(role, "pi spawn had no stdout pipe"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| CoreError::agent(role, "pi spawn had no stderr pipe"))?;

        // Drained concurrently with stdout: both pipes have to be read while
        // the child is alive, or whichever we ignore fills and blocks it.
        let verbose = self.verbose;
        let drain = std::thread::spawn(move || {
            let mut tail: std::collections::VecDeque<String> = std::collections::VecDeque::new();
            for line in BufReader::new(stderr)
                .lines()
                .map_while(std::io::Result::ok)
            {
                if verbose {
                    eprintln!("{line}");
                }
                if tail.len() == STDERR_TAIL_LINES {
                    tail.pop_front();
                }
                tail.push_back(line);
            }
            tail.into_iter().collect::<Vec<_>>().join("\n")
        });

        // Parsing never fails the run on a bad line (see stream.rs); reading
        // stdout to EOF is exactly what tolerates a crash-truncated stream.
        let stream = parse_stream(BufReader::new(stdout))?;

        let status = child
            .wait()
            .map_err(|e| CoreError::agent(role, format!("failed waiting for pi: {e}")))?;
        let stderr_tail = drain.join().unwrap_or_default();

        Ok(SpawnOutcome {
            stream,
            exit_ok: status.success(),
            stderr_tail,
        })
    }
}

/// Append a spawn's stderr tail to a message, when it had anything to say.
/// The label matters: this is the subprocess talking, not the harness.
fn with_stderr(message: &str, stderr_tail: &str) -> String {
    if stderr_tail.trim().is_empty() {
        return message.to_string();
    }
    format!("{message}; pi stderr tail:\n{stderr_tail}")
}

impl AgentRunner for PiRunner {
    /// Spawn the worker, stream-parse its output, and return the proposal.
    ///
    /// Contract details that matter:
    /// - The `transition` call arrives as a `tool_execution_end` event whose
    ///   result text starts with `LOOP_TRANSITION `. We take that over the
    ///   tool-call args: the tool validates before echoing.
    /// - `usage` is summed off every `message_end` event for an assistant
    ///   message.
    /// - A worker that never calls `transition` is not an error here: we
    ///   return `proposal: None` and let the engine decide (it re-enters or
    ///   navigates).
    /// - `exit_ok` reflects the process exit code only; a non-zero exit
    ///   doesn't stop us from returning whatever partial summary/usage the
    ///   stream did contain before it was cut off.
    /// - `stderr_tail` travels beside them, because on a non-zero exit the
    ///   engine writes an `error` event and drops the summary entirely — the
    ///   tail is the only place a spawn failure can leave a diagnosis.
    fn run_worker(&self, spec: &WorkerSpec) -> Result<WorkerResult> {
        let cmd = command::worker_command(&self.pi_bin, spec);
        let out = self.spawn_and_parse(cmd, "worker")?;
        let proposal = out.stream.proposal()?;
        Ok(WorkerResult {
            summary: out.stream.summary,
            proposal,
            usage: out.stream.usage,
            session_id: out.stream.session_id,
            exit_ok: out.exit_ok,
            stderr_tail: out.stderr_tail,
        })
    }

    /// `--no-session --no-builtin-tools --no-extensions -e verdict-tool.ts`.
    /// The verdict arrives as `LOOP_VERDICT {…}`. A judge that returns
    /// nothing — no marker, a malformed one, or the process exiting non-zero
    /// — is a **fail**, not a pass: an unavailable grader must never wave
    /// work through (docs/05-design-notes.md).
    fn run_judge(&self, spec: &JudgeSpec) -> Result<Verdict> {
        let cmd = command::judge_command(&self.pi_bin, spec);
        let out = self.spawn_and_parse(cmd, "judge")?;

        if out.exit_ok {
            if let Some(payload) = out.stream.marker(LOOP_VERDICT_MARKER) {
                if let Ok(mut verdict) = serde_json::from_str::<Verdict>(payload) {
                    verdict.usage = out.stream.usage;
                    return Ok(verdict);
                }
            }
        }

        // The fail-closed rationale is what a human reads when a guard fails
        // for a reason that has nothing to do with the work, so it carries
        // whatever the spawn managed to say before giving up.
        Ok(Verdict {
            pass: false,
            rationale: with_stderr("judge returned no usable verdict", &out.stderr_tail),
            usage: out.stream.usage,
        })
    }

    /// `--no-session --no-builtin-tools --no-extensions -e choose-tool.ts`,
    /// with `LOOP_REACHABLE` exported so the `to` enum is constrained. A
    /// navigator that returns nothing — no marker, a malformed one, or a
    /// non-zero exit — escalates rather than stalling the run.
    fn run_navigator(&self, spec: &NavigatorSpec) -> Result<Choice> {
        let cmd = command::navigator_command(&self.pi_bin, spec);
        let out = self.spawn_and_parse(cmd, "navigator")?;

        if out.exit_ok {
            if let Some(payload) = out.stream.marker(LOOP_CHOICE_MARKER) {
                if let Ok(mut choice) = serde_json::from_str::<Choice>(payload) {
                    choice.usage = out.stream.usage;
                    return Ok(choice);
                }
            }
        }

        Ok(Choice {
            to: "escalate".to_string(),
            entry_prompt: Some(with_stderr(
                "the navigator spawn produced no usable choice, so the harness escalated",
                &out.stderr_tail,
            )),
            usage: out.stream.usage,
        })
    }
}

/// Where the transcript for a spawn lives, for the ledger to reference.
#[derive(Clone, Debug)]
pub struct SessionRef {
    pub id: String,
    pub path: Option<PathBuf>,
}
