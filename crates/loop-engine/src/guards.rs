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
    let _ = (machine, guards, from, to, vars);
    todo!("T5")
}

/// Run the tiers on one edge.
///
/// TASK T5. Two rules that are load-bearing, not stylistic:
/// - Gate `when` on **trusted** vars only. A worker-declared var may inform a
///   prompt; it may never open a QA gate (docs/03, docs/07 #2).
/// - The Judge sees the worker's output digest and artifact paths — never the
///   worker's own claim that it succeeded (docs/07 #1).
pub fn check(
    machine: &Machine,
    guards: &dyn GuardEvaluator,
    runner: &dyn AgentRunner,
    edge: &Transition,
    trusted_vars: &Vars,
    judge: impl FnOnce(&str) -> JudgeSpec,
) -> Result<GuardReport> {
    let _ = (machine, guards, runner, edge, trusted_vars, judge);
    todo!("T5")
}
