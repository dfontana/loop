//! The guard tiers, checked cheapest-first.

use loop_core::{AgentRunner, GuardOutcome, JudgeSpec, Machine, Result, Transition};

/// The verdict on one proposed edge, plus what to write to the ledger.
#[derive(Clone, Debug)]
pub struct GuardReport {
    pub structural: GuardOutcome,
    pub criteria: GuardOutcome,
    pub judge_rationale: Option<String>,
    pub usage: loop_core::Usage,
}

impl GuardReport {
    pub fn passed(&self) -> bool {
        self.structural != GuardOutcome::Fail && self.criteria != GuardOutcome::Fail
    }
}

/// The edge a worker's proposal takes.
///
/// Parallel edges between the same pair used to be disambiguated by their
/// `when` guards; with those gone the first declared edge wins, and
/// [`crate::validate`] flags the duplicate so it never silently decides which
/// `criteria` applies.
pub fn select_edge<'m>(machine: &'m Machine, from: &str, to: &str) -> Option<&'m Transition> {
    machine
        .transitions
        .iter()
        .find(|t| t.from == from && t.to == to)
}

/// Run the tiers on one edge.
///
/// TASK T5. The load-bearing rule: the Judge sees the worker's output digest
/// and artifact paths — never the worker's own claim that it succeeded
/// (docs/07 #1).
pub fn check(
    runner: &dyn AgentRunner,
    edge: &Transition,
    judge: impl FnOnce(&str) -> JudgeSpec,
) -> Result<GuardReport> {
    // Structural: by the time an edge reaches `check`, it was already resolved
    // out of the machine's declared transitions (by `select_edge` or the
    // constrained `transition` tool schema), so it always passes here.
    let structural = GuardOutcome::Pass;

    // `criteria` — a separate, cheap Judge that sees only outputs and
    // artifacts, never the worker's own claim of success.
    let (criteria, judge_rationale, usage) = match &edge.criteria {
        None => (GuardOutcome::Skip, None, loop_core::Usage::default()),
        Some(criteria_text) => {
            let spec = judge(criteria_text);
            let verdict = runner.run_judge(&spec)?;
            let outcome = if verdict.pass {
                GuardOutcome::Pass
            } else {
                GuardOutcome::Fail
            };
            (outcome, Some(verdict.rationale), verdict.usage)
        }
    };

    Ok(GuardReport {
        structural,
        criteria,
        judge_rationale,
        usage,
    })
}
