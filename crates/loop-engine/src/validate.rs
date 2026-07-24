//! `loop validate` — the static linter (docs/07-risks.md #11).
//!
//! Machine authoring errors are the cheapest class of failure to catch and the
//! most annoying to debug at run time: an unreachable state, a dangling
//! playbook reference, no path to a terminal, a stage whose `criteria` can
//! never be judged because nothing emits the var it gates on.

use loop_core::Machine;

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
pub fn validate(
    machine: &Machine,
    resolve: &dyn Fn(&loop_core::PlaybookRef) -> bool,
) -> Vec<Diagnostic> {
    let _ = (machine, resolve);
    todo!("T5")
}
