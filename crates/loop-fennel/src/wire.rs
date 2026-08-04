//! The authored Fennel shape, as serde sees it.
//!
//! An evaluated `machine.fnl` is a Lua table. `mlua`'s serde support turns
//! that table into these structs, and [`crate::convert`] turns these into the
//! `loop-core` IR. Two layers rather than one because the authored shape and
//! the IR genuinely differ: `:playbook`/`:prompt` collapse into one
//! [`loop_core::PlaybookRef`], `:task` may name a file that has to be read,
//! and budgets get tightened against the config before anyone downstream sees
//! them.
//!
//! What is *not* here is as important. [`loop_core::OnFail`],
//! [`loop_core::OnExhausted`], [`loop_core::Thinking`], and
//! [`loop_core::QaCase`] already deserialize straight from the authored shape
//! — `"retry"`, `{:route "implement"}`, `"high"`, `{:id .. :desc ..}` all land
//! on the IR type with no wire struct in between. A wire type exists only
//! where a name is kebab-cased, a field is polymorphic, or the IR carries
//! something the author never writes.
//!
//! # Every struct here is `deny_unknown_fields`
//!
//! This is a deliberate change from the hand-written walker that preceded it,
//! which read the keys it knew and ignored the rest. `:playbok "implement"`
//! used to load fine and silently fall through to "needs either `:playbook`
//! or `:prompt`" — or worse, `:max-cycles` misspelled on a loop left the
//! bound at its default and the run just kept going. A typo in a machine file
//! is now an error that names the field and lists the ones that exist, which
//! is the same standard `:when`, `:context`, and `:transition-mode` are held
//! to in [`crate::convert`].

use std::collections::BTreeMap;

use serde::Deserialize;

use loop_core::{OnExhausted, OnFail, QaCase, Thinking};

/// `machine.fnl`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Machine {
    pub ticket: String,
    /// A path relative to the machine file, or inline prose. Resolved by
    /// `convert::resolve_prose`, which is why this is a bare `String` here.
    pub task: String,
    pub plan: String,
    #[serde(default)]
    pub qa_cases: Vec<QaCase>,
    #[serde(default)]
    pub defaults: Option<Defaults>,
    #[serde(default)]
    pub budgets: Option<Budgets>,
    #[serde(default)]
    pub judge: Option<ModelChoice>,
    #[serde(default)]
    pub navigator: Option<Navigator>,
    /// Optional only because a single-state machine may omit it; anything
    /// larger must say which state starts the run.
    #[serde(default)]
    pub entry: Option<String>,
    pub terminals: Vec<String>,
    #[serde(default)]
    pub escalation_state: Option<String>,
    pub states: BTreeMap<String, State>,
    #[serde(default)]
    pub transitions: Vec<Transition>,
    #[serde(default)]
    pub loops: Vec<Loop>,
    /// The provider every role falls back to. A role naming its own wins.
    #[serde(default)]
    pub provider: Option<String>,
    /// The Worker floor, under every state and playbook. Distinct from
    /// `:defaults`, which also carries skills and MCP.
    #[serde(default)]
    pub worker: Option<ModelChoice>,
    #[serde(default)]
    pub digest_last_n: Option<u32>,
    #[serde(default)]
    pub pi_extensions: Option<Vec<String>>,
}

/// A partial model selection. Every level of the four-layer chain writes the
/// same three keys, and all three are optional at every level — filling in
/// defaults here would defeat the layering, since the playbook frontmatter
/// layer sits between a state and the machine defaults.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ModelChoice {
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub thinking: Option<Thinking>,
}

impl ModelChoice {
    pub fn to_ir(&self) -> loop_core::ModelChoice {
        loop_core::ModelChoice {
            provider: self.provider.clone(),
            model: self.model.clone(),
            thinking: self.thinking,
        }
    }
}

/// The Navigator role: a model choice plus the one knob only it has.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Navigator {
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub thinking: Option<Thinking>,
    #[serde(default)]
    pub max_invocations: Option<u32>,
}

impl Navigator {
    pub fn to_ir(&self) -> loop_core::ModelChoice {
        loop_core::ModelChoice {
            provider: self.provider.clone(),
            model: self.model.clone(),
            thinking: self.thinking,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Budgets {
    #[serde(default)]
    pub usd: Option<f64>,
    #[serde(default)]
    pub wallclock_s: Option<u64>,
    #[serde(default)]
    pub max_transitions: Option<u32>,
}

impl Budgets {
    pub fn to_ir(&self) -> loop_core::Budgets {
        loop_core::Budgets {
            usd: self.usd,
            wallclock_s: self.wallclock_s,
            max_transitions: self.max_transitions,
        }
    }
}

/// Machine-level fallbacks: a model choice, plus skill and MCP sets that stack
/// under every state's own.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Defaults {
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub thinking: Option<Thinking>,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub mcp: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct State {
    /// A bare name resolved through the toolbox, or a `/`-containing path.
    /// Mutually exclusive with `prompt`; exactly one is required, which serde
    /// cannot express and `convert` checks.
    #[serde(default)]
    pub playbook: Option<String>,
    /// An inline prompt, for a stage too one-off to deserve a file.
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub thinking: Option<Thinking>,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub mcp: Vec<String>,
    /// What this stage is for. Worth writing: it is what the Navigator reads
    /// when it decides where a stuck run should go.
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Transition {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub check: Option<Check>,
    #[serde(default)]
    pub criteria: Option<String>,
    /// Absent means [`OnFail::Retry`], the IR's own default.
    #[serde(default)]
    pub on_fail: Option<OnFail>,
    #[serde(default)]
    pub backoff_s: Option<u64>,
}

/// `:check` is a bare command string in the common case, or a table when it
/// needs a non-default timeout. Untagged, so both spellings land here.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Check {
    Cmd(String),
    Table {
        cmd: String,
        #[serde(default, rename = "timeout-s")]
        timeout_s: Option<u64>,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Loop {
    pub name: String,
    /// `states[0]` is the loop head — the state whose re-entry counts a cycle.
    pub states: Vec<String>,
    pub max_cycles: u32,
    /// Absent means [`OnExhausted::Escalate`].
    #[serde(default)]
    pub on_exhausted: Option<OnExhausted>,
}
