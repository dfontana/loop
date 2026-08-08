//! The authored Fennel shape, as serde sees it.
//!
//! An evaluated `machine.fnl` is a Lua table. `mlua`'s serde support turns
//! that table into these structs, and [`crate::fennel::convert`] turns these into the
//! [`crate::core`] IR. Two layers rather than one because the authored shape and
//! the IR genuinely differ: `:stage-prompt`/`:prompt` collapse into one
//! [`crate::core::StagePromptRef`], `:task` may name a file that has to be read,
//! and budgets get tightened against the config before anyone downstream sees
//! them.
//!
//! What is *not* here is as important. [`crate::core::OnFail`],
//! [`crate::core::OnExhausted`], [`crate::core::Thinking`], and
//! [`crate::core::QaCase`] already deserialize straight from the authored shape
//! — `"retry"`, `{:route "implement"}`, `"high"`, `{:id .. :desc ..}` all land
//! on the IR type with no wire struct in between. A wire type exists only
//! where a name is kebab-cased, a field is polymorphic, or the IR carries
//! something the author never writes.
//!
//! # Every struct here is `deny_unknown_fields`
//!
//! This is a deliberate change from the hand-written walker that preceded it,
//! which read the keys it knew and ignored the rest. `:playbok "implement"`
//! used to load fine and silently fall through to "needs either `:stage-prompt`
//! or `:prompt`" — or worse, `:max-cycles` misspelled on a loop left the
//! bound at its default and the run just kept going. A typo in a machine file
//! is now an error that names the field and lists the ones that exist, which
//! is the same standard `:when`, `:context`, and `:transition-mode` are held
//! to in [`crate::fennel::convert`].

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::core::{OnExhausted, OnFail, QaCase, Thinking};

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
    /// The Worker floor, under every state and stage prompt. Distinct from
    /// `:defaults`, which also carries skills and MCP.
    #[serde(default)]
    pub worker: Option<ModelChoice>,
    #[serde(default)]
    pub digest_last_n: Option<u32>,
    #[serde(default)]
    pub pi_extensions: Option<Vec<String>>,
}

/// Declare the `provider`/`model`/`thinking` triple, plus whatever else a
/// struct carries, and derive its conversion to an IR
/// [`crate::core::ModelChoice`].
///
/// Three wire types spell that triple — `ModelChoice`, `Defaults` and `State` —
/// because `#[serde(flatten)]` cannot be combined with `deny_unknown_fields`,
/// and this module's whole premise is that every struct keeps the latter. A
/// macro is the way to keep one declaration when serde will not: it was three
/// hand-written copies, two `to_ir` impls and a free function, none of which the
/// compiler could relate to each other, so a fourth knob was four edits.
macro_rules! model_keys {
    (
        $(#[$meta:meta])*
        pub struct $name:ident { $($(#[$fmeta:meta])* pub $field:ident: $ty:ty),* $(,)? }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Default, Deserialize)]
        #[serde(rename_all = "kebab-case", deny_unknown_fields)]
        pub struct $name {
            #[serde(default)]
            pub provider: Option<String>,
            #[serde(default)]
            pub model: Option<String>,
            #[serde(default)]
            pub thinking: Option<Thinking>,
            $($(#[$fmeta])* #[serde(default)] pub $field: $ty,)*
        }

        impl $name {
            pub fn to_ir(&self) -> crate::core::ModelChoice {
                crate::core::ModelChoice {
                    provider: self.provider.clone(),
                    model: self.model.clone(),
                    thinking: self.thinking,
                }
            }
        }
    };
}

model_keys! {
    /// A partial model selection. Every level of the four-layer chain writes
    /// the same three keys, and all three are optional at every level —
    /// filling in defaults here would defeat the layering, since the stage
    /// prompt frontmatter layer sits between a state and the machine defaults.
    pub struct ModelChoice {}
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
    pub fn to_ir(&self) -> crate::core::Budgets {
        crate::core::Budgets {
            usd: self.usd,
            wallclock_s: self.wallclock_s,
            max_transitions: self.max_transitions,
        }
    }
}

model_keys! {
    /// The Navigator role: a model choice plus the one knob only it has.
    ///
    /// The survey suggested moving `:max-invocations` to a top-level key so
    /// this struct could collapse into `ModelChoice`. It does not need to: the
    /// macro takes extra fields, so the authored grouping stays where an author
    /// would look for it and the duplication goes anyway.
    pub struct Navigator {
        pub max_invocations: Option<u32>,
    }
}

model_keys! {
    /// Machine-level fallbacks: a model choice, plus skill and MCP sets that
    /// stack under every state's own.
    pub struct Defaults {
        pub skills: Vec<String>,
        pub mcp: Vec<String>,
    }
}

model_keys! {
    pub struct State {
        /// A bare name resolved through the toolbox, or a `/`-containing path.
        /// Mutually exclusive with `prompt`; exactly one is required, which
        /// serde cannot express and `convert` checks.
        pub stage_prompt: Option<String>,
        /// An inline prompt, for a stage too one-off to deserve a file.
        pub prompt: Option<String>,
        pub skills: Vec<String>,
        pub mcp: Vec<String>,
        /// What this stage is for. Worth writing: it is what the Navigator
        /// reads when it decides where a stuck run should go.
        pub description: Option<String>,
    }
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
    /// Absent means [`crate::core::Floor::transition_max_attempts`].
    #[serde(default)]
    pub max_attempts: Option<u32>,
}

/// `:check` is a bare command string in the common case, or a table when it
/// needs a non-default timeout.
///
/// Hand-written rather than `#[serde(untagged)]`, and the reason is the whole
/// point of this module. `untagged` tries each variant and, on failure,
/// reports only "data did not match any variant" — so `:check {:cmd "true"
/// :timeut-s 300}` would be *rejected* but not *explained*, on precisely the
/// key whose job is to stop a slow check being killed early. Dispatching on
/// the Lua value's own shape lets [`CheckTable`]'s `deny_unknown_fields` error
/// through intact, naming the misspelled field and listing the real ones.
#[derive(Debug)]
pub enum Check {
    Cmd(String),
    Table(CheckTable),
}

impl<'de> Deserialize<'de> for Check {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = Check;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a command string, or a table with `:cmd` and optional `:timeout-s`")
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> std::result::Result<Check, E> {
                Ok(Check::Cmd(v.to_string()))
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                map: A,
            ) -> std::result::Result<Check, A::Error> {
                CheckTable::deserialize(serde::de::value::MapAccessDeserializer::new(map))
                    .map(Check::Table)
            }
        }
        d.deserialize_any(V)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct CheckTable {
    pub cmd: String,
    #[serde(default)]
    pub timeout_s: Option<u64>,
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
