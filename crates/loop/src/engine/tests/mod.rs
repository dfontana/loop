//! Engine tests, against the fakes in [`crate::engine::test_support`].
//! No Lua, no subprocess, no filesystem, no API key.
//!
//! Two suites, because they exercise two different things and were only ever
//! in one file by accretion: [`run`] drives the control loop and asserts on the
//! ledger it produces, and [`validate`] lints a machine that never runs.

mod run;
mod validate;

use crate::engine::test_support::*;
use crate::engine::{Engine, Outcome};

/// Drive a machine to its terminal with no scripted checks — the empty
/// [`FakeChecks`] queue passes, so a test that doesn't care about the check
/// tier needs no setup for it.
fn drive(machine: &crate::core::Machine, runner: &FakeRunner, ledger: &mut FakeLedger) -> Outcome {
    drive_with(Rig::new(machine, runner, ledger)).0
}

/// The same, with a scripted [`FakeChecks`].
fn drive_checked(
    machine: &crate::core::Machine,
    runner: &FakeRunner,
    checks: &FakeChecks,
    ledger: &mut FakeLedger,
) -> Outcome {
    drive_with(Rig::new(machine, runner, ledger).checks(checks)).0
}

/// Everything the engine borrows, assembled in one place.
///
/// Three tests used to spell out the whole `Engine { .. }` literal themselves —
/// to start the clock in the past, to swap in a refusing artifact sink, to keep
/// the stage builder alive afterwards — so the eight-field struct was written
/// four times over. Now the odd cases set one field and the rest is shared.
struct Rig<'a> {
    machine: &'a crate::core::Machine,
    runner: &'a FakeRunner,
    ledger: &'a mut FakeLedger,
    checks: Option<&'a FakeChecks>,
    artifacts: Option<&'a dyn crate::core::ArtifactSink>,
    started_at: Option<std::time::Instant>,
}

impl<'a> Rig<'a> {
    fn new(
        machine: &'a crate::core::Machine,
        runner: &'a FakeRunner,
        ledger: &'a mut FakeLedger,
    ) -> Self {
        Self {
            machine,
            runner,
            ledger,
            checks: None,
            artifacts: None,
            started_at: None,
        }
    }

    fn checks(mut self, checks: &'a FakeChecks) -> Self {
        self.checks = Some(checks);
        self
    }

    fn artifacts(mut self, sink: &'a dyn crate::core::ArtifactSink) -> Self {
        self.artifacts = Some(sink);
        self
    }

    /// Start the run's clock in the past, so a wallclock budget is already
    /// blown before the first spawn.
    fn started_s_ago(mut self, secs: u64) -> Self {
        self.started_at = Some(std::time::Instant::now() - std::time::Duration::from_secs(secs));
        self
    }
}

/// Run the rig, returning the outcome and every context the stages were built
/// with — how a test asserts on what would have reached a stage prompt.
fn drive_with(rig: Rig<'_>) -> (Outcome, Vec<crate::core::Context>) {
    let own_checks = FakeChecks::default();
    let own_artifacts = FakeArtifacts;
    let stage = FakeStageBuilder::new(rig.machine);

    let outcome = {
        let mut engine = Engine {
            machine: rig.machine,
            runner: rig.runner,
            checks: rig.checks.unwrap_or(&own_checks),
            ledger: rig.ledger,
            artifacts: rig.artifacts.unwrap_or(&own_artifacts),
            stage: &stage,
            started_at: rig.started_at,
            elapsed_offset_s: 0,
        };
        engine.run().expect("engine run should not error")
    };

    let contexts = stage.contexts.borrow().clone();
    (outcome, contexts)
}

/// A worker whose process died: no handoff, non-zero exit.
fn crashed_worker() -> crate::core::WorkerResult {
    crate::core::WorkerResult {
        exit_ok: false,
        proposal: None,
        ..worker_result(proposal_to("done", "unused"))
    }
}
