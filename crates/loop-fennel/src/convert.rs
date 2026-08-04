//! Lua table → `loop-core` IR.
//!
//! # The authored `machine.fnl` shape (v1)
//!
//! A machine file evaluates to a table. Keyword keys, kebab-case state ids.
//!
//! ```fennel
//! {:ticket "PROJ-1487"
//!  :task "task.md"                  ; path relative to machine.fnl, or inline prose
//!  :plan "plan.md"
//!  :qa-cases [{:id "pipeline" :desc "…"}
//!             {:id "contract" :desc "…"}]
//!
//!  :defaults {:model "claude-sonnet-5" :thinking "medium" :skills ["jj"] :mcp []}
//!  :budgets  {:usd 8 :wallclock-s 5400 :max-transitions 40}
//!  :judge     {:model "claude-haiku-4-5" :thinking "low"}
//!  :navigator {:model "claude-haiku-4-5" :thinking "low" :max-invocations 5}
//!
//!  :entry "implement"
//!  :terminals ["done" "blocked"]
//!  :escalation-state "blocked"
//!
//!  :states
//!  {:implement  {:playbook "implement" :thinking "high"
//!                :skills ["spark-build"]
//!                :description "Implement the plan; keep the build green."}
//!   :qa-staging {:playbook "qa" :thinking "high"
//!                :skills ["staging-deploy" "spark-run"]
//!                :mcp ["warehouse"]}}
//!
//!  :transitions
//!  [{:from "implement" :to "review"
//!    :criteria "The plan's items are addressed, the build is green, no TODOs remain."
//!    :on-fail "retry"}
//!   {:from "qa-staging" :to "qa-staging"
//!    :backoff-s 30 :on-fail "abort"}
//!   {:from "qa-staging" :to "debug"
//!    :check "sparkctl status --ns loop-$TICKET_ID-$CYCLE"}]
//!
//!  :loops
//!  [{:name "qa" :states ["qa-staging" "debug"] :max-cycles 4 :on-exhausted "escalate"}]}
//! ```
//!
//! The shape itself lives in [`crate::wire`] as serde structs; this module is
//! only what serde cannot do on its own. That split is the point: a field that
//! is merely *named* in Fennel and *stored* in the IR needs no code here at
//! all, so what remains is exactly the set of rules that are genuinely rules.
//!
//! Four kinds of thing survive:
//!
//! 1. **Removed keys.** `:when`, `:context`, and `:transition-mode` must fail
//!    with a message naming their replacement, which is more than
//!    `deny_unknown_fields` would say.
//! 2. **File resolution.** `:task`/`:plan` may name a file to read.
//! 3. **Cross-field rules.** A state needs exactly one of `:playbook`/
//!    `:prompt`; `:entry` may be omitted only for a single-state machine;
//!    every `:from`/`:to`/`:escalation-state` must name something declared.
//! 4. **Layering.** Budgets tighten against the config, and role models
//!    overlay it.
//!
//! Notes that matter for the conversion:
//! - `:check` is a bare command string, or `{:cmd .. :timeout-s ..}`. The
//!   harness runs it itself; exit 0 passes the edge.
//! - `:on-fail` is `"retry"` | `"abort"` | `{:route "state-id"}`.
//! - `:mcp` names servers in the **user's own** `mcp.json`, which loop never
//!   reads. The names ride into the entry message as `mcp({connect: …})`
//!   instructions, so a name that exists nowhere fails at connect time rather
//!   than at load — loop has nothing to check it against.
//! - `:model`/`:thinking`/`:provider` are all optional at every level — leave
//!   them `None` and let [`loop_core::Machine::resolve_model`] stack the layers.
//!   Do **not** eagerly fill in defaults here; the playbook frontmatter layer
//!   sits between the state and the machine defaults.
//! - Missing `:entry` defaults to the first key of `:states` only if there is
//!   exactly one; otherwise it is an error. Silent guessing is worse than a
//!   clear message.
//!
//! # `config.fnl`
//!
//! Same idea, matching [`loop_core::Config`]:
//!
//! ```fennel
//! {:provider "anthropic"
//!  :worker    {:model "claude-sonnet-5"  :thinking "medium"}
//!  :judge     {:model "claude-haiku-4-5" :thinking "low"}
//!  :navigator {:model "claude-haiku-4-5" :thinking "low" :max-invocations 5}
//!  :default-skills []
//!  :default-mcp []
//!  :pi-extensions ["mcp" "review-model-selector"]
//!  :budgets {:usd 15 :wallclock-s 7200 :max-transitions 60}
//!  :digest-last-n 8
//!  }
//! ```
//!
//! `:provider` is the base every role falls back to: a role table that names
//! its own wins, one that doesn't inherits this.
//!
//! Every key is optional; whatever is absent keeps its [`loop_core::Config::defaults`]
//! value. Kebab-case in Fennel maps to snake_case in Rust (`:max-invocations` →
//! `navigator_max_invocations`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use loop_core::{
    Check, Config, CoreError, DEFAULT_CHECK_TIMEOUT_S, Defaults, LoopSpec, Machine, ModelChoice,
    ModelSpec, OnExhausted, PlaybookRef, Result, State, StateId, Transition,
};

use crate::wire;

// ── deserialization ────────────────────────────────────────────────────────

/// Deserialize an evaluated Lua table into a wire struct, with the field path
/// in the error message.
///
/// `serde_path_to_error` is what makes this readable: mlua's own error is
/// `unknown variant \`wat\`` with no indication of *where*, which in a file
/// with fifteen states is not a diagnosis. Wrapped, the same failure reads
/// `states.qa-staging.thinking: unknown variant …, expected one of …` — which
/// is strictly more than the hand-written walker's `ctx` strings managed, and
/// it is generated rather than maintained.
fn from_table<T: serde::de::DeserializeOwned>(table: &mlua::Table, what: &str) -> Result<T> {
    let de = mlua::serde::Deserializer::new(mlua::Value::Table(table.clone()));
    serde_path_to_error::deserialize(de).map_err(|e| {
        let path = e.path().to_string();
        // A failure at the document root has the path ".", which reads as
        // noise next to the message; anywhere else the path is the useful half.
        if path == "." {
            CoreError::machine(format!("{what}: {}", e.inner()))
        } else {
            CoreError::machine(format!("{what}: at `{path}`: {}", e.inner()))
        }
    })
}

// ── removed keys ───────────────────────────────────────────────────────────
//
// `deny_unknown_fields` would already reject each of these, but only with
// "unknown field". A key that used to do something deserves to be told what
// replaced it, so these three run first and win.

fn get_value(table: &mlua::Table, key: &str) -> Result<mlua::Value> {
    table
        .get::<mlua::Value>(key)
        .map_err(|e| CoreError::machine(format!("reading `:{key}`: {e}")))
}

fn present(table: &mlua::Table, key: &str) -> Result<bool> {
    Ok(get_value(table, key)? != mlua::Value::Nil)
}

/// `:when` was a Fennel closure gating an edge on ledger vars. Both tiers are
/// gone; ignoring the key would silently leave the edge unguarded.
fn reject_when(table: &mlua::Table) -> Result<()> {
    let Some(transitions) = table
        .get::<mlua::Value>("transitions")
        .ok()
        .and_then(|v| match v {
            mlua::Value::Table(t) => Some(t),
            _ => None,
        })
    else {
        return Ok(());
    };
    for (i, item) in transitions.sequence_values::<mlua::Value>().enumerate() {
        let Ok(mlua::Value::Table(t)) = item else {
            continue;
        };
        if present(&t, "when")? {
            return Err(CoreError::machine(format!(
                "transitions[{i}]: `:when` guards were removed — express the condition as a \
                 `:check` command the harness runs, or as `:criteria` for the Judge to evaluate"
            )));
        }
    }
    Ok(())
}

/// `:transition-mode` chose between two schemas for the injected `transition`
/// tool's `to` parameter. There is no injected tool any more — a Worker writes
/// a handoff file — so the key selects between nothing and nothing.
fn reject_transition_mode(table: &mlua::Table, ctx: &str) -> Result<()> {
    if present(table, "transition-mode")? {
        return Err(CoreError::machine(format!(
            "{ctx}: `:transition-mode` was removed — a worker now ends its stage by writing \
             its proposal to `$LOOP_HANDOFF`, and the harness checks the target against the \
             graph either way. An off-graph target routes to the Navigator, which is what \
             `open` used to mean and is now the only behaviour"
        )));
    }
    Ok(())
}

/// `:context` was a two-valued knob whose second value was never wired to
/// anything.
fn reject_context(table: &mlua::Table) -> Result<()> {
    if present(table, "context")? {
        return Err(CoreError::machine(
            "config: `:context` was removed — the rolling digest is the only continuity channel \
             between stages; interpolate `$LEDGER_DIGEST` in a playbook and tune `:digest-last-n`"
                .to_string(),
        ));
    }
    Ok(())
}

// ── the rules serde cannot express ─────────────────────────────────────────

/// `:task`/`:plan`: a path relative to `machine_dir` if it resolves to a real
/// file, else inline prose — unless it looks like it was meant to be a file
/// (`.md` suffix), in which case a non-resolving path is an authoring error,
/// not a silent fallback to prose.
fn resolve_prose(raw: String, field: &'static str, machine_dir: &Path) -> Result<String> {
    let candidate = machine_dir.join(&raw);
    if candidate.is_file() {
        std::fs::read_to_string(&candidate).map_err(|e| {
            CoreError::io(
                format!("reading `:{field}` file {}", candidate.display()),
                e,
            )
        })
    } else if raw.ends_with(".md") {
        Err(CoreError::Unresolved {
            kind: field,
            name: raw,
            searched: vec![candidate],
        })
    } else {
        Ok(raw)
    }
}

/// `:playbook` (bare name or `/`-containing path) or a state-local `:prompt`.
/// Exactly one is required — a cross-field rule serde has no way to state.
fn playbook_ref(state: &wire::State, state_id: &str) -> Result<PlaybookRef> {
    match (&state.playbook, &state.prompt) {
        (Some(p), _) if p.contains('/') => Ok(PlaybookRef::Path(PathBuf::from(p))),
        (Some(p), _) => Ok(PlaybookRef::Named(p.clone())),
        (None, Some(prompt)) => Ok(PlaybookRef::Inline(prompt.clone())),
        (None, None) => Err(CoreError::machine(format!(
            "state `{state_id}`: needs either `:playbook` or `:prompt`"
        ))),
    }
}

/// An empty command is an authoring mistake rather than "no check": the key is
/// present, so the author believed they had gated the edge.
fn check_ir(check: &wire::Check, ctx: &str) -> Result<Check> {
    let (cmd, timeout_s) = match check {
        wire::Check::Cmd(cmd) => (cmd.clone(), None),
        wire::Check::Table { cmd, timeout_s } => (cmd.clone(), *timeout_s),
    };
    if cmd.trim().is_empty() {
        return Err(CoreError::machine(format!(
            "{ctx}: `:check` command is empty — omit the key instead"
        )));
    }
    Ok(Check {
        cmd,
        timeout_s: timeout_s.unwrap_or(DEFAULT_CHECK_TIMEOUT_S),
    })
}

/// Missing `:entry` is only unambiguous for a one-state machine.
fn resolve_entry(entry: Option<String>, states: &BTreeMap<StateId, State>) -> Result<StateId> {
    match entry {
        Some(e) => {
            if !states.contains_key(&e) {
                return Err(CoreError::machine(format!(
                    "`:entry` `{e}` is not a declared state"
                )));
            }
            Ok(e)
        }
        None if states.len() == 1 => Ok(states.keys().next().expect("len == 1").clone()),
        None => Err(CoreError::machine(format!(
            "missing `:entry` and `:states` has {} entries; ambiguous which one starts the machine",
            states.len()
        ))),
    }
}

// ── entry points ───────────────────────────────────────────────────────────

/// Convert an evaluated machine table into the IR, resolving `:task`/`:plan`
/// paths against `machine_dir`.
pub fn machine_from_table(
    table: &mlua::Table,
    machine_dir: &Path,
    source_hash: String,
    source_path: &Path,
    config: &Config,
) -> Result<Machine> {
    reject_transition_mode(table, "machine")?;
    reject_when(table)?;
    let m: wire::Machine = from_table(table, "machine")?;

    let task = resolve_prose(m.task, "task", machine_dir)?;
    let plan = resolve_prose(m.plan, "plan", machine_dir)?;

    let mut states = BTreeMap::new();
    for (id, st) in &m.states {
        states.insert(
            id.clone(),
            State {
                id: id.clone(),
                playbook: playbook_ref(st, id)?,
                model: ModelChoice {
                    provider: st.provider.clone(),
                    model: st.model.clone(),
                    thinking: st.thinking,
                },
                skills: st.skills.clone(),
                mcp: st.mcp.clone(),
                description: st.description.clone(),
            },
        );
    }
    if states.is_empty() {
        return Err(CoreError::machine(
            "`:states` must declare at least one state",
        ));
    }

    let entry = resolve_entry(m.entry, &states)?;
    let terminals: BTreeSet<StateId> = m.terminals.into_iter().collect();

    if let Some(esc) = &m.escalation_state {
        if !states.contains_key(esc) && !terminals.contains(esc) {
            return Err(CoreError::machine(format!(
                "`:escalation-state` `{esc}` is not a declared state or terminal"
            )));
        }
    }

    let mut transitions = Vec::with_capacity(m.transitions.len());
    for (i, t) in m.transitions.iter().enumerate() {
        let ctx = format!("transitions[{i}]");
        if !states.contains_key(&t.from) {
            return Err(CoreError::machine(format!(
                "transition from `{}`: not a declared state",
                t.from
            )));
        }
        if !states.contains_key(&t.to) && !terminals.contains(&t.to) {
            return Err(CoreError::machine(format!(
                "transition to `{}`: not a declared state or terminal",
                t.to
            )));
        }
        transitions.push(Transition {
            from: t.from.clone(),
            to: t.to.clone(),
            check: t.check.as_ref().map(|c| check_ir(c, &ctx)).transpose()?,
            criteria: t.criteria.clone(),
            on_fail: t.on_fail.clone().unwrap_or_default(),
            backoff_s: t.backoff_s,
        });
    }

    let mut loops = Vec::with_capacity(m.loops.len());
    for (i, l) in m.loops.into_iter().enumerate() {
        if l.states.is_empty() {
            return Err(CoreError::machine(format!(
                "loops[{i}]: `:states` must not be empty"
            )));
        }
        loops.push(LoopSpec {
            name: l.name,
            states: l.states,
            max_cycles: l.max_cycles,
            on_exhausted: l.on_exhausted.unwrap_or(OnExhausted::Escalate),
        });
    }

    let defaults = m
        .defaults
        .map(|d| Defaults {
            model: ModelChoice {
                provider: d.provider,
                model: d.model,
                thinking: d.thinking,
            },
            skills: d.skills,
            mcp: d.mcp,
        })
        .unwrap_or_default();

    // A machine may *tighten* the global budgets, never loosen them.
    let budgets = m
        .budgets
        .map(|b| b.to_ir())
        .unwrap_or_default()
        .tighten(config.budgets);

    let judge = overlay(
        m.judge.as_ref().map(wire::ModelChoice::to_ir),
        &config.judge,
    );
    let navigator = overlay(
        m.navigator.as_ref().map(wire::Navigator::to_ir),
        &config.navigator,
    );
    let navigator_max_invocations = m
        .navigator
        .as_ref()
        .and_then(|n| n.max_invocations)
        .unwrap_or(config.navigator_max_invocations);

    Ok(Machine {
        ticket: m.ticket,
        task,
        plan,
        qa_cases: m.qa_cases,
        entry,
        terminals,
        escalation_state: m.escalation_state,
        defaults,
        states,
        transitions,
        loops,
        budgets,
        judge,
        navigator,
        navigator_max_invocations,
        source_hash,
        source_path: source_path.to_path_buf(),
        dir: machine_dir.to_path_buf(),
    })
}

/// Overlay an authored partial model choice onto a fully-specified base.
fn overlay(choice: Option<ModelChoice>, base: &ModelSpec) -> ModelSpec {
    match choice {
        None => base.clone(),
        Some(c) => c.resolve(base),
    }
}

/// Overlay an evaluated config table onto [`Config::defaults`].
pub fn config_from_table(table: &mlua::Table, base: Config) -> Result<Config> {
    reject_transition_mode(table, "config")?;
    reject_context(table)?;
    let c: wire::Config = from_table(table, "config")?;

    let provider = c.provider.unwrap_or(base.provider);
    // The top-level `:provider` is the base of all three role chains, not a
    // stored-and-ignored default: it is applied *under* each role table, so a
    // role naming its own still wins and a toolbox switching providers only
    // has to say so once.
    let with_provider = |spec: &ModelSpec| ModelSpec {
        provider: provider.clone(),
        ..spec.clone()
    };

    let budgets = match c.budgets {
        None => base.budgets,
        Some(b) => loop_core::Budgets {
            usd: b.usd.or(base.budgets.usd),
            wallclock_s: b.wallclock_s.or(base.budgets.wallclock_s),
            max_transitions: b.max_transitions.or(base.budgets.max_transitions),
        },
    };

    Ok(Config {
        worker: overlay(
            c.worker.as_ref().map(wire::ModelChoice::to_ir),
            &with_provider(&base.worker),
        ),
        judge: overlay(
            c.judge.as_ref().map(wire::ModelChoice::to_ir),
            &with_provider(&base.judge),
        ),
        navigator: overlay(
            c.navigator.as_ref().map(wire::Navigator::to_ir),
            &with_provider(&base.navigator),
        ),
        navigator_max_invocations: c
            .navigator
            .as_ref()
            .and_then(|n| n.max_invocations)
            .unwrap_or(base.navigator_max_invocations),
        default_skills: c.default_skills.unwrap_or(base.default_skills),
        default_mcp: c.default_mcp.unwrap_or(base.default_mcp),
        pi_extensions: c.pi_extensions.unwrap_or(base.pi_extensions),
        budgets,
        digest_last_n: c
            .digest_last_n
            .map(|n| n as usize)
            .unwrap_or(base.digest_last_n),
        provider,
        pi_bin: base.pi_bin,
        paths: base.paths,
    })
}
