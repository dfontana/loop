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
//!  :defaults {:model "claude-sonnet-5" :thinking "medium" :tools ["read" "bash"]}
//!  :budgets  {:usd 8 :wallclock-s 5400 :max-transitions 40}
//!  :judge     {:model "claude-haiku-4-5" :thinking "low"}
//!  :navigator {:model "claude-haiku-4-5" :thinking "low" :max-invocations 5}
//!
//!  :entry "implement"
//!  :terminals ["done" "blocked"]
//!  :escalation-state "blocked"
//!  :transition-mode "constrained"   ; or "open"
//!
//!  :states
//!  {:implement  {:playbook "implement" :thinking "high"
//!                :tools ["edit" "write" "spark_build"]
//!                :description "Implement the plan; keep the build green."}
//!   :qa-staging {:playbook "qa" :thinking "high"
//!                :tools ["staging_deploy" "spark_run" "fetch_job_output"]
//!                :exclude-tools ["edit" "write"]}}
//!
//!  :transitions
//!  [{:from "implement" :to "review"
//!    :criteria "The plan's items are addressed, the build is green, no TODOs remain."
//!    :on-fail "retry"}
//!   {:from "qa-staging" :to "qa-staging"
//!    :backoff-s 30 :on-fail "abort"}
//!   {:from "qa-staging" :to "debug"}]
//!
//!  :loops
//!  [{:name "qa" :states ["qa-staging" "debug"] :max-cycles 4 :on-exhausted "escalate"}]}
//! ```
//!
//! Notes that matter for the conversion:
//! - `:on-fail` is `"retry"` | `"abort"` | `{:route "state-id"}`.
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
//!  :default-tools ["read" "bash"]
//!  :pi-extensions ["scoped-tools" "mcp" "review-model-selector"]
//!  :budgets {:usd 15 :wallclock-s 7200 :max-transitions 60}
//!  :context "digest" :digest-last-n 8
//!  :transition-mode "constrained"}
//! ```
//!
//! Every key is optional; whatever is absent keeps its [`loop_core::Config::defaults`]
//! value. Kebab-case in Fennel maps to snake_case in Rust (`:max-invocations` →
//! `navigator_max_invocations`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use loop_core::{
    Budgets, Check, Config, ContextMode, CoreError, DEFAULT_CHECK_TIMEOUT_S, Defaults, LoopSpec,
    Machine, ModelChoice, ModelSpec, OnExhausted, OnFail, PlaybookRef, QaCase, Result, State,
    StateId, Thinking, Transition, TransitionMode,
};

// ── small Lua table readers ────────────────────────────────────────────────

fn get_value(table: &mlua::Table, key: &str) -> Result<mlua::Value> {
    table
        .get::<mlua::Value>(key)
        .map_err(|e| CoreError::machine(format!("reading `:{key}`: {e}")))
}

fn get_table(table: &mlua::Table, key: &str) -> Result<Option<mlua::Table>> {
    match get_value(table, key)? {
        mlua::Value::Nil => Ok(None),
        mlua::Value::Table(t) => Ok(Some(t)),
        other => Err(CoreError::machine(format!(
            "`:{key}` must be a table, got {}",
            other.type_name()
        ))),
    }
}

fn get_str(table: &mlua::Table, key: &str) -> Result<Option<String>> {
    match get_value(table, key)? {
        mlua::Value::Nil => Ok(None),
        mlua::Value::String(s) => Ok(Some(s.to_string_lossy())),
        other => Err(CoreError::machine(format!(
            "`:{key}` must be a string, got {}",
            other.type_name()
        ))),
    }
}

fn require_str(table: &mlua::Table, key: &str, ctx: &str) -> Result<String> {
    get_str(table, key)?.ok_or_else(|| CoreError::machine(format!("{ctx}: missing `:{key}`")))
}

fn get_f64(table: &mlua::Table, key: &str) -> Result<Option<f64>> {
    match get_value(table, key)? {
        mlua::Value::Nil => Ok(None),
        mlua::Value::Integer(i) => Ok(Some(i as f64)),
        mlua::Value::Number(n) => Ok(Some(n)),
        other => Err(CoreError::machine(format!(
            "`:{key}` must be a number, got {}",
            other.type_name()
        ))),
    }
}

fn get_u64(table: &mlua::Table, key: &str) -> Result<Option<u64>> {
    match get_f64(table, key)? {
        None => Ok(None),
        Some(n) if n >= 0.0 => Ok(Some(n.round() as u64)),
        Some(_) => Err(CoreError::machine(format!("`:{key}` must not be negative"))),
    }
}

fn get_u32(table: &mlua::Table, key: &str) -> Result<Option<u32>> {
    match get_f64(table, key)? {
        None => Ok(None),
        Some(n) if n >= 0.0 => Ok(Some(n.round() as u32)),
        Some(_) => Err(CoreError::machine(format!("`:{key}` must not be negative"))),
    }
}

/// A list-of-strings field. `None` when the key is absent; an error if
/// present but not a sequence of strings.
fn get_str_vec(table: &mlua::Table, key: &str) -> Result<Option<Vec<String>>> {
    match get_value(table, key)? {
        mlua::Value::Nil => Ok(None),
        mlua::Value::Table(t) => {
            let mut out = Vec::new();
            for (i, item) in t.sequence_values::<mlua::Value>().enumerate() {
                let item = item.map_err(|e| CoreError::machine(format!("`:{key}`[{i}]: {e}")))?;
                match item {
                    mlua::Value::String(s) => out.push(s.to_string_lossy()),
                    other => {
                        return Err(CoreError::machine(format!(
                            "`:{key}`[{i}] must be a string, got {}",
                            other.type_name()
                        )));
                    }
                }
            }
            Ok(Some(out))
        }
        other => Err(CoreError::machine(format!(
            "`:{key}` must be a list of strings, got {}",
            other.type_name()
        ))),
    }
}

fn parse_thinking(s: &str, ctx: &str) -> Result<Thinking> {
    Thinking::parse(s)
        .ok_or_else(|| CoreError::machine(format!("{ctx}: unknown `:thinking` value `{s}`")))
}

fn parse_model_choice(table: &mlua::Table, ctx: &str) -> Result<ModelChoice> {
    let provider = get_str(table, "provider")?;
    let model = get_str(table, "model")?;
    let thinking = match get_str(table, "thinking")? {
        Some(s) => Some(parse_thinking(&s, ctx)?),
        None => None,
    };
    Ok(ModelChoice {
        provider,
        model,
        thinking,
    })
}

/// Overlay an optional `{key: {...}}` sub-table (a partial `ModelChoice`) onto
/// a fully-specified `base` `ModelSpec`. Used for the machine's `:judge`
/// /`:navigator` over `config`'s, and for `config.fnl`'s `:worker`/`:judge`
/// /`:navigator` over [`Config::defaults`]'s.
fn model_spec_overlay(table: &mlua::Table, key: &str, base: &ModelSpec) -> Result<ModelSpec> {
    match get_table(table, key)? {
        None => Ok(base.clone()),
        Some(t) => {
            let choice = parse_model_choice(&t, &format!("`:{key}`"))?;
            Ok(choice.resolve(base))
        }
    }
}

fn parse_transition_mode(value: Option<String>, default: TransitionMode) -> Result<TransitionMode> {
    match value {
        None => Ok(default),
        Some(s) => match s.as_str() {
            "constrained" => Ok(TransitionMode::Constrained),
            "open" => Ok(TransitionMode::Open),
            other => Err(CoreError::machine(format!(
                "unknown `:transition-mode` value `{other}`"
            ))),
        },
    }
}

fn parse_on_exhausted(s: &str, ctx: &str) -> Result<OnExhausted> {
    match s {
        "escalate" => Ok(OnExhausted::Escalate),
        "abort" => Ok(OnExhausted::Abort),
        other => Err(CoreError::machine(format!(
            "{ctx}: unknown `:on-exhausted` value `{other}`"
        ))),
    }
}

fn parse_on_fail(value: mlua::Value, ctx: &str) -> Result<OnFail> {
    match value {
        mlua::Value::Nil => Ok(OnFail::default()),
        mlua::Value::String(s) => match s.to_string_lossy().as_str() {
            "retry" => Ok(OnFail::Retry),
            "abort" => Ok(OnFail::Abort),
            other => Err(CoreError::machine(format!(
                "{ctx}: unknown `:on-fail` value `{other}`"
            ))),
        },
        mlua::Value::Table(t) => {
            let route = require_str(&t, "route", ctx)?;
            Ok(OnFail::Route(route))
        }
        other => Err(CoreError::machine(format!(
            "{ctx}: `:on-fail` must be a string or a `{{:route ..}}` table, got {}",
            other.type_name()
        ))),
    }
}

/// `:playbook` (bare name or `/`-containing path) or a state-local `:prompt`.
fn parse_playbook(state_table: &mlua::Table, state_id: &str) -> Result<PlaybookRef> {
    let playbook = get_str(state_table, "playbook")?;
    let prompt = get_str(state_table, "prompt")?;
    match (playbook, prompt) {
        (Some(p), _) if p.contains('/') => Ok(PlaybookRef::Path(PathBuf::from(p))),
        (Some(p), _) => Ok(PlaybookRef::Named(p)),
        (None, Some(prompt)) => Ok(PlaybookRef::Inline(prompt)),
        (None, None) => Err(CoreError::machine(format!(
            "state `{state_id}`: needs either `:playbook` or `:prompt`"
        ))),
    }
}

/// `:task`/`:plan`: a path relative to `machine_dir` if it resolves to a real
/// file, else inline prose — unless it looks like it was meant to be a file
/// (`.md` suffix), in which case a non-resolving path is an authoring error,
/// not a silent fallback to prose.
fn resolve_prose(table: &mlua::Table, field: &'static str, machine_dir: &Path) -> Result<String> {
    let raw = require_str(table, field, "machine")?;
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

fn parse_qa_cases(table: &mlua::Table) -> Result<Vec<QaCase>> {
    let mut out = Vec::new();
    let Some(arr) = get_table(table, "qa-cases")? else {
        return Ok(out);
    };
    for (i, item) in arr.sequence_values::<mlua::Value>().enumerate() {
        let item = item.map_err(|e| CoreError::machine(format!("qa-cases[{i}]: {e}")))?;
        let t = match item {
            mlua::Value::Table(t) => t,
            other => {
                return Err(CoreError::machine(format!(
                    "qa-cases[{i}]: expected a table, got {}",
                    other.type_name()
                )));
            }
        };
        let ctx = format!("qa-cases[{i}]");
        let id = require_str(&t, "id", &ctx)?;
        let desc = require_str(&t, "desc", &ctx)?;
        out.push(QaCase { id, desc });
    }
    Ok(out)
}

fn parse_defaults(table: &mlua::Table) -> Result<Defaults> {
    match get_table(table, "defaults")? {
        None => Ok(Defaults::default()),
        Some(t) => {
            let model = parse_model_choice(&t, "`:defaults`")?;
            let tools = get_str_vec(&t, "tools")?.unwrap_or_default();
            let exclude_tools = get_str_vec(&t, "exclude-tools")?.unwrap_or_default();
            Ok(Defaults {
                model,
                tools,
                exclude_tools,
            })
        }
    }
}

fn parse_states(table: &mlua::Table) -> Result<BTreeMap<StateId, State>> {
    let states_table =
        get_table(table, "states")?.ok_or_else(|| CoreError::machine("missing `:states`"))?;
    let mut out = BTreeMap::new();
    for pair in states_table.pairs::<String, mlua::Value>() {
        let (id, value) = pair.map_err(|e| CoreError::machine(format!("`:states`: {e}")))?;
        let st = match value {
            mlua::Value::Table(t) => t,
            other => {
                return Err(CoreError::machine(format!(
                    "state `{id}`: expected a table, got {}",
                    other.type_name()
                )));
            }
        };
        let ctx = format!("state `{id}`");
        let playbook = parse_playbook(&st, &id)?;
        let model = parse_model_choice(&st, &ctx)?;
        let tools = get_str_vec(&st, "tools")?.unwrap_or_default();
        let exclude_tools = get_str_vec(&st, "exclude-tools")?.unwrap_or_default();
        let description = get_str(&st, "description")?;
        out.insert(
            id.clone(),
            State {
                id,
                playbook,
                model,
                tools,
                exclude_tools,
                description,
            },
        );
    }
    if out.is_empty() {
        return Err(CoreError::machine(
            "`:states` must declare at least one state",
        ));
    }
    Ok(out)
}

fn resolve_entry(table: &mlua::Table, states: &BTreeMap<StateId, State>) -> Result<StateId> {
    match get_str(table, "entry")? {
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

fn parse_terminals(table: &mlua::Table) -> Result<BTreeSet<StateId>> {
    let arr =
        get_table(table, "terminals")?.ok_or_else(|| CoreError::machine("missing `:terminals`"))?;
    let mut out = BTreeSet::new();
    for (i, item) in arr.sequence_values::<mlua::Value>().enumerate() {
        let item = item.map_err(|e| CoreError::machine(format!("terminals[{i}]: {e}")))?;
        match item {
            mlua::Value::String(s) => {
                out.insert(s.to_string_lossy());
            }
            other => {
                return Err(CoreError::machine(format!(
                    "terminals[{i}]: expected a string, got {}",
                    other.type_name()
                )));
            }
        }
    }
    Ok(out)
}

fn parse_budgets(table: &mlua::Table) -> Result<Budgets> {
    match get_table(table, "budgets")? {
        None => Ok(Budgets::default()),
        Some(t) => Ok(Budgets {
            usd: get_f64(&t, "usd")?,
            wallclock_s: get_u64(&t, "wallclock-s")?,
            max_transitions: get_u32(&t, "max-transitions")?,
        }),
    }
}

/// `:check` is either a bare command string or a `{:cmd .. :timeout-s ..}`
/// table. The bare form is the common case, so it stays a one-liner.
fn parse_check(t: &mlua::Table, ctx: &str) -> Result<Option<Check>> {
    match get_value(t, "check")? {
        mlua::Value::Nil => Ok(None),
        mlua::Value::String(s) => {
            let cmd = s.to_str().map(|s| s.to_string()).map_err(|e| {
                CoreError::machine(format!("{ctx}: `:check` is not valid UTF-8: {e}"))
            })?;
            check_from_parts(cmd, None, ctx)
        }
        mlua::Value::Table(inner) => {
            let cmd = require_str(&inner, "cmd", ctx)?;
            let timeout_s = get_u64(&inner, "timeout-s")?;
            check_from_parts(cmd, timeout_s, ctx)
        }
        other => Err(CoreError::machine(format!(
            "{ctx}: `:check` must be a command string or a `{{:cmd ..}}` table, got {}",
            other.type_name()
        ))),
    }
}

fn check_from_parts(cmd: String, timeout_s: Option<u64>, ctx: &str) -> Result<Option<Check>> {
    if cmd.trim().is_empty() {
        return Err(CoreError::machine(format!(
            "{ctx}: `:check` command is empty — omit the key instead"
        )));
    }
    Ok(Some(Check {
        cmd,
        timeout_s: timeout_s.unwrap_or(DEFAULT_CHECK_TIMEOUT_S),
    }))
}

fn parse_transitions(table: &mlua::Table) -> Result<Vec<Transition>> {
    let mut out = Vec::new();
    let Some(arr) = get_table(table, "transitions")? else {
        return Ok(out);
    };
    for (i, item) in arr.sequence_values::<mlua::Value>().enumerate() {
        let item = item.map_err(|e| CoreError::machine(format!("transitions[{i}]: {e}")))?;
        let t = match item {
            mlua::Value::Table(t) => t,
            other => {
                return Err(CoreError::machine(format!(
                    "transitions[{i}]: expected a table, got {}",
                    other.type_name()
                )));
            }
        };
        let ctx = format!("transitions[{i}]");
        let from = require_str(&t, "from", &ctx)?;
        let to = require_str(&t, "to", &ctx)?;
        // `:when` was a Fennel closure gating on ledger vars. Both are gone;
        // say so rather than ignoring the key and silently unguarding an edge.
        if !matches!(get_value(&t, "when")?, mlua::Value::Nil) {
            return Err(CoreError::machine(format!(
                "{ctx}: `:when` guards were removed — express the condition as a `:check` \
                 command the harness runs, or as `:criteria` for the Judge to evaluate"
            )));
        }
        let check = parse_check(&t, &ctx)?;
        let criteria = get_str(&t, "criteria")?;
        let on_fail = parse_on_fail(get_value(&t, "on-fail")?, &ctx)?;
        let backoff_s = get_u64(&t, "backoff-s")?;
        out.push(Transition {
            from,
            to,
            check,
            criteria,
            on_fail,
            backoff_s,
        });
    }
    Ok(out)
}

fn parse_loops(table: &mlua::Table) -> Result<Vec<LoopSpec>> {
    let mut out = Vec::new();
    let Some(arr) = get_table(table, "loops")? else {
        return Ok(out);
    };
    for (i, item) in arr.sequence_values::<mlua::Value>().enumerate() {
        let item = item.map_err(|e| CoreError::machine(format!("loops[{i}]: {e}")))?;
        let t = match item {
            mlua::Value::Table(t) => t,
            other => {
                return Err(CoreError::machine(format!(
                    "loops[{i}]: expected a table, got {}",
                    other.type_name()
                )));
            }
        };
        let ctx = format!("loops[{i}]");
        let name = require_str(&t, "name", &ctx)?;
        let states = get_str_vec(&t, "states")?.unwrap_or_default();
        if states.is_empty() {
            return Err(CoreError::machine(format!(
                "{ctx}: `:states` must not be empty"
            )));
        }
        let max_cycles = get_u32(&t, "max-cycles")?
            .ok_or_else(|| CoreError::machine(format!("{ctx}: missing `:max-cycles`")))?;
        let on_exhausted = match get_str(&t, "on-exhausted")? {
            Some(s) => parse_on_exhausted(&s, &ctx)?,
            None => OnExhausted::default(),
        };
        out.push(LoopSpec {
            name,
            states,
            max_cycles,
            on_exhausted,
        });
    }
    Ok(out)
}

fn parse_navigator_max_invocations(table: &mlua::Table, default: u32) -> Result<u32> {
    match get_table(table, "navigator")? {
        None => Ok(default),
        Some(t) => Ok(get_u32(&t, "max-invocations")?.unwrap_or(default)),
    }
}

/// Convert an evaluated machine table into the IR, resolving `:task`/`:plan`
/// paths against `machine_dir`.
pub fn machine_from_table(
    table: &mlua::Table,
    machine_dir: &Path,
    source_hash: String,
    source_path: &Path,
    config: &Config,
) -> Result<Machine> {
    let ticket = require_str(table, "ticket", "machine")?;
    let task = resolve_prose(table, "task", machine_dir)?;
    let plan = resolve_prose(table, "plan", machine_dir)?;
    let qa_cases = parse_qa_cases(table)?;
    let defaults = parse_defaults(table)?;

    let states = parse_states(table)?;
    let entry = resolve_entry(table, &states)?;
    let terminals = parse_terminals(table)?;

    let escalation_state = get_str(table, "escalation-state")?;
    if let Some(esc) = &escalation_state {
        if !states.contains_key(esc) && !terminals.contains(esc) {
            return Err(CoreError::machine(format!(
                "`:escalation-state` `{esc}` is not a declared state or terminal"
            )));
        }
    }

    let transitions = parse_transitions(table)?;
    for t in &transitions {
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
    }

    let loops = parse_loops(table)?;
    let budgets = parse_budgets(table)?.tighten(config.budgets);
    let judge = model_spec_overlay(table, "judge", &config.judge)?;
    let navigator = model_spec_overlay(table, "navigator", &config.navigator)?;
    let navigator_max_invocations =
        parse_navigator_max_invocations(table, config.navigator_max_invocations)?;
    let transition_mode =
        parse_transition_mode(get_str(table, "transition-mode")?, config.transition_mode)?;

    Ok(Machine {
        ticket,
        task,
        plan,
        qa_cases,
        entry,
        terminals,
        escalation_state,
        defaults,
        states,
        transitions,
        loops,
        budgets,
        judge,
        navigator,
        navigator_max_invocations,
        transition_mode,
        source_hash,
        source_path: source_path.to_path_buf(),
        dir: machine_dir.to_path_buf(),
    })
}

/// Overlay an evaluated config table onto [`Config::defaults`].
pub fn config_from_table(table: &mlua::Table, base: Config) -> Result<Config> {
    let provider = get_str(table, "provider")?.unwrap_or(base.provider);
    let worker = model_spec_overlay(table, "worker", &base.worker)?;
    let judge = model_spec_overlay(table, "judge", &base.judge)?;
    let navigator = model_spec_overlay(table, "navigator", &base.navigator)?;
    let navigator_max_invocations =
        parse_navigator_max_invocations(table, base.navigator_max_invocations)?;
    let default_tools = get_str_vec(table, "default-tools")?.unwrap_or(base.default_tools);
    let pi_extensions = get_str_vec(table, "pi-extensions")?.unwrap_or(base.pi_extensions);

    let budgets = match get_table(table, "budgets")? {
        None => base.budgets,
        Some(t) => Budgets {
            usd: get_f64(&t, "usd")?.or(base.budgets.usd),
            wallclock_s: get_u64(&t, "wallclock-s")?.or(base.budgets.wallclock_s),
            max_transitions: get_u32(&t, "max-transitions")?.or(base.budgets.max_transitions),
        },
    };

    let context = match get_str(table, "context")? {
        None => base.context,
        Some(s) => match s.as_str() {
            "digest" => ContextMode::Digest,
            "full" => ContextMode::Full,
            other => {
                return Err(CoreError::machine(format!(
                    "unknown `:context` value `{other}`"
                )));
            }
        },
    };

    let digest_last_n = get_u32(table, "digest-last-n")?
        .map(|n| n as usize)
        .unwrap_or(base.digest_last_n);
    let transition_mode =
        parse_transition_mode(get_str(table, "transition-mode")?, base.transition_mode)?;

    Ok(Config {
        provider,
        worker,
        judge,
        navigator,
        navigator_max_invocations,
        default_tools,
        pi_extensions,
        budgets,
        context,
        digest_last_n,
        transition_mode,
        pi_bin: base.pi_bin,
        paths: base.paths,
    })
}
