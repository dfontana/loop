//! The guard tiers, checked cheapest-first.

use loop_core::{
    AgentRunner, CheckRunner, GuardOutcome, JudgeSpec, Machine, Result, StateId, Transition,
};

/// The verdict on one proposed edge, plus what to write to the ledger.
#[derive(Clone, Debug)]
pub struct GuardReport {
    pub structural: GuardOutcome,
    pub check: GuardOutcome,
    pub criteria: GuardOutcome,
    pub check_output: Option<String>,
    pub judge_rationale: Option<String>,
    pub usage: loop_core::Usage,
}

impl GuardReport {
    pub fn passed(&self) -> bool {
        self.structural != GuardOutcome::Fail
            && self.check != GuardOutcome::Fail
            && self.criteria != GuardOutcome::Fail
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

/// Run the tiers on one edge, cheapest-first.
///
/// TASK T5. Two rules that are load-bearing, not stylistic:
/// - The `check` runs **before** the Judge and short-circuits it. It is
///   deterministic, costs no tokens, and — unlike anything the Judge reads —
///   the worker had no hand in producing it. A failed check is not appealable
///   to an LLM; that would be the grade-your-own-homework hole re-opened from
///   the other side.
/// - The Judge sees the worker's output digest, artifact paths, and the
///   check's output — never the worker's own claim that it succeeded
///   (docs/07 #1).
pub fn check(
    runner: &dyn AgentRunner,
    checks: &dyn CheckRunner,
    edge: &Transition,
    from: &StateId,
    cycle: u32,
    attempt: u32,
    judge: impl FnOnce(&str, Option<&str>) -> Result<JudgeSpec>,
) -> Result<GuardReport> {
    // Structural: by the time an edge reaches `check`, it was already resolved
    // out of the machine's declared transitions (by `select_edge` or the
    // constrained `transition` tool schema), so it always passes here.
    let structural = GuardOutcome::Pass;

    let (check_tier, check_output) = match &edge.check {
        None => (GuardOutcome::Skip, None),
        Some(c) => {
            let outcome = checks.run_check(c, from, cycle, attempt)?;
            let tier = if outcome.passed {
                GuardOutcome::Pass
            } else {
                GuardOutcome::Fail
            };
            (tier, Some(outcome.output))
        }
    };

    // A failed check settles the edge. Spending a Judge spawn to second-guess
    // a non-zero exit status buys nothing and can only weaken the gate.
    if check_tier == GuardOutcome::Fail {
        return Ok(GuardReport {
            structural,
            check: check_tier,
            criteria: GuardOutcome::Skip,
            check_output,
            judge_rationale: None,
            usage: loop_core::Usage::default(),
        });
    }

    // `criteria` — a separate, cheap Judge that sees only outputs, artifacts,
    // and the check's stdout, never the worker's own claim of success.
    let (criteria, judge_rationale, usage) = match &edge.criteria {
        None => (GuardOutcome::Skip, None, loop_core::Usage::default()),
        Some(criteria_text) => {
            let spec = judge(criteria_text, check_output.as_deref())?;
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
        check: check_tier,
        criteria,
        check_output,
        judge_rationale,
        usage,
    })
}
