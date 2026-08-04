//! The agent-spawning seam.
//!
//! [`crate::runner`] implements [`AgentRunner`] by spawning `pi`; the engine only
//! ever sees this trait, so the whole control loop is testable against an
//! in-process fake or the `mock-pi` fixture binary.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::core::error::Result;
use crate::core::event::{Artifact, Usage};
use crate::core::machine::{ModelSpec, StateId};

/// Everything a worker spawn needs. Assembled deterministically by the engine
/// from the state's config plus the rendered prompt files.
#[derive(Clone, Debug)]
pub struct WorkerSpec {
    pub state: StateId,
    pub cycle: u32,
    pub attempt: u32,
    pub model: ModelSpec,
    /// Resolved skill directories/files, passed as `--skill <path>`. The
    /// spawn also gets `--no-skills`, so this list is exactly what loads —
    /// nothing is picked up by ambient discovery.
    pub skill_paths: Vec<PathBuf>,
    // No `mcp`. There is no flag for it: the `mcp` extension starts every
    // session with every server off, and the only way in is the agent calling
    // `mcp({connect})` — so the list reaches the agent inside `entry_message`
    // and nowhere else. The names the ledger records travel on
    // [`crate::engine::StagePlan`], beside `skills`, which is the other
    // name list the spawn does not take either.
    /// Rendered stage prompt, passed as `--append-system-prompt @path`.
    pub system_prompt_path: PathBuf,
    /// The short positional kickoff message.
    pub entry_message: String,
    /// Where this spawn is told to write its proposal, exported as
    /// [`crate::core::HANDOFF_ENV`]. The harness reads it once the process exits.
    ///
    /// There is no list of pi-extensions to enable beside it: pi has no flag
    /// for that. A worker spawn simply omits `--no-extensions` and gets pi's
    /// ambient discovery, which is why the machine's `:pi-extensions` is a
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

/// `detail`, with what a spawn said on its way out appended when it said
/// anything.
///
/// One spelling of the suffix, because it is written from two layers: the
/// runner explains an off-contract reply, and the engine explains a crashed
/// stage. Both are the same sentence about the same subprocess, and the engine
/// only ever had its own copy because [`WorkerResult::stderr_tail`] is handed
/// over raw.
pub fn with_stderr_tail(mut detail: String, stderr_tail: &str) -> String {
    if !stderr_tail.trim().is_empty() {
        detail.push_str("; pi stderr tail:\n");
        detail.push_str(stderr_tail);
    }
    detail
}

/// A one-paragraph summary of a single stage's output, for the Judge — the
/// value that fills [`JudgeSpec::worker_digest`].
///
/// It must exclude the worker's own pass/fail claim: the Judge grades
/// artifacts, not self-assessment (docs/05-design-notes.md). The exclusion is
/// structural, not a filter — this takes `summary` (what `worker_output`
/// records the worker *did*) and the artifact list, never the
/// `transition_proposed.rationale`, which is exactly where a self-graded "QA
/// passed" would live. Callers must not paste one in themselves.
///
/// Lives here, beside the field it fills, rather than in `ledger` — it is a
/// pure function of a string and a slice, with no ledger, no filesystem and no
/// I/O anywhere in it. Being filed under `ledger` put it out of reach of
/// `engine`, which may import nothing but `core`, so the engine's own
/// [`crate::core::AgentRunner`] fake wrote a *second* version with different
/// separators and no `Artifacts:` header — and the engine tests then covered
/// that one instead of the digest a Judge actually receives.
pub fn worker_digest_for_judge(summary: &str, artifacts: &[Artifact]) -> String {
    let mut out = String::from(summary.trim());
    if !artifacts.is_empty() {
        out.push_str("\n\nArtifacts:\n");
        for a in artifacts {
            out.push_str(&format!("- {} ({})\n", a.name, a.path));
        }
    }
    out
}

/// The rationale the engine synthesizes when a Worker leaves no usable
/// handoff. It lands on the ledger and is spliced into the Navigator's prompt,
/// so it has to describe the protocol the Worker was actually given — it lives
/// here, in the module that owns [`Proposal`], because the engine reaches for
/// nothing but [`crate::core`].
pub const ABSENT_HANDOFF_RATIONALE: &str =
    "worker ended its turn without writing a usable handoff file";

/// The worker's proposal, as read back out of its handoff file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Proposal {
    pub to: Option<StateId>,
    #[serde(default)]
    pub blocked: bool,
    pub rationale: String,
    #[serde(default)]
    pub artifacts: Vec<Artifact>,
}

#[derive(Clone, Debug)]
pub struct WorkerResult {
    /// The final assistant text, trimmed — what goes in `worker_output.summary`.
    pub summary: String,
    /// `None` when the worker left no handoff file, or wrote one that isn't a
    /// usable [`Proposal`].
    pub proposal: Option<Proposal>,
    pub usage: Usage,
    /// False when pi exited non-zero.
    pub exit_ok: bool,
    /// The tail of the spawn's stderr. Empty when it said nothing. A crashed
    /// stage produces no `worker_output`, so without this a failed spawn would
    /// reach the ledger as an exit code and nothing else.
    pub stderr_tail: String,
}

/// What the Judge sees: the criteria, a digest of the worker's output, and
/// whatever the edge's deterministic check printed. Never the worker's
/// self-assessment of whether it passed.
#[derive(Clone, Debug)]
pub struct JudgeSpec {
    pub criteria: String,
    /// The worker's summary and the artifacts it produced, built by
    /// [`worker_digest_for_judge`]. There is no second artifact field
    /// beside it: this one already names every artifact, and a spec carrying
    /// the same list twice put the block in every Judge prompt twice, in two
    /// spellings.
    pub worker_digest: String,
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
    /// Combined stdout/stderr, truncated. Recorded on `guard_checked` and fed
    /// to the Judge.
    ///
    /// No `exit_code` beside it. It crossed this trait boundary to be read by
    /// nobody: `passed` already answers the only question the guard tier asks,
    /// a timeout says so in this text, and no event records a number.
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
    /// The stuck state's declared neighbours, and **only** those.
    ///
    /// Not the choice set: `runner::command::navigator_choices` appends the
    /// `escalate` sentinel to build that, and it is what both the prompt and
    /// the parser are given. This field is the graph's half of it.
    ///
    /// The distinction used to be stated three ways — this doc claimed the
    /// escalation state was included, `CliStage` passed the bare neighbours,
    /// and the engine's fake appended `machine.escalation_state` — so the
    /// engine's tests exercised a choice set no run ever produces.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The Judge grades artifacts, not self-assessment. The exclusion is
    /// structural — there is no parameter here a worker's self-graded verdict
    /// could enter through — so this documents that the summary text (which
    /// callers must draw from `worker_output`, never from a proposal's
    /// rationale) passes through unedited.
    #[test]
    fn worker_digest_excludes_pass_fail_self_assessment() {
        let artifacts = vec![Artifact {
            name: "report".into(),
            path: ".loop/artifacts/qa-1-report".into(),
        }];
        let digest = worker_digest_for_judge("Ran the QA suite against staging.", &artifacts);
        assert!(digest.contains("Ran the QA suite"));
        assert!(digest.contains("report"));

        let lower = digest.to_lowercase();
        assert!(!lower.contains("i passed") && !lower.contains("qa passed"));
    }

    /// One digest, so the engine's fake and the real `CliStage` produce the
    /// same bytes. The fake's own version had no `Artifacts:` header and
    /// joined with a single newline, so an engine test asserting on a Judge's
    /// input was asserting on something no Judge ever receives.
    #[test]
    fn the_artifact_block_is_headed_and_one_line_each() {
        let digest = worker_digest_for_judge(
            "  did the work  ",
            &[
                Artifact {
                    name: "diff".into(),
                    path: "d.patch".into(),
                },
                Artifact {
                    name: "report".into(),
                    path: "r.md".into(),
                },
            ],
        );
        assert_eq!(
            digest,
            "did the work\n\nArtifacts:\n- diff (d.patch)\n- report (r.md)\n"
        );
        // No artifacts, no block — not an empty heading.
        assert_eq!(worker_digest_for_judge("just prose", &[]), "just prose");
    }
}
