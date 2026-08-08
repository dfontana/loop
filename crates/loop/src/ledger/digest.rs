//! The rolling ledger digest — the deterministic continuity channel between
//! stages (skills/loop-authoring/references/runtime.md, "Data flow between stages").
//!
//! Never transcripts: the last N committed transitions with their rationales
//! and pinned artifact references. Cost and drift are the reasons this is a
//! summary and not a replay.

use std::fmt::Write as _;

use crate::core::Event;
use crate::core::text::truncate;

/// Rationale text longer than this is truncated in the digest. Rationales are
/// meant to be one sentence; a worker padding one out shouldn't be able to
/// balloon every downstream prompt with it.
const MAX_RATIONALE_CHARS: usize = 300;

/// Render the digest fed to a stage as `$LEDGER_DIGEST`.
///
/// Deliberately reads only the *decisions* out of the log — the ticket, the
/// committed hops with their rationales, the totals, and the artifact refs —
/// and never `worker_output.summary`. That is what keeps this a summary rather
/// than a transcript by construction rather than by convention: every field it
/// reads comes off [`crate::core::RunState`], which carries no prose a worker
/// wrote about itself.
pub fn render(events: &[Event], last_n: usize) -> String {
    let ticket = crate::core::run_started(events).map(|s| s.ticket);

    // Everything below the heading comes off the fold, which already walks
    // these events for the engine: the spend, the transition count, the
    // artifact table, and the committed hops with their rationales. This
    // function used to fold *and* re-walk for the commit indices *and* walk
    // backwards from each one hunting the matching proposal — three passes,
    // and the last of them a nearest-match search for something the fold has
    // in hand at the moment the commit arrives.
    let rs = crate::core::fold(events);

    let mut out = String::new();
    match ticket {
        Some(t) => {
            let _ = writeln!(out, "# Ledger digest — {t}");
        }
        None => {
            let _ = writeln!(out, "# Ledger digest");
        }
    }
    let _ = writeln!(out, "totals: {}", crate::output::fmt_totals(&rs.totals));

    out.push_str("\n## Recent transitions\n");
    let tail = &rs.hops[rs.hops.len().saturating_sub(last_n)..];
    if tail.is_empty() {
        out.push_str("(none yet)\n");
    }
    for hop in tail {
        let rationale = hop
            .rationale
            .as_deref()
            .map(|r| truncate(r, MAX_RATIONALE_CHARS))
            .unwrap_or_else(|| "(no rationale recorded)".to_string());
        let _ = writeln!(
            out,
            "- cycle {}: {} -> {} — {rationale}",
            hop.cycle, hop.from, hop.to
        );
    }

    out.push_str("\n## Artifacts\n");
    if rs.artifacts.is_empty() {
        out.push_str("(none)\n");
    } else {
        for (name, path) in &rs.artifacts {
            let _ = writeln!(out, "- {name}: {path}");
        }
    }

    out
}

// `worker_digest_for_judge` used to live here. It moved to `core::runner`,
// beside the `JudgeSpec` field it fills: it is pure, and filing it under
// `ledger` put it out of `engine`'s reach and cost the engine's fake a second
// implementation of it. `render` above stays, because it does depend on this
// layer's neighbour `output::fmt_totals`.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::fixtures::{EventExt, committed, guard_checked, output, proposed, started};
    use crate::core::{Event, RunStatus};

    fn sample_events(n_transitions: usize) -> Vec<Event> {
        let mut events = vec![started("PROJ-1")];
        for i in 0..n_transitions {
            let from = format!("state{i}");
            let to = format!("state{}", i + 1);
            events.push(
                output(&from, 1)
                    .summary(&format!("did work in {from}"))
                    .usage(10, 0.1),
            );
            events.push(proposed(&from, &to).rationale(&format!("rationale {i}")));
            events.push(committed(&from, &to, 1));
        }
        events
    }

    #[test]
    fn includes_ticket_and_totals() {
        let events = sample_events(2);
        let digest = render(&events, 10);
        assert!(digest.contains("PROJ-1"));
        assert!(digest.contains("$0.20"));
        assert!(digest.contains("2 transition"));
    }

    #[test]
    fn last_n_truncates_to_most_recent_transitions() {
        let events = sample_events(5);
        let digest = render(&events, 2);
        // Only the last two transitions' rationales should appear.
        assert!(digest.contains("rationale 3"));
        assert!(digest.contains("rationale 4"));
        assert!(!digest.contains("rationale 0"));
        assert!(!digest.contains("rationale 1"));
        assert!(!digest.contains("rationale 2"));
    }

    #[test]
    fn last_n_zero_yields_no_transitions_but_no_panic() {
        let events = sample_events(3);
        let digest = render(&events, 0);
        assert!(digest.contains("none yet"));
    }

    #[test]
    fn empty_ledger_does_not_panic() {
        let digest = render(&[], 8);
        assert!(digest.contains("Ledger digest"));
        assert!(digest.contains("none yet"));
    }

    #[test]
    fn never_includes_a_raw_transcript() {
        let mut events = sample_events(1);
        let huge_summary = "TRANSCRIPT-MARKER-".to_string() + &"x".repeat(10_000);
        events.push(output("state1", 1).summary(&huge_summary));
        events.push(crate::core::fixtures::finished(RunStatus::Done, "done"));

        let digest = render(&events, 8);
        assert!(
            !digest.contains("TRANSCRIPT-MARKER"),
            "worker_output.summary must never leak into the digest"
        );
        assert!(digest.len() < 5_000, "digest ballooned: {}", digest.len());
    }

    #[test]
    fn includes_artifact_table() {
        let mut events = sample_events(0);
        events.push(output("implement", 1).artifact("diff", ".loop/artifacts/implement-1-diff"));

        let digest = render(&events, 8);
        assert!(digest.contains("diff: .loop/artifacts/implement-1-diff"));
    }

    /// The Judge's spend is on `guard_checked`, and the digest's totals line
    /// is where a human reading a stage's prompt sees what the run has cost.
    /// Leaving it out understated a criteria-heavy machine by the whole
    /// criteria tier.
    #[test]
    fn totals_include_the_judges_spend() {
        let mut events = sample_events(1);
        events.push(
            guard_checked("state0", "state1")
                .guards(
                    crate::core::GuardOutcome::Skip,
                    crate::core::GuardOutcome::Pass,
                )
                .usage(900, 0.4),
        );

        let digest = render(&events, 8);
        assert!(digest.contains("$0.50"), "got: {digest}");
        assert!(digest.contains("910 token(s)"), "got: {digest}");
    }

    #[test]
    fn long_rationale_is_truncated() {
        let mut events = vec![started("T")];
        let long_rationale = "a".repeat(1000);
        events.push(proposed("a", "b").rationale(&long_rationale));
        events.push(committed("a", "b", 1));

        let digest = render(&events, 8);
        assert!(!digest.contains(&long_rationale));
        assert!(digest.len() < long_rationale.len());
    }
}
