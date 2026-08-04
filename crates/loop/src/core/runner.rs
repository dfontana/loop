//! The agent-spawning seam.
//!
//! [`crate::runner`] implements [`AgentRunner`] by spawning `pi`; the engine only
//! ever sees this trait, so the whole control loop is testable against an
//! in-process fake or the `mock-pi` fixture binary.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::core::error::Result;
use crate::core::event::{ArtifactClaim, Usage};
use crate::core::machine::{ModelSpec, StateId};

/// Everything a worker spawn needs. Assembled deterministically by the engine
/// from the state's config plus the rendered prompt files.
#[derive(Clone, Debug)]
pub struct WorkerSpec {
    pub ticket: String,
    pub state: StateId,
    pub cycle: u32,
    pub attempt: u32,
    pub model: ModelSpec,
    /// Resolved skill directories/files, passed as `--skill <path>`. The
    /// spawn also gets `--no-skills`, so this list is exactly what loads —
    /// nothing is picked up by ambient discovery.
    pub skill_paths: Vec<PathBuf>,
    /// MCP servers this stage should reach, by name. There is no flag for
    /// this: the `mcp` extension starts every session with every server off,
    /// and the only way in is the agent calling `mcp({connect})`. So the list
    /// travels in the entry message as an instruction, and lives here for the
    /// ledger's record of what the stage was told to connect.
    pub mcp: Vec<String>,
    /// Rendered playbook, passed as `--append-system-prompt @path`.
    pub system_prompt_path: PathBuf,
    /// The short positional kickoff message.
    pub entry_message: String,
    /// Neighbors of the current state — the targets the handoff protocol
    /// block lists as valid. Advisory rather than enforced: the harness
    /// re-checks the proposed target against the graph either way, and an
    /// off-graph target routes to the Navigator.
    pub reachable: Vec<StateId>,
    /// Where this spawn is told to write its proposal, exported as
    /// [`crate::core::HANDOFF_ENV`]. The harness reads it once the process exits.
    ///
    /// There is no list of pi-extensions to enable beside it: pi has no flag
    /// for that. A worker spawn simply omits `--no-extensions` and gets pi's
    /// ambient discovery, which is why `config.fnl`'s `:pi-extensions` is a
    /// declaration the linter reads rather than a switch this struct carries.
    pub handoff_path: PathBuf,
    /// Where pi runs — the project root.
    pub cwd: PathBuf,
    /// Deterministic id, so a crashed stage's transcript is findable.
    pub session_id: Option<String>,
    /// Context-namespace values exported to the spawn, so a skill's script can
    /// read `$TICKET_ID` / `$CYCLE` and key its idempotency on them.
    pub env: Vec<(String, String)>,
}

/// The worker's proposal, as read back out of its handoff file.
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
    /// `None` when the worker left no handoff file, or wrote one that isn't a
    /// usable [`Proposal`].
    pub proposal: Option<Proposal>,
    pub usage: Usage,
    pub session_id: Option<String>,
    /// False when pi exited non-zero.
    pub exit_ok: bool,
    /// The tail of the spawn's stderr. Empty when it said nothing. A crashed
    /// stage produces no `worker_output`, so without this a failed spawn would
    /// reach the ledger as an exit code and nothing else.
    pub stderr_tail: String,
}

/// What the Judge sees: the criteria, a digest of the worker's output,
/// artifact paths, and whatever the edge's deterministic check printed. Never
/// the worker's self-assessment of whether it passed.
#[derive(Clone, Debug)]
pub struct JudgeSpec {
    pub criteria: String,
    pub worker_digest: String,
    pub artifact_paths: Vec<PathBuf>,
    /// Output of the edge's [`crate::core::Check`], when it has one. Unlike every
    /// other field here, the worker had no hand in producing it.
    pub check_output: Option<String>,
    pub model: ModelSpec,
    pub cwd: PathBuf,
}

/// The result of running a [`crate::core::Check`].
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
        check: &crate::core::machine::Check,
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
    /// The states it may pick from. Includes the escalation state; a reply
    /// naming anything else escalates.
    pub reachable: Vec<StateId>,
    pub model: ModelSpec,
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
