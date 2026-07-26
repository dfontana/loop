//! Spawning `pi` and reading its JSON event stream.
//!
//! See docs/05-orchestration.md. Three roles, three cost profiles: the Worker
//! does the stage's work and ends with `transition`; the Judge independently
//! grades a `criteria`; the Navigator reroutes a blocked worker. All three are
//! `pi --print --mode json` subprocesses whose newline-delimited events this
//! crate parses.
//!
//! TASK T4 implements this crate (and `mock-pi`, which lets everything else be
//! tested without an API key).

use std::io::BufReader;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use loop_core::{
    AgentRunner, Choice, Config, CoreError, JudgeSpec, LOOP_CHOICE_MARKER, LOOP_VERDICT_MARKER,
    NavigatorSpec, Result, Verdict, WorkerResult, WorkerSpec,
};

pub mod command;
pub mod stream;

pub use stream::{StreamOutcome, parse_stream};

/// Spawns real `pi` subprocesses.
pub struct PiRunner {
    pi_bin: String,
    /// Streamed to stderr as the run proceeds, so a human watching `loop run`
    /// sees the worker working. Off in tests.
    pub verbose: bool,
}

impl PiRunner {
    pub fn new(config: &Config) -> Self {
        Self {
            pi_bin: config.pi_bin.clone(),
            verbose: false,
        }
    }

    pub fn pi_bin(&self) -> &str {
        &self.pi_bin
    }

    /// Spawn `cmd`, stream-parse its stdout, and wait for it to exit.
    ///
    /// stdin is closed (these are non-interactive `--print` spawns); stderr
    /// is inherited when `verbose` so a human watching `loop run` sees
    /// progress, and discarded otherwise — never piped-but-unread, which
    /// risks a deadlock if pi ever writes enough to fill the pipe buffer.
    fn spawn_and_parse(&self, mut cmd: Command, role: &str) -> Result<(StreamOutcome, bool)> {
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(if self.verbose {
            Stdio::inherit()
        } else {
            Stdio::null()
        });

        let mut child = cmd
            .spawn()
            .map_err(|e| CoreError::agent(role, format!("failed to spawn pi: {e}")))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| CoreError::agent(role, "pi spawn had no stdout pipe"))?;

        // Parsing never fails the run on a bad line (see stream.rs); reading
        // stdout to EOF is exactly what tolerates a crash-truncated stream.
        let outcome = parse_stream(BufReader::new(stdout))?;

        let status = child
            .wait()
            .map_err(|e| CoreError::agent(role, format!("failed waiting for pi: {e}")))?;

        Ok((outcome, status.success()))
    }
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
    fn run_worker(&self, spec: &WorkerSpec) -> Result<WorkerResult> {
        let cmd = command::worker_command(&self.pi_bin, spec);
        let (outcome, exit_ok) = self.spawn_and_parse(cmd, "worker")?;
        let proposal = outcome.proposal()?;
        Ok(WorkerResult {
            summary: outcome.summary,
            proposal,
            usage: outcome.usage,
            session_id: outcome.session_id,
            exit_ok,
        })
    }

    /// `--no-session --no-builtin-tools --no-extensions -e verdict-tool.ts`.
    /// The verdict arrives as `LOOP_VERDICT {…}`. A judge that returns
    /// nothing — no marker, a malformed one, or the process exiting non-zero
    /// — is a **fail**, not a pass: an unavailable grader must never wave
    /// work through (docs/07-risks.md #1).
    fn run_judge(&self, spec: &JudgeSpec) -> Result<Verdict> {
        let cmd = command::judge_command(&self.pi_bin, spec);
        let (outcome, exit_ok) = self.spawn_and_parse(cmd, "judge")?;

        if exit_ok {
            if let Some(payload) = outcome.marker(LOOP_VERDICT_MARKER) {
                if let Ok(mut verdict) = serde_json::from_str::<Verdict>(payload) {
                    verdict.usage = outcome.usage;
                    return Ok(verdict);
                }
            }
        }

        Ok(Verdict {
            pass: false,
            rationale: "judge returned no usable verdict".to_string(),
            usage: outcome.usage,
        })
    }

    /// `--no-session --no-builtin-tools --no-extensions -e choose-tool.ts`,
    /// with `LOOP_REACHABLE` exported so the `to` enum is constrained. A
    /// navigator that returns nothing — no marker, a malformed one, or a
    /// non-zero exit — escalates rather than stalling the run.
    fn run_navigator(&self, spec: &NavigatorSpec) -> Result<Choice> {
        let cmd = command::navigator_command(&self.pi_bin, spec);
        let (outcome, exit_ok) = self.spawn_and_parse(cmd, "navigator")?;

        if exit_ok {
            if let Some(payload) = outcome.marker(LOOP_CHOICE_MARKER) {
                if let Ok(mut choice) = serde_json::from_str::<Choice>(payload) {
                    choice.usage = outcome.usage;
                    return Ok(choice);
                }
            }
        }

        Ok(Choice {
            to: "escalate".to_string(),
            entry_prompt: None,
            usage: outcome.usage,
        })
    }
}

/// Where the transcript for a spawn lives, for the ledger to reference.
#[derive(Clone, Debug)]
pub struct SessionRef {
    pub id: String,
    pub path: Option<PathBuf>,
}
