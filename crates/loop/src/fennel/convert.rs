//! Lua table → [`crate::core`] IR.
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
//!  :provider "anthropic"             ; the base under every role
//!  :defaults {:model "claude-sonnet-5" :thinking "medium" :skills ["jj"] :mcp []}
//!  :budgets  {:usd 8 :wallclock-s 5400 :max-transitions 40}
//!  :worker    {:model "claude-sonnet-5"  :thinking "medium"}
//!  :judge     {:model "claude-haiku-4-5" :thinking "low"}
//!  :navigator {:model "claude-haiku-4-5" :thinking "low" :max-invocations 5}
//!  :pi-extensions ["mcp"]
//!  :digest-last-n 8
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
//! The shape itself lives in [`crate::fennel::wire`] as serde structs; this module is
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
//! 4. **Layering.** Budgets tighten against loop's built-in floor, and role
//!    models overlay it.
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
//!   them `None` and let [`crate::core::Machine::resolve_model`] stack the layers.
//!   Do **not** eagerly fill in defaults here; the playbook frontmatter layer
//!   sits between the state and the machine defaults.
//! - Missing `:entry` defaults to the first key of `:states` only if there is
//!   exactly one; otherwise it is an error. Silent guessing is worse than a
//!   clear message.
//!
//! # Where `config.fnl` went
//!
//! There used to be a second authored file, `~/.config/loop/config.fnl`,
//! holding `:provider`, `:worker`, `:judge`, `:navigator`, `:default-skills`,
//! `:default-mcp`, `:pi-extensions`, `:budgets`, and `:digest-last-n`. Every
//! one of those was a value a machine could already override, so the file was
//! a second place to look for an answer the machine gave anyway.
//!
//! Those keys are machine keys now (`:default-skills`/`:default-mcp` folded
//! into the `:defaults` a machine already had), and what a machine does not
//! name comes from [`crate::core::Config::defaults`] — loop's built-in floor,
//! which is not a file. Carrying preferences between tickets is `loop init
//! --from <dir>`, which copies a `.loop/` you keep somewhere.
//!
//! Leftover config-only keys are rejected by name rather than by
//! `deny_unknown_fields`, so an author is told where the tier went.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::core::{
    Check, Config, CoreError, DEFAULT_CHECK_TIMEOUT_S, Defaults, LoopSpec, Machine, ModelChoice,
    ModelSpec, OnExhausted, PlaybookRef, Result, State, StateId, Transition,
};

use crate::fennel::wire;

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
fn from_table<T: serde::de::DeserializeOwned>(table: &mlua::Table) -> Result<T> {
    let de = mlua::serde::Deserializer::new(mlua::Value::Table(table.clone()));
    serde_path_to_error::deserialize(de).map_err(|e| {
        let path = e.path().to_string();
        // A failure at the document root has the path ".", which reads as
        // noise next to the message; anywhere else the path is the useful half.
        // No "machine:" prefix here — `CoreError::machine` adds one, and this
        // used to add a second, so every load error read `machine: machine:`.
        if path == "." {
            CoreError::machine(e.inner().to_string())
        } else {
            CoreError::machine(format!("at `{path}`: {}", e.inner()))
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

/// Keys that only ever made sense in `config.fnl`, which no longer exists.
///
/// `:context` was a two-valued knob whose second value was never wired to
/// anything. `:default-skills` and `:default-mcp` were the config's spelling
/// of what a machine writes as `:defaults {:skills .. :mcp ..}` — one baseline
/// tier rather than two, now that there is only one file.
fn reject_removed_config_key(table: &mlua::Table) -> Result<()> {
    if present(table, "context")? {
        return Err(CoreError::machine(
            "`:context` was removed — the rolling digest is the only continuity channel between \
             stages; interpolate `$LEDGER_DIGEST` in a playbook and tune `:digest-last-n`"
                .to_string(),
        ));
    }
    for (key, replacement) in [
        ("default-skills", ":defaults {:skills [..]}"),
        ("default-mcp", ":defaults {:mcp [..]}"),
    ] {
        if present(table, key)? {
            return Err(CoreError::machine(format!(
                "`:{key}` was a `config.fnl` key, and config.fnl was merged into the machine — \
                 write `{replacement}` instead"
            )));
        }
    }
    Ok(())
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
fn reject_transition_mode(table: &mlua::Table) -> Result<()> {
    if present(table, "transition-mode")? {
        return Err(CoreError::machine(
            "`:transition-mode` was removed — a worker now ends its stage by writing \
             its proposal to `$LOOP_HANDOFF`, and the harness checks the target against the \
             graph either way. An off-graph target routes to the Navigator, which is what \
             `open` used to mean and is now the only behaviour",
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
        wire::Check::Table(t) => (t.cmd.clone(), t.timeout_s),
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
    reject_transition_mode(table)?;
    reject_removed_config_key(table)?;
    reject_when(table)?;
    let m: wire::Machine = from_table(table)?;

    let task = resolve_prose(m.task, "task", machine_dir)?;
    let plan = resolve_prose(m.plan, "plan", machine_dir)?;

    let mut states = BTreeMap::new();
    for (id, st) in &m.states {
        states.insert(
            id.clone(),
            State {
                id: id.clone(),
                playbook: playbook_ref(st, id)?,
                model: wire::model_choice(&st.provider, &st.model, st.thinking),
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
            model: wire::model_choice(&d.provider, &d.model, d.thinking),
            skills: d.skills,
            mcp: d.mcp,
        })
        .unwrap_or_default();

    // A machine may *tighten* the built-in budgets, never loosen them. The
    // ceiling is loop's own rather than a user file's now, which makes it a
    // weaker guarantee than it reads as — but a machine that wants more can
    // still only get it by saying so, in the file under review.
    let budgets = m
        .budgets
        .map(|b| b.to_ir())
        .unwrap_or_default()
        .tighten(config.budgets);

    // The top-level `:provider` is the base of all three role chains, not a
    // stored-and-ignored default: it is applied *under* each role, so a role
    // naming its own still wins and switching providers is one line.
    let with_provider = |spec: &ModelSpec| match &m.provider {
        None => spec.clone(),
        Some(p) => ModelSpec {
            provider: p.clone(),
            ..spec.clone()
        },
    };
    let worker = overlay(
        m.worker.as_ref().map(wire::ModelChoice::to_ir),
        &with_provider(&config.worker),
    );
    let judge = overlay(
        m.judge.as_ref().map(wire::ModelChoice::to_ir),
        &with_provider(&config.judge),
    );
    let navigator = overlay(
        m.navigator.as_ref().map(wire::Navigator::to_ir),
        &with_provider(&config.navigator),
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
        worker,
        judge,
        navigator,
        navigator_max_invocations,
        digest_last_n: m
            .digest_last_n
            .map(|n| n as usize)
            .unwrap_or(config.digest_last_n),
        pi_extensions: m
            .pi_extensions
            .unwrap_or_else(|| config.pi_extensions.clone()),
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
