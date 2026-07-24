//! The three guard tiers, checked cheapest-first.

use loop_core::{
    AgentRunner, GuardEvaluator, GuardOutcome, JudgeSpec, Machine, Result, Transition, Vars,
};

/// The verdict on one proposed edge, plus what to write to the ledger.
#[derive(Clone, Debug)]
pub struct GuardReport {
    pub structural: GuardOutcome,
    pub when: GuardOutcome,
    pub criteria: GuardOutcome,
    pub judge_rationale: Option<String>,
    pub usage: loop_core::Usage,
}

impl GuardReport {
    pub fn passed(&self) -> bool {
        self.structural != GuardOutcome::Fail
            && self.when != GuardOutcome::Fail
            && self.criteria != GuardOutcome::Fail
    }
}

/// Pick the edge a worker's proposal takes. With several edges `from → to`
/// (the transient-vs-real pattern), choose the first whose `when` passes; that
/// is what makes routing on `error_class` work.
///
/// TASK T5.
pub fn select_edge<'m>(
    machine: &'m Machine,
    guards: &dyn GuardEvaluator,
    from: &str,
    to: &str,
    vars: &Vars,
) -> Result<Option<&'m Transition>> {
    for t in machine
        .transitions
        .iter()
        .filter(|t| t.from == from && t.to == to)
    {
        let passes = match t.when {
            // No `when` at all: unconditionally eligible, same as the `when`
            // tier reporting `skip` in `check`.
            None => true,
            Some(guard) => guards.eval(guard, vars)?,
        };
        if passes {
            return Ok(Some(t));
        }
    }
    Ok(None)
}

/// Run the tiers on one edge.
///
/// TASK T5. Two rules that are load-bearing, not stylistic:
/// - Gate `when` on **trusted** vars only. A worker-declared var may inform a
///   prompt; it may never open a QA gate (docs/03, docs/07 #2).
/// - The Judge sees the worker's output digest and artifact paths — never the
///   worker's own claim that it succeeded (docs/07 #1).
pub fn check(
    _machine: &Machine,
    guards: &dyn GuardEvaluator,
    runner: &dyn AgentRunner,
    edge: &Transition,
    trusted_vars: &Vars,
    judge: impl FnOnce(&str) -> JudgeSpec,
) -> Result<GuardReport> {
    // Structural: by the time an edge reaches `check`, it was already resolved
    // out of the machine's declared transitions (by `select_edge` or the
    // constrained `transition` tool schema), so it always passes here.
    let structural = GuardOutcome::Pass;

    // `when` — a deterministic closure over TRUSTED vars only. A
    // worker-declared (untrusted) var must never be able to open a gate.
    let when = match edge.when {
        None => GuardOutcome::Skip,
        Some(guard) => {
            if guards.eval(guard, trusted_vars)? {
                GuardOutcome::Pass
            } else {
                GuardOutcome::Fail
            }
        }
    };

    if when == GuardOutcome::Fail {
        return Ok(GuardReport {
            structural,
            when,
            criteria: GuardOutcome::Skip,
            judge_rationale: None,
            usage: loop_core::Usage::default(),
        });
    }

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
        when,
        criteria,
        judge_rationale,
        usage,
    })
}
