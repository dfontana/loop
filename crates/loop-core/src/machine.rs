//! The canonical machine IR.
//!
//! `loop-fennel` produces one of these from `machine.fnl`; every other crate
//! consumes it. Nothing here knows about Lua, YAML, or the filesystem — a
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

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "off" => Self::Off,
            "minimal" => Self::Minimal,
            "low" => Self::Low,
            "medium" => Self::Medium,
            "high" => Self::High,
            "xhigh" => Self::Xhigh,
            "max" => Self::Max,
            _ => return None,
        })
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

/// Where a stage's prompt comes from. Resolution (local-first, then toolbox)
/// happens in `loop-toolbox`; this only records what the author wrote.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybookRef {
    /// A bare name: resolved `./.loop/playbooks/<name>.md`, then
    /// `~/.config/loop/playbooks/<name>.md`.
    Named(String),
    /// A value containing `/`: taken as an exact path, relative to the machine file.
    Path(PathBuf),
    /// An inline `:prompt` string on the state.
    Inline(String),
}

/// A partially-specified model choice: whatever one layer of config declared.
///
/// Four layers stack, most specific first: the **state**, the **playbook's
/// frontmatter**, the **machine's `defaults`**, and the global **config**. Only
/// the last is guaranteed complete, so resolution happens in
/// [`Machine::resolve_model`] — not at load time, since the playbook layer isn't
/// readable until `loop-toolbox` has resolved the file.
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

    pub fn is_empty(&self) -> bool {
        self.provider.is_none() && self.model.is_none() && self.thinking.is_none()
    }
}

/// Machine-level fallbacks that sit under every state.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Defaults {
    pub model: ModelChoice,
    /// Added to every state's allowlist.
    pub tools: Vec<String>,
    pub exclude_tools: Vec<String>,
}

/// A node in the graph.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct State {
    pub id: StateId,
    pub playbook: PlaybookRef,
    /// The state's own overrides, as authored. Resolve with
    /// [`Machine::resolve_model`].
    pub model: ModelChoice,
    /// The state's own allowlist, as authored. Resolve with
    /// [`Machine::resolve_tools`].
    pub tools: Vec<String>,
    /// Maps to pi's `--exclude-tools`.
    pub exclude_tools: Vec<String>,
    /// One line on what this stage is for. Fed to the Navigator so it can route.
    pub description: Option<String>,
}

/// An opaque handle to a guard closure held in `loop-fennel`'s Lua registry.
///
/// The engine passes it back to a [`crate::GuardEvaluator`] rather than linking
/// `mlua` itself — that seam is what keeps the control loop testable with plain
/// Rust fakes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GuardRef(pub u32);

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

/// An edge. The three guard tiers are checked cheapest-first: structural (does
/// this edge exist), `when` (a Fennel closure over ledger vars), `criteria` (an
/// LLM Judge).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Transition {
    pub from: StateId,
    pub to: StateId,
    pub when: Option<GuardRef>,
    /// Human-readable form of the `when` guard, for the ledger and `validate`.
    pub when_src: Option<String>,
    pub criteria: Option<String>,
    pub on_fail: OnFail,
    /// Sleep this long before re-entering the target (transient retry self-loops).
    pub backoff_s: Option<u64>,
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
    pub fn tighten(self, other: Budgets) -> Budgets {
        fn min_opt<T: PartialOrd>(a: Option<T>, b: Option<T>) -> Option<T> {
            match (a, b) {
                (Some(a), Some(b)) => Some(if a < b { a } else { b }),
                (Some(a), None) => Some(a),
                (None, b) => b,
            }
        }
        Budgets {
            usd: min_opt(self.usd, other.usd),
            wallclock_s: min_opt(self.wallclock_s, other.wallclock_s),
            max_transitions: min_opt(self.max_transitions, other.max_transitions),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QaCase {
    pub id: String,
    pub desc: String,
}

/// How the injected `transition` tool declares its `to` parameter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TransitionMode {
    /// `to` is an enum of the current state's neighbors — the worker cannot
    /// name an invalid edge. The Navigator fires only on explicit `blocked`.
    #[default]
    Constrained,
    /// `to` is a free string; unknown targets route to the Navigator.
    Open,
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
    pub judge: ModelSpec,
    pub navigator: ModelSpec,
    pub navigator_max_invocations: u32,
    pub transition_mode: TransitionMode,

    /// sha256 of the machine source, pinned into `run_started`.
    pub source_hash: String,
    pub source_path: PathBuf,
    /// Directory the machine file lives in; relative refs resolve against it.
    pub dir: PathBuf,
}

impl Machine {
    pub fn state(&self, id: &str) -> Option<&State> {
        self.states.get(id)
    }

    /// Stack the four config layers: state → playbook frontmatter → machine
    /// defaults → global config.
    pub fn resolve_model(
        &self,
        state: &State,
        playbook: &ModelChoice,
        config_default: &ModelSpec,
    ) -> ModelSpec {
        state
            .model
            .clone()
            .over(playbook)
            .over(&self.defaults.model)
            .resolve(config_default)
    }

    /// Union of the global baseline, the machine defaults, and the state's own
    /// allowlist, order-preserving and deduplicated. `transition` is always
    /// appended — a worker that cannot end its stage is a hung run.
    pub fn resolve_tools(&self, state: &State, config_default: &[String]) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for t in config_default
            .iter()
            .chain(self.defaults.tools.iter())
            .chain(state.tools.iter())
            .chain(std::iter::once(&"transition".to_string()))
        {
            if !out.iter().any(|x| x == t) {
                out.push(t.clone());
            }
        }
        let excluded =
            |t: &String| state.exclude_tools.contains(t) || self.defaults.exclude_tools.contains(t);
        out.retain(|t| t == "transition" || !excluded(t));
        out
    }

    pub fn is_terminal(&self, id: &str) -> bool {
        self.terminals.contains(id)
    }

    /// Every edge out of `from`, in declaration order.
    pub fn edges_from(&self, from: &str) -> Vec<&Transition> {
        self.transitions.iter().filter(|t| t.from == from).collect()
    }

    /// The distinct states reachable in one hop — the `transition` tool's enum.
    pub fn neighbors(&self, from: &str) -> Vec<StateId> {
        let mut seen = BTreeSet::new();
        self.transitions
            .iter()
            .filter(|t| t.from == from)
            .map(|t| t.to.clone())
            .filter(|to| seen.insert(to.clone()))
            .collect()
    }

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

    /// Every loop that contains `state`.
    pub fn loops_containing(&self, state: &str) -> Vec<&LoopSpec> {
        self.loops
            .iter()
            .filter(|l| l.states.iter().any(|s| s == state))
            .collect()
    }
}
