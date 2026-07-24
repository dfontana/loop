//! `loop validate` — the static linter (docs/07-risks.md #11).
//!
//! Machine authoring errors are the cheapest class of failure to catch and the
//! most annoying to debug at run time: an unreachable state, a dangling
//! playbook reference, no path to a terminal, a stage whose `criteria` can
//! never be judged because nothing emits the var it gates on.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use loop_core::{Machine, PlaybookRef, State};

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
/// - Every loop's states exist and its head is a state some edge re-enters.
/// - `escalation_state`, if set, is a terminal.
/// - **Warning:** a state whose only outgoing edges have `when` guards over
///   vars no bound tool is known to emit — the classic "gate that can never
///   open".
/// - **Warning:** a QA-shaped state (one gating a `criteria` edge) that
///   allowlists `edit` or `write` — a validator that can fix what it judges
///   (docs/07 #1).
pub fn validate(machine: &Machine, resolve: &dyn Fn(&PlaybookRef) -> bool) -> Vec<Diagnostic> {
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

    // Every state's playbook resolves.
    for (id, state) in &machine.states {
        if !resolve(&state.playbook) {
            out.push(Diagnostic::error(
                id.clone(),
                format!("playbook for state `{id}` does not resolve in the toolbox"),
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
            let re_entered = machine.transitions.iter().any(|t| &t.to == head);
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

    // Warning: a state whose only outgoing edges are `when`-guarded on a var
    // scope no tool on that state looks bound to emit — the "gate that can
    // never open" trap. This is a syntactic heuristic (the IR carries no
    // tool -> emitted-vars registry): it flags a var scope referenced in a
    // `when` that doesn't textually correspond to any allowlisted tool name.
    for (id, state) in &machine.states {
        let edges = machine.edges_from(id);
        if edges.is_empty() || edges.iter().any(|t| t.when.is_none()) {
            continue;
        }
        let tools = effective_tools(machine, state);
        let mut ungrounded: BTreeSet<String> = BTreeSet::new();
        for t in &edges {
            let Some(src) = &t.when_src else { continue };
            for scope in extract_var_scopes(src) {
                let grounded = tools
                    .iter()
                    .any(|tool| tool.to_lowercase().contains(&scope.to_lowercase()));
                if !grounded {
                    ungrounded.insert(scope);
                }
            }
        }
        if !ungrounded.is_empty() {
            out.push(Diagnostic::warning(
                id.clone(),
                format!(
                    "state `{id}`'s outgoing `when` guards gate on {} but no allowlisted tool looks bound to emit {}",
                    ungrounded
                        .iter()
                        .map(|s| format!("`{s}`"))
                        .collect::<Vec<_>>()
                        .join(", "),
                    if ungrounded.len() == 1 { "it" } else { "them" }
                ),
            ));
        }
    }

    // Warning: a QA-shaped state (gates a `criteria` edge) that allowlists
    // `edit`/`write` — a validator that can fix what it judges (docs/07 #1).
    for (id, state) in &machine.states {
        let is_qa_shaped = machine.edges_from(id).iter().any(|t| t.criteria.is_some());
        if !is_qa_shaped {
            continue;
        }
        let tools = effective_tools(machine, state);
        let offending: Vec<&str> = ["edit", "write"]
            .into_iter()
            .filter(|t| tools.contains(*t))
            .collect();
        if !offending.is_empty() {
            out.push(Diagnostic::warning(
                id.clone(),
                format!(
                    "state `{id}` gates a `criteria` (Judge) edge but allowlists {} — it can fix what it's being judged on",
                    offending.join(", ")
                ),
            ));
        }
    }

    out
}

/// The union of machine defaults and a state's own allowlist, minus excludes —
/// without a `Config` baseline, which `validate` doesn't have.
fn effective_tools(machine: &Machine, state: &State) -> BTreeSet<String> {
    let mut set: BTreeSet<String> = machine.defaults.tools.iter().cloned().collect();
    set.extend(state.tools.iter().cloned());
    for t in state
        .exclude_tools
        .iter()
        .chain(machine.defaults.exclude_tools.iter())
    {
        set.remove(t);
    }
    set
}

/// Pull `scope` out of dotted references like `qa.error_class` in a guard's
/// human-readable source text. Best-effort text scan, not a real parser.
fn extract_var_scopes(src: &str) -> BTreeSet<String> {
    let mut scopes = BTreeSet::new();
    let mut token = String::new();
    let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.';
    let flush = |token: &mut String, scopes: &mut BTreeSet<String>| {
        if let Some(dot_pos) = token.find('.') {
            let scope = &token[..dot_pos];
            if scope.chars().next().is_some_and(|c| c.is_alphabetic()) {
                scopes.insert(scope.to_string());
            }
        }
        token.clear();
    };
    for c in src.chars() {
        if is_ident(c) {
            token.push(c);
        } else {
            flush(&mut token, &mut scopes);
        }
    }
    flush(&mut token, &mut scopes);
    scopes
}
