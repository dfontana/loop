//! The rolling ledger digest — the deterministic continuity channel between
//! stages (docs/02-how-it-works.md, "Data flow between stages").
//!
//! Never transcripts: the last N committed transitions with their rationales
//! and pinned artifact references. Cost and drift are the reasons this is a
//! summary and not a replay.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use loop_core::{ArtifactRef, Event, EventPayload};

/// Rationale text longer than this is truncated in the digest. Rationales are
/// meant to be one sentence; a worker padding one out shouldn't be able to
/// balloon every downstream prompt with it.
const MAX_RATIONALE_CHARS: usize = 300;

/// Render the digest fed to a stage as `$LEDGER_DIGEST`.
///
/// Deliberately reads only the *decisions* out of the log — `run_started`,
/// `transition_committed`/`transition_proposed` (for the rationale), and the
/// artifact refs on `worker_output` — and never the
/// `worker_output.summary` prose. That's what keeps this a summary rather
/// than a transcript by construction, not by convention: there's no field in
/// this function that could leak one in.
pub fn render(events: &[Event], last_n: usize) -> String {
    let ticket = events.iter().find_map(|e| match &e.payload {
        EventPayload::RunStarted { ticket, .. } => Some(ticket.clone()),
        _ => None,
    });

    let mut cost_usd = 0.0_f64;
    let mut tokens: u64 = 0;
    let mut transitions: u32 = 0;
    let mut artifacts: BTreeMap<String, String> = BTreeMap::new();
    let mut committed_idx: Vec<usize> = Vec::new();

    for (i, e) in events.iter().enumerate() {
        match &e.payload {
            EventPayload::WorkerOutput {
                usage,
                artifacts: arts,
                ..
            } => {
                cost_usd += usage.cost_usd;
                tokens += usage.tokens;
                for a in arts {
                    artifacts.insert(a.name.clone(), a.path.clone());
                }
            }
            EventPayload::NavigatorInvoked { usage, .. } => {
                cost_usd += usage.cost_usd;
                tokens += usage.tokens;
            }
            EventPayload::TransitionCommitted { .. } => {
                transitions += 1;
                committed_idx.push(i);
            }
            _ => {}
        }
    }

    let mut out = String::new();
    match &ticket {
        Some(t) => {
            let _ = writeln!(out, "# Ledger digest — {t}");
        }
        None => {
            let _ = writeln!(out, "# Ledger digest");
        }
    }
    let _ = writeln!(
        out,
        "totals: ${cost_usd:.2} · {tokens} tokens · {transitions} transition(s)"
    );

    out.push_str("\n## Recent transitions\n");
    let tail_start = committed_idx.len().saturating_sub(last_n);
    if committed_idx[tail_start..].is_empty() {
        out.push_str("(none yet)\n");
    }
    for &i in &committed_idx[tail_start..] {
        if let EventPayload::TransitionCommitted { from, to, cycle } = &events[i].payload {
            let rationale = rationale_for(events, i, from, to)
                .map(|r| truncate(&r, MAX_RATIONALE_CHARS))
                .unwrap_or_else(|| "(no rationale recorded)".to_string());
            let _ = writeln!(out, "- cycle {cycle}: {from} -> {to} — {rationale}");
        }
    }

    out.push_str("\n## Artifacts\n");
    if artifacts.is_empty() {
        out.push_str("(none)\n");
    } else {
        for (name, path) in &artifacts {
            let _ = writeln!(out, "- {name}: {path}");
        }
    }

    out
}

/// The rationale of the `transition_proposed` immediately behind a given
/// `transition_committed`, matched by `(from, to)`. Committed events are
/// always preceded by the proposal they ratify (docs/02-how-it-works.md's control loop:
/// propose -> guard -> commit), so the nearest match walking backward is the
/// right one even across cycles that revisit the same edge.
fn rationale_for(events: &[Event], committed_at: usize, from: &str, to: &str) -> Option<String> {
    events[..committed_at]
        .iter()
        .rev()
        .find_map(|e| match &e.payload {
            EventPayload::TransitionProposed {
                from: f,
                to: t,
                rationale,
                ..
            } if f == from && t.as_deref() == Some(to) => Some(rationale.clone()),
            _ => None,
        })
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars).collect();
    out.push('…');
    out
}

/// A one-paragraph summary of a single stage's output, for the Judge. It must
/// exclude the worker's own pass/fail claim — the Judge grades artifacts, not
/// self-assessment (docs/05-design-notes.md).
///
/// The exclusion is structural, not a filter: this takes `summary` (what
/// `worker_output` records the worker *did*) and the artifact list, never the
/// `transition_proposed.rationale` or the worker's `vars` hints, which are
/// exactly where a self-graded "QA passed" would live. Callers must not paste
/// those in themselves.
pub fn worker_digest_for_judge(summary: &str, artifacts: &[ArtifactRef]) -> String {
    let mut out = String::new();
    out.push_str(summary.trim());
    if !artifacts.is_empty() {
        out.push_str("\n\nArtifacts:\n");
        for a in artifacts {
            let _ = writeln!(out, "- {} ({}) sha256:{}", a.name, a.path, a.sha256);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use loop_core::{Actor, Budgets, Event, EventPayload, RunStatus, Totals, Usage};

    fn ev(payload: EventPayload) -> Event {
        Event::now(payload)
    }

    fn sample_events(n_transitions: usize) -> Vec<Event> {
        let mut events = vec![ev(EventPayload::RunStarted {
            ticket: "PROJ-1".into(),
            machine_hash: "sha256:abc".into(),
            budgets: Budgets::default(),
        })];

        for i in 0..n_transitions {
            let from = format!("state{i}");
            let to = format!("state{}", i + 1);
            events.push(ev(EventPayload::WorkerOutput {
                state: from.clone(),
                cycle: 1,
                summary: format!("did work in {from}"),
                artifacts: vec![],
                usage: Usage {
                    tokens: 10,
                    cost_usd: 0.1,
                },
            }));
            events.push(ev(EventPayload::TransitionProposed {
                from: from.clone(),
                to: Some(to.clone()),
                blocked: false,
                rationale: format!("rationale {i}"),
                by: Actor::Worker,
            }));
            events.push(ev(EventPayload::TransitionCommitted { from, to, cycle: 1 }));
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
        events.push(ev(EventPayload::WorkerOutput {
            state: "state1".into(),
            cycle: 1,
            summary: huge_summary,
            artifacts: vec![],
            usage: Usage::default(),
        }));
        events.push(ev(EventPayload::RunFinished {
            status: RunStatus::Done,
            terminal_state: Some("done".into()),
            totals: Totals::default(),
        }));

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
        events.push(ev(EventPayload::WorkerOutput {
            state: "implement".into(),
            cycle: 1,
            summary: "did stuff".into(),
            artifacts: vec![ArtifactRef {
                name: "diff".into(),
                path: ".loop/artifacts/implement-1-diff".into(),
                sha256: "deadbeef".into(),
            }],
            usage: Usage::default(),
        }));

        let digest = render(&events, 8);
        assert!(digest.contains("diff: .loop/artifacts/implement-1-diff"));
    }

    #[test]
    fn long_rationale_is_truncated() {
        let mut events = vec![ev(EventPayload::RunStarted {
            ticket: "T".into(),
            machine_hash: "h".into(),
            budgets: Budgets::default(),
        })];
        let long_rationale = "a".repeat(1000);
        events.push(ev(EventPayload::TransitionProposed {
            from: "a".into(),
            to: Some("b".into()),
            blocked: false,
            rationale: long_rationale.clone(),
            by: Actor::Worker,
        }));
        events.push(ev(EventPayload::TransitionCommitted {
            from: "a".into(),
            to: "b".into(),
            cycle: 1,
        }));

        let digest = render(&events, 8);
        assert!(!digest.contains(&long_rationale));
        assert!(digest.len() < long_rationale.len());
    }

    #[test]
    fn worker_digest_excludes_pass_fail_self_assessment() {
        let artifacts = vec![ArtifactRef {
            name: "report".into(),
            path: ".loop/artifacts/qa-1-report".into(),
            sha256: "abc123".into(),
        }];
        let digest = worker_digest_for_judge("Ran the QA suite against staging.", &artifacts);
        assert!(digest.contains("Ran the QA suite"));
        assert!(digest.contains("report"));
        // The function has no parameter through which a worker's self-graded
        // verdict could enter — this just documents that the summary text
        // itself (which callers must draw from `worker_output`, not from a
        // `transition` proposal) passes through unedited.
        let lower = digest.to_lowercase();
        assert!(!lower.contains("i passed") && !lower.contains("qa passed"));
    }
}
