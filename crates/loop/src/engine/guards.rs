//! The guard tiers, checked cheapest-first.

use crate::core::{AgentRunner, CheckRunner, GuardOutcome, JudgeSpec, Result, StateId, Transition};

/// The verdict on one proposed edge, plus what to write to the ledger.
#[derive(Clone, Debug)]
pub struct GuardReport {
    pub check: GuardOutcome,
    pub criteria: GuardOutcome,
    pub check_output: Option<String>,
    pub judge_rationale: Option<String>,
    pub usage: crate::core::Usage,
}

impl GuardReport {
    pub fn passed(&self) -> bool {
        self.check != GuardOutcome::Fail && self.criteria != GuardOutcome::Fail
    }
}

/// Run the tiers on one edge, cheapest-first.
///
/// Two rules that are load-bearing, not stylistic:
/// - The `check` runs **before** the Judge and short-circuits it. It is
///   deterministic, costs no tokens, and — unlike anything the Judge reads —
///   the worker had no hand in producing it. A failed check is not appealable
///   to an LLM; that would be the grade-your-own-homework hole re-opened from
///   the other side.
/// - The Judge sees the worker's output digest, artifact paths, and the
///   check's output — never the worker's own claim that it succeeded
///   (docs/design-notes.md).
///
/// The edge is already known to exist: it was resolved out of the machine's
/// declared transitions by `Machine::edge`, or the Navigator picked it from the
/// states it was offered. There is no structural tier to evaluate here.
pub fn check(
    runner: &dyn AgentRunner,
    checks: &dyn CheckRunner,
    edge: &Transition,
    from: &StateId,
    cycle: u32,
    attempt: u32,
    judge: impl FnOnce(&str, Option<&str>) -> Result<JudgeSpec>,
) -> Result<GuardReport> {
    let (check_tier, check_output) = match &edge.check {
        None => (GuardOutcome::Skip, None),
        Some(c) => {
            let outcome = checks.run_check(c, from, cycle, attempt)?;
            (outcome.passed.into(), Some(outcome.output))
        }
    };

    // A failed check settles the edge. Spending a Judge spawn to second-guess
    // a non-zero exit status buys nothing and can only weaken the gate.
    if check_tier == GuardOutcome::Fail {
        return Ok(GuardReport {
            check: check_tier,
            criteria: GuardOutcome::Skip,
            check_output,
            judge_rationale: None,
            usage: crate::core::Usage::default(),
        });
    }

    // `criteria` — a separate, cheap Judge that sees only outputs, artifacts,
    // and the check's stdout, never the worker's own claim of success.
    let (criteria, judge_rationale, usage) = match &edge.criteria {
        None => (GuardOutcome::Skip, None, crate::core::Usage::default()),
        Some(criteria_text) => {
            let spec = judge(criteria_text, check_output.as_deref())?;
            let verdict = runner.run_judge(&spec)?;
            (verdict.pass.into(), Some(verdict.rationale), verdict.usage)
        }
    };

    Ok(GuardReport {
        check: check_tier,
        criteria,
        check_output,
        judge_rationale,
        usage,
    })
}
