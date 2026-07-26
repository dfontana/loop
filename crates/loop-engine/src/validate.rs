//! `loop validate` — the static linter (docs/05-design-notes.md).
//!
//! Machine authoring errors are the cheapest class of failure to catch and the
//! most annoying to debug at run time: an unreachable state, a dangling
//! playbook or skill reference, no path to a terminal, two edges between the
//! same pair where only the first can ever be taken.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use loop_core::{Machine, PlaybookRef};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Warning,
    Error,
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub severity: Severity,
    /// Where in the machine: a state id, a transition, or the file itself.
    pub where_: String,
    pub message: String,
}

impl Diagnostic {
    fn error(where_: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            where_: where_.into(),
            message: message.into(),
        }
    }

    fn warning(where_: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            where_: where_.into(),
            message: message.into(),
        }
    }
}

/// Lint a loaded machine.
///
/// TASK T5. Checks, all of them `Error` unless noted:
/// - `entry` exists; every `terminals` entry exists as a state or is otherwise
///   never entered (a terminal needs no state definition — it has no playbook).
/// - Every transition's `from`/`to` names a state or terminal.
/// - Every state is reachable from `entry`.
/// - Every state has a path to some terminal (else the run can only exhaust).
/// - Every state's `playbook` resolves (checked by the caller supplying
///   `resolve`, since resolution is filesystem work).
/// - The skills and MCP servers each state *actually loads*, which is the
///   union with `config.fnl`'s `:default-skills` / `:default-mcp` — not just
///   the names the machine writes. Linting the machine's layer alone left a
///   typo in the global toolbox config to surface as a failed stage mid-run.
/// - Every loop's states exist and its head is a state some edge re-enters.
/// - `escalation_state`, if set, is a terminal.
/// - Every skill a state names resolves (same caller-supplied filesystem seam).
/// - A state names MCP servers while `mcp` is absent from `pi-extensions`
///   (`mcp_enabled`): the stage would be told to call a tool it wasn't given.
///   The server *names* are not checkable — they live in the user's own
///   `mcp.json`, which loop never reads.
/// - **Warning:** an edge with neither `check` nor `criteria` — the worker's
///   proposal is committed unexamined.
/// - No two transitions share a `from`/`to` pair: `select_edge` takes the
///   first, so the rest are dead and their `criteria` silently ignored.
pub fn validate(
    machine: &Machine,
    resolve: &dyn Fn(&PlaybookRef) -> bool,
    resolve_skill: &dyn Fn(&str) -> bool,
    mcp_enabled: bool,
    default_skills: &[String],
    default_mcp: &[String],
) -> Vec<Diagnostic> {
    let mut out = Vec::new();

    let name_exists =
        |id: &str| -> bool { machine.states.contains_key(id) || machine.terminals.contains(id) };

    // entry exists.
    if !machine.states.contains_key(&machine.entry) {
        out.push(Diagnostic::error(
            "machine",
            format!("entry state `{}` is not a defined state", machine.entry),
        ));
    }

    // Every transition's from/to names a state or terminal.
    for t in &machine.transitions {
        if !name_exists(&t.from) {
            out.push(Diagnostic::error(
                format!("{} -> {}", t.from, t.to),
                format!("transition `from` `{}` names no state or terminal", t.from),
            ));
        }
        if !name_exists(&t.to) {
            out.push(Diagnostic::error(
                format!("{} -> {}", t.from, t.to),
                format!("transition `to` `{}` names no state or terminal", t.to),
            ));
        }
    }

    // Every state is reachable from `entry`.
    let mut reachable: BTreeSet<&str> = BTreeSet::new();
    if machine.states.contains_key(&machine.entry) {
        let mut queue: VecDeque<&str> = VecDeque::new();
        queue.push_back(machine.entry.as_str());
        reachable.insert(machine.entry.as_str());
        while let Some(id) = queue.pop_front() {
            for t in machine.edges_from(id) {
                if machine.states.contains_key(&t.to) && reachable.insert(t.to.as_str()) {
                    queue.push_back(t.to.as_str());
                }
            }
        }
    }
    for id in machine.states.keys() {
        if !reachable.contains(id.as_str()) {
            out.push(Diagnostic::error(
                id.clone(),
                format!("state `{id}` is unreachable from entry `{}`", machine.entry),
            ));
        }
    }

    // Every state has a path to some terminal.
    let mut reverse: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for t in &machine.transitions {
        reverse
            .entry(t.to.as_str())
            .or_default()
            .push(t.from.as_str());
    }
    let mut can_reach_terminal: BTreeSet<&str> = BTreeSet::new();
    let mut queue: VecDeque<&str> = machine.terminals.iter().map(|s| s.as_str()).collect();
    while let Some(id) = queue.pop_front() {
        if let Some(froms) = reverse.get(id) {
            for &f in froms {
                if can_reach_terminal.insert(f) {
                    queue.push_back(f);
                }
            }
        }
    }
    for id in machine.states.keys() {
        if !can_reach_terminal.contains(id.as_str()) {
            out.push(Diagnostic::error(
                id.clone(),
                format!("state `{id}` has no path to any terminal"),
            ));
        }
    }

    // Every state's playbook and skills resolve.
    for (id, state) in &machine.states {
        if !resolve(&state.playbook) {
            out.push(Diagnostic::error(
                id.clone(),
                format!("playbook for state `{id}` does not resolve in the toolbox"),
            ));
        }
        // The effective list, config defaults included — that union is what
        // the spawn loads, so it is what has to resolve.
        for name in machine.resolve_skills(state, default_skills) {
            if !resolve_skill(&name) {
                let source = if default_skills.contains(&name) {
                    " (from `:default-skills` in config.fnl)"
                } else {
                    ""
                };
                out.push(Diagnostic::error(
                    id.clone(),
                    format!(
                        "skill `{name}` on state `{id}`{source} does not resolve in the toolbox"
                    ),
                ));
            }
        }
        // The names themselves are unverifiable here — they belong to the
        // user's `mcp.json`. What *is* checkable is whether the tool that
        // connects them will exist in the spawn at all.
        if !mcp_enabled && !machine.resolve_mcp(state, default_mcp).is_empty() {
            out.push(Diagnostic::error(
                id.clone(),
                format!(
                    "state `{id}` names MCP servers, but `mcp` is not in `:pi-extensions` \
                     — the stage would be told to call a tool it does not have"
                ),
            ));
        }
    }

    // Every loop's states exist and its head is re-entered by some edge.
    for l in &machine.loops {
        if l.states.is_empty() {
            out.push(Diagnostic::error(
                l.name.clone(),
                format!("loop `{}` declares no states", l.name),
            ));
            continue;
        }
        for s in &l.states {
            if !name_exists(s) {
                out.push(Diagnostic::error(
                    l.name.clone(),
                    format!("loop `{}` references unknown state `{s}`", l.name),
                ));
            }
        }
        if let Some(head) = l.head() {
            // A head is re-entered either by a declared edge or by an
            // `on_fail: route` landing on it — a failed judge routing back to
            // `implement` is exactly how the shipped template loops, and it
            // commits a real transition, so it counts.
            let re_entered = machine.transitions.iter().any(|t| {
                &t.to == head || matches!(&t.on_fail, loop_core::OnFail::Route(r) if r == head)
            });
            if !re_entered {
                out.push(Diagnostic::error(
                    l.name.clone(),
                    format!(
                        "loop `{}`'s head `{head}` is never re-entered by any transition",
                        l.name
                    ),
                ));
            }
        }
    }

    // escalation_state, if set, is a terminal.
    if let Some(esc) = &machine.escalation_state
        && !machine.terminals.contains(esc)
    {
        out.push(Diagnostic::error(
            "machine",
            format!("escalation_state `{esc}` is not a declared terminal"),
        ));
    }

    // Warning: an edge with no `check` and no `criteria`. Nothing stands
    // between the worker's proposal and the commit, so the worker's word is
    // final on that hop. Sometimes that is exactly right — an unconditional
    // hand-off into the first working state — which is why this is advisory.
    // It is worth saying out loud because an unguarded edge is now the *shape
    // you get by forgetting*, where it used to take deleting a `when`.
    for t in &machine.transitions {
        if t.check.is_none() && t.criteria.is_none() {
            out.push(Diagnostic::warning(
                t.from.clone(),
                format!(
                    "transition `{}` → `{}` has neither `check` nor `criteria`: the worker's \
                     proposal is committed unexamined",
                    t.from, t.to
                ),
            ));
        }
    }

    // Error: two edges between the same pair. `when` guards used to tell such
    // edges apart; without them `select_edge` takes the first declared one and
    // the rest are dead, silently discarding whatever `criteria` they carry.
    let mut seen_pairs: BTreeSet<(&str, &str)> = BTreeSet::new();
    for t in &machine.transitions {
        if !seen_pairs.insert((t.from.as_str(), t.to.as_str())) {
            out.push(Diagnostic::error(
                t.from.clone(),
                format!(
                    "duplicate transition `{}` → `{}`: only the first is ever taken — merge them \
                     into one edge",
                    t.from, t.to
                ),
            ));
        }
    }

    out
}
