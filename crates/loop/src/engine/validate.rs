//! `loop validate` — the static linter (docs/05-design-notes.md).
//!
//! Machine authoring errors are the cheapest class of failure to catch and the
//! most annoying to debug at run time: an unreachable state, a dangling
//! stage prompt or skill reference, no path to a terminal, two edges between the
//! same pair where only the first can ever be taken.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::core::{Machine, StagePromptRef, Transition};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Warning,
    Error,
}

impl Severity {
    /// The fixed-width tag a diagnostic line opens with. Padded, so the
    /// `where_` column after it lines up between a warning and an error.
    pub fn tag(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warn ",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub severity: Severity,
    /// Where in the machine: a state id, a transition, or the file itself.
    pub where_: String,
    pub message: String,
}

/// `loop validate` prints these and `loop preview` embeds them, and the two
/// have to read identically — preview reuses the real linter precisely so that
/// it reports what validate would. That claim was made in a doc comment while
/// the format string itself was written out in both places; now there is one.
impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}  {}: {}",
            self.severity.tag(),
            self.where_,
            self.message
        )
    }
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

/// An edge list keyed by source, from whichever end of each transition `edge`
/// picks — so one map builder serves both the forward and the reversed graph.
fn adjacency<'m>(
    transitions: &'m [Transition],
    edge: impl Fn(&'m Transition) -> Option<(&'m str, &'m str)>,
) -> BTreeMap<&'m str, Vec<&'m str>> {
    let mut out: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (from, to) in transitions.iter().filter_map(edge) {
        out.entry(from).or_default().push(to);
    }
    out
}

/// Everything reachable from `seeds` over `adjacency`, seeds included.
///
/// One worklist traversal. `validate` asks two reachability questions — can
/// `entry` get here, and can this get to a terminal — which are the same walk
/// over the graph and over the graph reversed, and were written out twice with
/// their own `VecDeque`, their own `BTreeSet`, and their own `while let`.
fn reach<'m>(
    seeds: impl IntoIterator<Item = &'m str>,
    adjacency: &BTreeMap<&'m str, Vec<&'m str>>,
) -> BTreeSet<&'m str> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut queue: VecDeque<&str> = VecDeque::new();
    for seed in seeds {
        if seen.insert(seed) {
            queue.push_back(seed);
        }
    }
    while let Some(id) = queue.pop_front() {
        for &next in adjacency.get(id).into_iter().flatten() {
            if seen.insert(next) {
                queue.push_back(next);
            }
        }
    }
    seen
}

/// Lint a loaded machine.
///
/// Checks, all of them `Error` unless noted:
/// - `entry` exists; every `terminals` entry exists as a state or is otherwise
///   never entered (a terminal needs no state definition — it has no stage prompt).
/// - Every transition's `from`/`to` names a state or terminal.
/// - Every state is reachable from `entry`.
/// - Every state has a path to some terminal (else the run can only exhaust).
/// - Every state's `stage_prompt` resolves (checked by the caller supplying
///   `resolve`, since resolution is filesystem work).
/// - The skills and MCP servers each state *actually loads*, which is the
///   union with the machine's `:defaults {:skills ..}` / `{:mcp ..}` — not just
///   the names the machine writes. Linting the machine's layer alone left a
///   typo in the global toolbox config to surface as a failed stage mid-run.
/// - Every loop's states exist and its head is a state some edge re-enters.
/// - `escalation_state`, if set, names a state or terminal.
/// - Every skill a state names resolves (same caller-supplied filesystem seam).
/// - A state names MCP servers while `mcp` is absent from `pi-extensions`
///   (`mcp_enabled`): the stage would be told to call a tool it wasn't given.
///   The server *names* are not checkable — they live in the user's own
///   `mcp.json`, which loop never reads.
/// - **Warning:** an edge with neither `check` nor `criteria` — the worker's
///   proposal is committed unexamined.
/// - No two transitions share a `from`/`to` pair: `Machine::edge` takes the
///   first, so the rest are dead and their `criteria` silently ignored.
pub fn validate(
    machine: &Machine,
    resolve: &dyn Fn(&StagePromptRef) -> bool,
    resolve_skill: &dyn Fn(&str) -> bool,
) -> Vec<Diagnostic> {
    // Whether the `mcp` extension is declared installed. A stage naming MCP
    // servers without it would be told to call a tool it does not have.
    let mcp_enabled = machine.pi_extensions.iter().any(|e| e == "mcp");
    let mut out = Vec::new();

    // entry exists.
    if !machine.states.contains_key(&machine.entry) {
        out.push(Diagnostic::error(
            "machine",
            format!("entry state `{}` is not a defined state", machine.entry),
        ));
    }

    // Every transition's from/to names a state or terminal.
    for t in &machine.transitions {
        if !machine.declares(&t.from) {
            out.push(Diagnostic::error(
                format!("{} -> {}", t.from, t.to),
                format!("transition `from` `{}` names no state or terminal", t.from),
            ));
        }
        if !machine.declares(&t.to) {
            out.push(Diagnostic::error(
                format!("{} -> {}", t.from, t.to),
                format!("transition `to` `{}` names no state or terminal", t.to),
            ));
        }
    }

    // Every state is reachable from `entry`. Only edges landing in a *defined
    // state* are followed: a terminal is where a run stops, so reaching one
    // does not make whatever else points out of it reachable.
    let forward = adjacency(&machine.transitions, |t| {
        machine
            .states
            .contains_key(&t.to)
            .then_some((t.from.as_str(), t.to.as_str()))
    });
    let entry_exists = machine.states.contains_key(&machine.entry);
    let reachable = reach(entry_exists.then_some(machine.entry.as_str()), &forward);
    for id in machine.states.keys() {
        if !reachable.contains(id.as_str()) {
            out.push(Diagnostic::error(
                id.clone(),
                format!("state `{id}` is unreachable from entry `{}`", machine.entry),
            ));
        }
    }

    // Every state has a path to some terminal — the same walk, over the edges
    // reversed, seeded from the terminals.
    let reverse = adjacency(&machine.transitions, |t| {
        Some((t.to.as_str(), t.from.as_str()))
    });
    let can_reach_terminal = reach(machine.terminals.iter().map(String::as_str), &reverse);
    for id in machine.states.keys() {
        if !can_reach_terminal.contains(id.as_str()) {
            out.push(Diagnostic::error(
                id.clone(),
                format!("state `{id}` has no path to any terminal"),
            ));
        }
    }

    // Every state's stage prompt and skills resolve.
    for (id, state) in &machine.states {
        if !resolve(&state.stage_prompt) {
            out.push(Diagnostic::error(
                id.clone(),
                format!("stage prompt for state `{id}` does not resolve in .loop/stage-prompts/"),
            ));
        }
        // The effective list, machine defaults included — that union is what
        // the spawn loads, so it is what has to resolve.
        for name in machine.resolve_skills(state) {
            if !resolve_skill(&name) {
                let source = if machine.defaults.skills.contains(&name) {
                    " (from `:defaults {:skills ..}`)"
                } else {
                    ""
                };
                out.push(Diagnostic::error(
                    id.clone(),
                    format!(
                        "skill `{name}` on state `{id}`{source} does not resolve in .loop/skills/"
                    ),
                ));
            }
        }
        // The names themselves are unverifiable here — they belong to the
        // user's `mcp.json`. What *is* checkable is whether the tool that
        // connects them will exist in the spawn at all.
        if !mcp_enabled && !machine.resolve_mcp(state).is_empty() {
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
            if !machine.declares(s) {
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
                &t.to == head || matches!(&t.on_fail, crate::core::OnFail::Route(r) if r == head)
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

    // escalation_state, if set, names something. A *state* is as valid as a
    // terminal here: the engine commits to it directly and then runs it like
    // any other stage, which is how a machine gets a "go do recovery work"
    // destination rather than only a "give up here" one. This used to demand a
    // terminal, disagreeing with the loader, docs/03-customizing.md, and the
    // linter, loader, docs/03-customizing.md and engine must agree on this.
    if let Some(esc) = &machine.escalation_state
        && !machine.declares(esc)
    {
        out.push(Diagnostic::error(
            "machine",
            format!("escalation_state `{esc}` names no state or terminal"),
        ));
    }

    // Warning: an edge with no `check` and no `criteria`. Nothing stands
    // between the worker's proposal and the commit, so the worker's word is
    // final on that hop. Sometimes that is exactly right — an unconditional
    // hand-off into the first working state — which is why this is advisory.
    // It is worth saying out loud: unguarded is the shape you get by forgetting.
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

    // Error: two edges between the same pair. `Machine::edge` takes the first
    // declared one, so the rest are dead and their `criteria` silently ignored.
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
