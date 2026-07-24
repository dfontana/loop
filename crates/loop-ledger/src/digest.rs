//! The rolling ledger digest — the deterministic continuity channel between
//! stages (docs/01-architecture.md, "Data flow between stages").
//!
//! Never transcripts: the last N committed transitions with their rationales,
//! the current vars, and pinned artifact references. Cost and drift are the
//! reasons this is a summary and not a replay.

use loop_core::Event;

/// Render the digest fed to a stage as `$LEDGER_DIGEST`.
///
/// TASK T1. Include, in this order: the run's ticket and elapsed totals; the
/// last `last_n` `transition_committed` events with the rationale from the
/// matching `transition_proposed`; the current trusted vars; and the artifact
/// table (name → path). Keep it under a few hundred lines — it is prepended to
/// every prompt.
pub fn render(events: &[Event], last_n: usize) -> String {
    let _ = (events, last_n);
    todo!("T1")
}

/// A one-paragraph summary of a single stage's output, for the Judge. It must
/// exclude the worker's own pass/fail claim — the Judge grades artifacts, not
/// self-assessment (docs/07-risks.md #1).
pub fn worker_digest_for_judge(summary: &str, artifacts: &[loop_core::ArtifactRef]) -> String {
    let _ = (summary, artifacts);
    todo!("T1")
}
