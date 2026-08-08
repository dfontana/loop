//! The canonical machine IR.
//!
//! [`crate::fennel`] produces one of these from `machine.fnl`; every other
//! module consumes it. Nothing here knows about Lua, YAML, or the filesystem — a
//! `Machine` is fully resolved by the time it exists, with defaults applied and
//! prose (`task`, `plan`) already read into memory.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub type StateId = String;

/// pi's thinking levels, spelled exactly as `--model model:LEVEL` takes them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Thinking {
    Off,
    Minimal,
    Low,
    #[default]
    Medium,
    High,
    Xhigh,
    Max,
}

impl Thinking {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
}

impl fmt::Display for Thinking {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Provider + model + thinking, resolved. Renders to pi's CLI flags.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSpec {
    pub provider: String,
    pub model: String,
    pub thinking: Thinking,
}

impl ModelSpec {
    /// The value for `pi --model` — model and thinking travel as one token.
    pub fn pi_model_arg(&self) -> String {
        format!("{}:{}", self.model, self.thinking)
    }
}

/// Where a stage's prompt comes from. Resolution happens in `toolbox`;
/// this only records what the author wrote.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StagePromptRef {
    /// A bare name, resolved to `./.loop/stage-prompts/<name>.md`.
    Named(String),
    /// A value containing `/`: taken as an exact path, relative to the machine file.
    Path(PathBuf),
    /// An inline `:prompt` string on the state.
    Inline(String),
}

impl StagePromptRef {
    /// Which kind of reference an authored `:stage-prompt` value is.
    ///
    /// The `/` rule is [`crate::core::names_a_path`], shared with skill
    /// resolution rather than spelled out here — the two kinds offer the same
    /// escape hatch and must agree on what triggers it.
    pub fn parse(value: &str) -> Self {
        if crate::core::names_a_path(value) {
            Self::Path(PathBuf::from(value))
        } else {
            Self::Named(value.to_string())
        }
    }
}

/// A partially-specified model choice: whatever one layer of config declared.
///
/// Four layers stack, most specific first: the **state**, the **stage prompt's
/// frontmatter**, the **machine's `defaults`**, and loop's **built-in floor**.
/// Only the last is guaranteed complete, so resolution happens in
/// [`Machine::resolve_model`] — not at load time, since the stage prompt layer isn't
/// readable until `toolbox` has resolved the file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelChoice {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub thinking: Option<Thinking>,
}

impl ModelChoice {
    /// Fill any empty field from `lower`. Self wins field-by-field.
    pub fn over(self, lower: &ModelChoice) -> ModelChoice {
        ModelChoice {
            provider: self.provider.or_else(|| lower.provider.clone()),
            model: self.model.or_else(|| lower.model.clone()),
            thinking: self.thinking.or(lower.thinking),
        }
    }

    /// Complete the choice against a fully-specified base.
    pub fn resolve(self, base: &ModelSpec) -> ModelSpec {
        ModelSpec {
            provider: self.provider.unwrap_or_else(|| base.provider.clone()),
            model: self.model.unwrap_or_else(|| base.model.clone()),
            thinking: self.thinking.unwrap_or(base.thinking),
        }
    }
}

/// Machine-level fallbacks that sit under every state.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Defaults {
    pub model: ModelChoice,
    /// Loaded into every stage, on top of whatever the state names.
    pub skills: Vec<String>,
    /// MCP servers connected in every stage, on top of the state's own.
    pub mcp: Vec<String>,
}

/// A node in the graph.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct State {
    pub id: StateId,
    pub stage_prompt: StagePromptRef,
    /// The state's own overrides, as authored. Resolve with
    /// [`Machine::resolve_model`].
    pub model: ModelChoice,
    /// Skills loaded into this stage, by name. Resolved to paths by
    /// `toolbox` and passed to pi as `--skill`. Resolve the *set* with
    /// [`Machine::resolve_skills`].
    pub skills: Vec<String>,
    /// MCP servers this stage should reach, by the name they carry in the
    /// user's own `mcp.json`. loop neither reads nor ships that file; it only
    /// names servers. Resolve the *set* with [`Machine::resolve_mcp`].
    pub mcp: Vec<String>,
    /// One line on what this stage is for. Fed to the Navigator so it can route.
    pub description: Option<String>,
}

/// What to do when a guard on a proposed transition fails.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OnFail {
    /// Re-enter the *source* state for another attempt.
    #[default]
    Retry,
    /// Give up: `run_finished{status: failed}`.
    Abort,
    /// Send the run to a named state instead.
    Route(StateId),
}

/// A deterministic check the **harness** runs for itself before letting a
/// transition through.
///
/// This is the one signal in the system a worker cannot author. It is not run
/// by the agent, not routed through the agent's session, and not scraped off
/// the agent's transcript: the CLI shells out, and the exit code decides.
/// Anything the worker touches — its summary, its claimed artifacts, its
/// proposal — is evidence for the Judge to weigh, never a gate on its own.
///
/// Exit 0 passes. Any other exit fails the edge, and `on_fail` decides what
/// happens next. Combined stdout/stderr is recorded on the `guard_checked`
/// event and handed to the Judge, so a criterion can be phrased against what
/// the check actually printed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    /// Run via `bash -c` in the project root. `$UPPER_SNAKE` context
    /// placeholders are substituted before it runs.
    pub cmd: String,
    pub timeout_s: u64,
}

/// The default check timeout. Generous enough for a compile, short enough
/// that a wedged command doesn't stall the run for long.
pub const DEFAULT_CHECK_TIMEOUT_S: u64 = 120;

impl Check {
    pub fn new(cmd: impl Into<String>) -> Self {
        Self {
            cmd: cmd.into(),
            timeout_s: DEFAULT_CHECK_TIMEOUT_S,
        }
    }
}

/// An edge. Three guard tiers, checked cheapest-first: structural (does this
/// edge exist), `check` (a deterministic command the harness runs), and
/// `criteria` (an LLM Judge).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Transition {
    pub from: StateId,
    pub to: StateId,
    pub check: Option<Check>,
    pub criteria: Option<String>,
    pub on_fail: OnFail,
    /// Sleep this long before re-entering the target (transient retry self-loops).
    pub backoff_s: Option<u64>,
    /// How many times the source state may run against this edge in one cycle
    /// before the run escalates. Always at least 1 — the first attempt is an
    /// attempt, not a retry.
    ///
    /// This is the only bound on [`OnFail::Retry`]. `max_cycles` cannot serve:
    /// a retry commits no transition, so a loop head's cycle counter never
    /// advances past 1 no matter how many times the stage re-runs.
    pub max_attempts: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OnExhausted {
    /// Route to the machine's escalation state (a `blocked` terminal) and notify.
    #[default]
    Escalate,
    /// `run_finished{status: aborted}`.
    Abort,
}

/// A bounded cycle. `states[0]` is the loop head — the state whose re-entry
/// increments the cycle counter.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LoopSpec {
    pub name: String,
    pub states: Vec<StateId>,
    pub max_cycles: u32,
    pub on_exhausted: OnExhausted,
}

impl LoopSpec {
    pub fn head(&self) -> Option<&StateId> {
        self.states.first()
    }
}

/// Hard stops the harness enforces — never the agent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Budgets {
    pub usd: Option<f64>,
    pub wallclock_s: Option<u64>,
    pub max_transitions: Option<u32>,
}

impl Budgets {
    /// Take the tighter of each field. A machine may tighten global budgets,
    /// never loosen them.
    ///
    /// An absent bound is "unbounded", so it loses to any present one — which
    /// is why this is not `Option::min` (that would make `None` win on `usd`,
    /// where `f64` has no `Ord` anyway).
    pub fn tighten(self, other: Budgets) -> Budgets {
        fn tighter<T: PartialOrd>(a: Option<T>, b: Option<T>) -> Option<T> {
            match (a, b) {
                (Some(a), Some(b)) if b < a => Some(b),
                (Some(a), _) => Some(a),
                (None, b) => b,
            }
        }
        Budgets {
            usd: tighter(self.usd, other.usd),
            wallclock_s: tighter(self.wallclock_s, other.wallclock_s),
            max_transitions: tighter(self.max_transitions, other.max_transitions),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QaCase {
    pub id: String,
    pub desc: String,
}

/// One ticket's fully-resolved state machine.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Machine {
    pub ticket: String,
    /// Contents of `task.md`, already read.
    pub task: String,
    /// Contents of `plan.md`, already read.
    pub plan: String,
    pub qa_cases: Vec<QaCase>,

    pub entry: StateId,
    pub terminals: BTreeSet<StateId>,
    /// Where `on_exhausted: escalate` and a capped Navigator send the run.
    pub escalation_state: Option<StateId>,
    /// Machine-level fallbacks, under every state and over the global config.
    pub defaults: Defaults,

    pub states: BTreeMap<StateId, State>,
    pub transitions: Vec<Transition>,
    pub loops: Vec<LoopSpec>,

    pub budgets: Budgets,
    /// The Worker floor: what a state gets when neither it, its stage prompt, nor
    /// `:defaults` names a model.
    pub worker: ModelSpec,
    pub judge: ModelSpec,
    pub navigator: ModelSpec,
    pub navigator_max_invocations: u32,
    /// How many recent committed transitions the rolling digest lists.
    pub digest_last_n: usize,
    /// What the author has installed in pi, declared so `loop validate` can
    /// catch a mismatch. This turns nothing on — pi has no flag for enabling
    /// an installed extension by name.
    pub pi_extensions: Vec<String>,

    /// sha256 of the machine source, recorded in `run_started`.
    pub source_hash: String,
    pub source_path: PathBuf,
    /// Directory the machine file lives in; relative refs resolve against it.
    pub dir: PathBuf,
}

impl Machine {
    pub fn state(&self, id: &str) -> Option<&State> {
        self.states.get(id)
    }

    /// Stack the four config layers: state → stage prompt frontmatter → machine
    /// defaults → global config.
    pub fn resolve_model(&self, state: &State, stage_prompt: &ModelChoice) -> ModelSpec {
        state
            .model
            .clone()
            .over(stage_prompt)
            .over(&self.defaults.model)
            .resolve(&self.worker)
    }

    /// The machine defaults unioned with the state's own skills, and likewise
    /// its MCP servers: order-preserving, deduplicated, defaults first.
    ///
    /// Both settings are purely additive, and there is no exclude list for
    /// either. A skill is a prompt plus a script the agent runs through bash —
    /// loading one grants nothing bash did not already grant, so "removing"
    /// one would only hide instructions, never a capability. What a stage may
    /// *do* is decided by the tools pi gives it, not here.
    ///
    /// MCP names come out of the *user's* `mcp.json`, which loop never reads.
    /// A stage cannot reach a server it doesn't name, because the `mcp`
    /// extension starts every session with all servers off — but that also
    /// means loop cannot tell a typo from a server this machine simply has
    /// installed and loop doesn't, so a name that doesn't exist fails at
    /// connect time rather than at `loop validate`.
    pub fn resolve_skills(&self, state: &State) -> Vec<String> {
        union(&self.defaults.skills, &state.skills)
    }

    /// See [`Machine::resolve_skills`] — the same union, over `:mcp`.
    pub fn resolve_mcp(&self, state: &State) -> Vec<String> {
        union(&self.defaults.mcp, &state.mcp)
    }

    pub fn is_terminal(&self, id: &str) -> bool {
        self.terminals.contains(id)
    }

    /// Whether `id` names anything this machine declared — a state or a
    /// terminal.
    ///
    /// The predicate every reference check wants, and the one the loader and
    /// the linter each used to write out inline: `convert` spelled it three
    /// times while validating `:escalation-state` and both ends of every
    /// transition, and `validate` bound it as a local closure. A third kind of
    /// declared name is one edit here rather than six spread over two modules.
    pub fn declares(&self, id: &str) -> bool {
        self.states.contains_key(id) || self.is_terminal(id)
    }

    /// Every edge out of `from`, in declaration order.
    pub fn edges_from(&self, from: &str) -> Vec<&Transition> {
        self.transitions.iter().filter(|t| t.from == from).collect()
    }

    /// The distinct states reachable in one hop — the targets the handoff
    /// protocol block lists for a Worker, and the Navigator's choice set.
    ///
    /// Declaration order, first occurrence kept: this is [`crate::core::dedup`]
    /// over [`Machine::edges_from`], not a fifth hand-rolled copy of the loop
    /// that function was extracted to replace.
    pub fn neighbors(&self, from: &str) -> Vec<StateId> {
        crate::core::dedup(self.edges_from(from).into_iter().map(|t| t.to.clone()))
    }

    /// The edge `from -> to`. First declared wins: a machine may name the same
    /// pair twice, and `validate` reports the duplicate rather than this
    /// silently preferring one — so the tie-break is `find`, not `filter`.
    pub fn edge(&self, from: &str, to: &str) -> Option<&Transition> {
        self.transitions
            .iter()
            .find(|t| t.from == from && t.to == to)
    }

    /// The loop whose head is `state`, if any — used to bump cycle counters.
    pub fn loop_with_head(&self, state: &str) -> Option<&LoopSpec> {
        self.loops
            .iter()
            .find(|l| l.head().is_some_and(|h| h == state))
    }
}

/// Concatenate the two layers of a purely-additive setting, keeping the first
/// occurrence of each name so the machine's baseline stays at the front.
fn union(machine: &[String], state: &[String]) -> Vec<String> {
    crate::core::dedup(machine.iter().chain(state).cloned())
}
