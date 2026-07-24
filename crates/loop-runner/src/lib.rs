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

use std::path::PathBuf;

use loop_core::{
    AgentRunner, Choice, Config, JudgeSpec, NavigatorSpec, Result, Verdict, WorkerResult,
    WorkerSpec,
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
}

impl AgentRunner for PiRunner {
    /// Spawn the worker, stream-parse its output, and return the proposal.
    ///
    /// TASK T4. Contract details that matter:
    /// - The `transition` call arrives as a `tool_execution_end` event whose
    ///   result text starts with `LOOP_TRANSITION `. Prefer that over the
    ///   tool-call args: the tool validates before echoing.
    /// - `LOOP_VARS {…}` lines appear in *any* tool's result text. Scrape every
    ///   one, in order, deep-merging as you go — these are the trusted vars.
    /// - Sum `usage` off `message_end` events for assistant messages.
    /// - A worker that never calls `transition` is not an error here: return
    ///   `proposal: None` and let the engine decide (it re-enters or navigates).
    /// - Never fail the whole run on one unparseable line; skip it.
    fn run_worker(&self, spec: &WorkerSpec) -> Result<WorkerResult> {
        let _ = spec;
        todo!("T4")
    }

    /// TASK T4. `--no-session --no-builtin-tools -e verdict-tool.ts`. The
    /// verdict arrives as `LOOP_VERDICT {…}`. A judge that returns nothing is a
    /// **fail**, not a pass — an unavailable grader must not wave work through.
    fn run_judge(&self, spec: &JudgeSpec) -> Result<Verdict> {
        let _ = spec;
        todo!("T4")
    }

    /// TASK T4. `--no-session --no-builtin-tools -e choose-tool.ts`, with
    /// `LOOP_REACHABLE` exported so the `to` enum is constrained. A navigator
    /// that returns nothing escalates.
    fn run_navigator(&self, spec: &NavigatorSpec) -> Result<Choice> {
        let _ = spec;
        todo!("T4")
    }
}

/// Where the transcript for a spawn lives, for the ledger to reference.
#[derive(Clone, Debug)]
pub struct SessionRef {
    pub id: String,
    pub path: Option<PathBuf>,
}
