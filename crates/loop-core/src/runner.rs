//! The agent-spawning seam.
//!
//! `loop-runner` implements [`AgentRunner`] by spawning `pi`; the engine only
//! ever sees this trait, so the whole control loop is testable against an
//! in-process fake or the `mock-pi` fixture binary.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::event::{ArtifactClaim, Usage};
use crate::machine::{ModelSpec, StateId, TransitionMode};

/// Everything a worker spawn needs. Assembled deterministically by the engine
/// from the state's config plus the rendered prompt files.
#[derive(Clone, Debug)]
pub struct WorkerSpec {
    pub ticket: String,
    pub state: StateId,
    pub cycle: u32,
    pub attempt: u32,
    pub model: ModelSpec,
    /// The `--tools` allowlist, `transition` already included.
    pub tools: Vec<String>,
    /// Maps to `--exclude-tools`.
    pub exclude_tools: Vec<String>,
    /// Rendered playbook, passed as `--append-system-prompt @path`.
    pub system_prompt_path: PathBuf,
    /// The short positional kickoff message.
    pub entry_message: String,
    /// Neighbors of the current state — the `transition` tool's `to` enum.
    pub reachable: Vec<StateId>,
    pub transition_mode: TransitionMode,
    /// Exported as `PI_AGENT_DIR`.
    pub agent_dir: PathBuf,
    /// `-e` paths: loop's own vendored ext (`transition-tool.ts`).
    pub ext_paths: Vec<PathBuf>,
    /// Installed pi-extension names to keep enabled.
    pub pi_extensions: Vec<String>,
    /// Where pi runs — the project root.
    pub cwd: PathBuf,
    /// Deterministic id, so a crashed stage's transcript is findable.
    pub session_id: Option<String>,
    /// Context-namespace values exported to the spawn (so `valueFromCmd` in a
    /// scoped-tool can read `$TICKET_ID` / `$CYCLE`).
    pub env: Vec<(String, String)>,
}

/// The worker's `transition` tool call, as parsed off the event stream.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Proposal {
    pub to: Option<StateId>,
    #[serde(default)]
    pub blocked: bool,
    pub rationale: String,
    #[serde(default)]
    pub artifacts: Vec<ArtifactClaim>,
}

#[derive(Clone, Debug)]
pub struct WorkerResult {
    /// The final assistant text, trimmed — what goes in `worker_output.summary`.
    pub summary: String,
    /// `None` when the worker ended its turn without calling `transition`.
    pub proposal: Option<Proposal>,
    pub usage: Usage,
    pub session_id: Option<String>,
    /// False when pi exited non-zero.
    pub exit_ok: bool,
}

/// What the Judge sees: the criteria, a digest of the worker's output,
/// artifact paths, and whatever the edge's deterministic check printed. Never
/// the worker's self-assessment of whether it passed.
#[derive(Clone, Debug)]
pub struct JudgeSpec {
    pub criteria: String,
    pub worker_digest: String,
    pub artifact_paths: Vec<PathBuf>,
    /// Output of the edge's [`crate::Check`], when it has one. Unlike every
    /// other field here, the worker had no hand in producing it.
    pub check_output: Option<String>,
    pub model: ModelSpec,
    pub ext_path: PathBuf,
    pub cwd: PathBuf,
}

/// The result of running a [`crate::Check`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckOutcome {
    /// Exit status 0.
    pub passed: bool,
    /// `None` when the check was killed (timeout) or died on a signal.
    pub exit_code: Option<i32>,
    /// Combined stdout/stderr, truncated. Recorded on `guard_checked` and fed
    /// to the Judge.
    pub output: String,
}

/// Runs a transition's deterministic check.
///
/// Separate from [`AgentRunner`] on purpose: a check is the harness acting on
/// its own behalf, in its own subprocess, with no agent anywhere in the path.
/// The implementation substitutes the context namespace into the command and
/// supplies the working directory and environment.
pub trait CheckRunner {
    fn run_check(
        &self,
        check: &crate::machine::Check,
        from: &StateId,
        cycle: u32,
        attempt: u32,
    ) -> Result<CheckOutcome>;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Verdict {
    pub pass: bool,
    pub rationale: String,
    #[serde(default)]
    pub usage: Usage,
}

#[derive(Clone, Debug)]
pub struct NavigatorSpec {
    /// The graph, each state's purpose, and the edges out of `from`.
    pub graph_summary: String,
    pub ledger_digest: String,
    pub from: StateId,
    pub proposal: Option<Proposal>,
    /// The enum the `choose` tool is constrained to. Includes the escalation state.
    pub reachable: Vec<StateId>,
    pub model: ModelSpec,
    pub ext_path: PathBuf,
    pub cwd: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Choice {
    pub to: StateId,
    /// An addendum appended to the target state's entry message.
    pub entry_prompt: Option<String>,
    #[serde(default)]
    pub usage: Usage,
}

pub trait AgentRunner {
    fn run_worker(&self, spec: &WorkerSpec) -> Result<WorkerResult>;
    fn run_judge(&self, spec: &JudgeSpec) -> Result<Verdict>;
    fn run_navigator(&self, spec: &NavigatorSpec) -> Result<Choice>;
}
