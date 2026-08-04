//! The pure half of `loop sessions` and `loop session`: turning the ledger into
//! the Worker attempts a human can reopen, and finding the one they named.
//!
//! Nothing here touches a terminal, a process, or the filesystem. This module
//! owns what a candidate *is*, which candidates a state filter shows, how a
//! session id resolves back to an attempt, and how the listing is laid out —
//! `commands` owns the ledger, the ticket, and handing the terminal to pi.
//!
//! There used to be a full-screen picker here, with a fuzzy matcher and a
//! terminal backend, to choose one string out of a list. The list is now printed
//! and the choosing is the shell's job: `loop sessions | fzf` does what the
//! picker did, and every other pipeline the picker could not.

use crate::core::{Event, EventPayload};

/// What the ledger says became of an attempt, as far as this ledger can tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// A `worker_output` for this state/cycle landed inside the episode.
    Finished,
    /// No `worker_output`, but the episode recorded an error — the spawn died.
    Crashed,
    /// No `worker_output` and no error: still running, or killed without a trace.
    Incomplete,
}

impl Outcome {
    pub fn label(self) -> &'static str {
        match self {
            Self::Finished => "finished",
            Self::Crashed => "crashed",
            Self::Incomplete => "incomplete",
        }
    }
}

/// One Worker attempt a human can reopen.
#[derive(Clone, Debug)]
pub struct Candidate {
    /// The id pi persisted the session under. Non-empty by construction, and
    /// the only thing `loop session` needs to be handed.
    pub session_id: String,
    pub state: String,
    pub cycle: u32,
    pub attempt: u32,
    /// The `state_entered` timestamp, ISO-8601 as written.
    pub ts: String,
    /// The Worker's own digest, when it got as far as reporting one.
    pub summary: Option<String>,
    /// Error details recorded inside this attempt's episode.
    pub errors: Vec<String>,
    pub outcome: Outcome,
}

impl Candidate {
    pub fn is_complete(&self) -> bool {
        self.outcome == Outcome::Finished
    }

    /// `2026-07-26T12:04` in the local zone — minutes are enough to tell two
    /// attempts apart, and a full RFC-3339 stamp is a column nobody reads.
    ///
    /// The `T` is load-bearing: this string is the first column of a listing
    /// that gets split on whitespace, and a space between the date and the time
    /// would shift every field number after it.
    pub fn short_ts(&self) -> String {
        match chrono::DateTime::parse_from_rfc3339(&self.ts) {
            Ok(dt) => dt
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%dT%H:%M")
                .to_string(),
            // A hand-edited or foreign timestamp still has to render: the column
            // is for recognition, and an unparseable stamp is better shown than
            // swallowed.
            Err(_) => one_line(&self.ts),
        }
    }

    /// How `loop session` names the attempt it is about to open, for a human who
    /// just typed an opaque id and deserves to be told what it was.
    pub fn headline(&self) -> String {
        format!(
            "{} — cycle {}, attempt {} — {} — {}",
            self.state,
            self.cycle,
            self.attempt,
            self.short_ts(),
            self.outcome.label(),
        )
    }

    /// The Worker's summary, collapsed to one line, when it left one.
    pub fn detail(&self) -> Option<String> {
        self.summary
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(one_line)
    }

    /// The most informative thing the ledger knows about this attempt, in one
    /// line — the summary, else the failure that replaced it, else nothing.
    fn evidence(&self) -> Option<String> {
        self.detail().or_else(|| {
            self.errors
                .first()
                .map(|e| format!("error: {}", one_line(e)))
        })
    }
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Build one candidate per usable `state_entered`, **in ledger order**.
///
/// A projection of [`crate::episode`]: that module owns which events belong to
/// which attempt, and this one owns what a *reopenable* attempt is. Within an
/// episode the matching `worker_output` (same state and cycle) and any errors
/// belong to it.
///
/// A `state_entered` with no session id is dropped: there is nothing to reopen.
/// Judge and Navigator spawns are sessionless by design and never appear here.
///
/// Ledger order, oldest first, is the order the listing prints and the order
/// [`latest`] reads from the end of. The timestamp is display metadata and never
/// the sort key: a hand-edited or clock-skewed `ts` must not be able to reorder
/// history.
pub fn candidates(events: &[Event]) -> Vec<Candidate> {
    crate::episode::episodes(events)
        .into_iter()
        .filter_map(|ep| {
            let session_id = ep.session_id?;

            let mut summary = None;
            let mut errors = Vec::new();
            for e in ep.body {
                match &e.payload {
                    EventPayload::WorkerOutput {
                        state: s,
                        cycle: c,
                        summary: text,
                        ..
                    } if s == ep.state && *c == ep.cycle => summary = Some(text.clone()),
                    EventPayload::Error { detail, .. } => errors.push(detail.clone()),
                    _ => {}
                }
            }

            let outcome = if summary.is_some() {
                Outcome::Finished
            } else if errors.is_empty() {
                Outcome::Incomplete
            } else {
                Outcome::Crashed
            };

            Some(Candidate {
                session_id: session_id.to_string(),
                state: ep.state.clone(),
                cycle: ep.cycle,
                attempt: ep.attempt,
                ts: ep.entered.ts.clone(),
                summary,
                errors,
                outcome,
            })
        })
        .collect()
}

/// The ticket this ledger belongs to, for the line `loop session` prints.
pub fn ticket(events: &[Event]) -> Option<String> {
    events.iter().find_map(|e| match &e.payload {
        EventPayload::RunStarted { ticket, .. } => Some(ticket.clone()),
        _ => None,
    })
}

/// Keep only attempts at exactly this state. An *exact* filter, not a fuzzy one:
/// `loop sessions implement` must not also list `implement-hotfix`.
pub fn filter_state<'a>(candidates: &'a [Candidate], state: Option<&str>) -> Vec<&'a Candidate> {
    candidates
        .iter()
        .filter(|c| state.is_none_or(|s| c.state == s))
        .collect()
}

/// The `--latest` policy: the last candidate in ledger order, after the state
/// filter. Automation asking for "the last implement attempt" wants one
/// deterministic answer, so there is nothing configurable about it.
pub fn latest<'a>(candidates: &'a [Candidate], state: Option<&str>) -> Option<&'a Candidate> {
    filter_state(candidates, state).last().copied()
}

/// Resolve the id a human typed back to its attempt.
///
/// Searched from the end, so when a ledger carries the same id twice — a resumed
/// attempt re-enters the same state, cycle, and attempt, and `stage.rs` derives
/// the id from exactly those four — the winner is the later episode, whose
/// evidence is the one that describes the session as it now stands.
pub fn find<'a>(candidates: &'a [Candidate], id: &str) -> Option<&'a Candidate> {
    candidates.iter().rev().find(|c| c.session_id == id)
}

/// The distinct states with a reopenable attempt, in first-seen ledger order.
///
/// Only used to tell someone who typed a state where `loop session` wants an id
/// what they actually did — which is the difference between a dead end and a
/// working command.
pub fn states(candidates: &[Candidate]) -> Vec<&str> {
    let mut out: Vec<&str> = Vec::new();
    for c in candidates {
        if !out.contains(&c.state.as_str()) {
            out.push(&c.state);
        }
    }
    out
}

/// The `loop sessions` listing: one line per attempt, oldest first.
///
/// Written to be read by `awk` and `cut` as much as by a person. Every column
/// but the last holds a single whitespace-free token — the timestamp joins its
/// date and time with a `T`, and session ids are slugged by `stage.rs` — so
/// `awk '{print $6}'` is the session id no matter how wide the state names in
/// this particular ledger got. The evidence column is last precisely because it
/// is the only one that can contain spaces, and it is collapsed to one line and
/// truncated so a chatty Worker cannot wrap the row.
///
/// Columns are padded to the width of the rows actually printed rather than to
/// a fixed guess, which is what keeps a run of short state names from being
/// spread across half a terminal.
pub fn listing(candidates: &[&Candidate]) -> String {
    let cells: Vec<[String; 6]> = candidates
        .iter()
        .map(|c| {
            [
                c.short_ts(),
                c.state.clone(),
                c.cycle.to_string(),
                c.attempt.to_string(),
                c.outcome.label().to_string(),
                c.session_id.clone(),
            ]
        })
        .collect();

    let mut widths = [0usize; 6];
    for row in &cells {
        for (w, cell) in widths.iter_mut().zip(row) {
            *w = (*w).max(cell.chars().count());
        }
    }

    let mut out = String::new();
    for (row, c) in cells.iter().zip(candidates) {
        let mut line = String::new();
        for (i, (cell, w)) in row.iter().zip(widths).enumerate() {
            if i > 0 {
                line.push_str("  ");
            }
            // Cycle and attempt are counts, so they align on the right; a
            // two-digit cycle must not push the columns after it around.
            match i {
                2 | 3 => line.push_str(&format!("{cell:>w$}")),
                _ => line.push_str(&format!("{cell:<w$}")),
            }
        }
        if let Some(evidence) = c.evidence() {
            line.push_str("  ");
            line.push_str(&crate::output::truncate(&evidence, 72));
        }
        // No trailing padding: an attempt that reported nothing must not leave
        // whitespace for a `grep -E '…$'` to trip over.
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{ArtifactRef, Budgets, ErrorKind, RunStatus, Totals, Usage};

    fn ev(ts: &str, payload: EventPayload) -> Event {
        Event {
            ts: ts.into(),
            elapsed_s: 0,
            payload,
        }
    }

    fn entered(ts: &str, state: &str, cycle: u32, attempt: u32, session: Option<&str>) -> Event {
        ev(
            ts,
            EventPayload::StateEntered {
                state: state.into(),
                cycle,
                attempt,
                session_id: session.map(str::to_string),
                model: "claude-sonnet-5".into(),
                thinking: "medium".into(),
                skills: vec![],
                mcp: vec![],
            },
        )
    }

    fn output(ts: &str, state: &str, cycle: u32, summary: &str) -> Event {
        ev(
            ts,
            EventPayload::WorkerOutput {
                state: state.into(),
                cycle,
                summary: summary.into(),
                artifacts: vec![ArtifactRef {
                    name: "diff".into(),
                    path: ".loop/artifacts/d.patch".into(),
                }],
                usage: Usage {
                    tokens: 10,
                    cost_usd: 0.02,
                },
            },
        )
    }

    /// The shape every test below reads against: two `implement` attempts (the
    /// first crashed), one `review`, and one still-running `test`.
    fn ledger() -> Vec<Event> {
        vec![
            ev(
                "2026-07-26T12:00:00.000Z",
                EventPayload::RunStarted {
                    ticket: "PROJ-1".into(),
                    machine_hash: "x".into(),
                    budgets: Budgets::default(),
                },
            ),
            entered(
                "2026-07-26T12:01:00.000Z",
                "implement",
                1,
                1,
                Some("s-i-1-1"),
            ),
            ev(
                "2026-07-26T12:02:00.000Z",
                EventPayload::Error {
                    state: Some("implement".into()),
                    kind: ErrorKind::Transient,
                    detail: "executor lost".into(),
                },
            ),
            entered(
                "2026-07-26T12:03:00.000Z",
                "implement",
                1,
                2,
                Some("s-i-1-2"),
            ),
            output(
                "2026-07-26T12:04:00.000Z",
                "implement",
                1,
                "Added the guard.",
            ),
            entered("2026-07-26T12:05:00.000Z", "review", 1, 1, Some("s-r-1-1")),
            output("2026-07-26T12:06:00.000Z", "review", 1, "Found a defect."),
            entered("2026-07-26T12:07:00.000Z", "test", 1, 1, Some("s-t-1-1")),
        ]
    }

    #[test]
    fn candidates_are_in_ledger_order_and_carry_their_episode() {
        let cs = candidates(&ledger());
        let rows: Vec<(&str, u32, Outcome)> = cs
            .iter()
            .map(|c| (c.state.as_str(), c.attempt, c.outcome))
            .collect();
        assert_eq!(
            rows,
            vec![
                ("implement", 1, Outcome::Crashed),
                ("implement", 2, Outcome::Finished),
                ("review", 1, Outcome::Finished),
                ("test", 1, Outcome::Incomplete),
            ]
        );
        assert_eq!(cs[2].summary.as_deref(), Some("Found a defect."));
        assert_eq!(cs[0].errors, vec!["executor lost".to_string()]);
    }

    /// `worker_output` has no attempt field, so the only thing keeping attempt
    /// 2's summary off attempt 1's row is the episode boundary. If this ever
    /// regresses, a crashed attempt starts advertising the retry's work.
    #[test]
    fn a_summary_never_leaks_backwards_into_an_earlier_attempt() {
        let cs = candidates(&ledger());
        let crashed = cs
            .iter()
            .find(|c| c.state == "implement" && c.attempt == 1)
            .unwrap();
        assert!(crashed.summary.is_none(), "{crashed:?}");
        assert_eq!(crashed.outcome, Outcome::Crashed);
        // …while the retry that *did* report keeps its own summary.
        let retried = cs
            .iter()
            .find(|c| c.state == "implement" && c.attempt == 2)
            .unwrap();
        assert_eq!(retried.summary.as_deref(), Some("Added the guard."));
    }

    #[test]
    fn a_state_entered_without_a_session_id_is_not_listed() {
        let mut events = ledger();
        events.push(entered("2026-07-26T12:08:00.000Z", "open-pr", 1, 1, None));
        events.push(entered("2026-07-26T12:09:00.000Z", "qa", 1, 1, Some("  ")));
        let cs = candidates(&events);
        assert!(cs.iter().all(|c| c.state != "open-pr"));
        assert!(cs.iter().all(|c| c.state != "qa"));
    }

    #[test]
    fn ordering_ignores_a_skewed_timestamp() {
        let mut events = ledger();
        // A clock that jumped backwards. Ledger order still wins, so the attempt
        // that ran last is still the one `--latest` opens.
        events.push(entered(
            "1999-01-01T00:00:00.000Z",
            "open-pr",
            1,
            1,
            Some("s-p"),
        ));
        let cs = candidates(&events);
        assert_eq!(cs.last().unwrap().state, "open-pr");
        assert_eq!(latest(&cs, None).unwrap().session_id, "s-p");
    }

    #[test]
    fn state_filter_is_exact_not_a_prefix() {
        let mut events = ledger();
        events.push(entered(
            "2026-07-26T12:08:00.000Z",
            "implement-hotfix",
            1,
            1,
            Some("s-h"),
        ));
        let cs = candidates(&events);
        let filtered = filter_state(&cs, Some("implement"));
        assert_eq!(filtered.len(), 2);
        assert!(filtered.iter().all(|c| c.state == "implement"));
    }

    #[test]
    fn latest_respects_the_state_filter_and_ledger_order() {
        let cs = candidates(&ledger());
        assert_eq!(latest(&cs, None).unwrap().state, "test");
        let li = latest(&cs, Some("implement")).unwrap();
        assert_eq!((li.state.as_str(), li.attempt), ("implement", 2));
        assert!(latest(&cs, Some("nope")).is_none());
    }

    /// The whole contract of `loop session <ID>`: the id selects the attempt,
    /// and nothing about the attempt's text can make another one answer for it.
    #[test]
    fn an_id_resolves_to_exactly_its_own_attempt() {
        let cs = candidates(&ledger());
        let found = find(&cs, "s-i-1-1").unwrap();
        assert_eq!((found.state.as_str(), found.attempt), ("implement", 1));
        assert_eq!(found.outcome, Outcome::Crashed);
        assert!(find(&cs, "implement").is_none());
        assert!(find(&cs, "s-i-1-1 ").is_none());
    }

    /// Two attempts can share an id — a resume re-enters the same state, cycle
    /// and attempt, and the id is derived from exactly those. The later episode
    /// is the one that describes the session as it now stands.
    #[test]
    fn a_repeated_id_resolves_to_the_later_episode() {
        let events = vec![
            entered("2026-07-26T12:00:00.000Z", "flaky", 1, 1, Some("dup")),
            ev(
                "2026-07-26T12:00:10.000Z",
                EventPayload::Error {
                    state: Some("flaky".into()),
                    kind: ErrorKind::Transient,
                    detail: "executor lost".into(),
                },
            ),
            entered("2026-07-26T12:00:20.000Z", "flaky", 1, 1, Some("dup")),
            output("2026-07-26T12:00:30.000Z", "flaky", 1, "second time lucky"),
        ];
        let cs = candidates(&events);
        assert_eq!(cs.len(), 2);
        let found = find(&cs, "dup").unwrap();
        assert_eq!(found.outcome, Outcome::Finished);
        assert_eq!(found.detail().as_deref(), Some("second time lucky"));
    }

    /// The listing is a pipeline's input before it is a screen's: whitespace
    /// splitting must give the same field number in every row, whatever the
    /// state names and ids in this ledger look like.
    #[test]
    fn every_listing_row_puts_the_session_id_in_the_same_field() {
        let mut events = ledger();
        events.push(entered(
            "2026-07-26T12:08:00.000Z",
            "a-very-long-state-name",
            12,
            3,
            Some("s-long"),
        ));
        let cs = candidates(&events);
        let text = listing(&filter_state(&cs, None));

        let ids: Vec<&str> = text
            .lines()
            .map(|l| l.split_whitespace().nth(5).unwrap())
            .collect();
        assert_eq!(
            ids,
            vec!["s-i-1-1", "s-i-1-2", "s-r-1-1", "s-t-1-1", "s-long"]
        );
        // Columns line up, and no row ends in the padding that would make it.
        let id_columns: Vec<usize> = text
            .lines()
            .map(|l| l.find(l.split_whitespace().nth(5).unwrap()).unwrap())
            .collect();
        assert!(
            id_columns.windows(2).all(|w| w[0] == w[1]),
            "ragged id column in:\n{text}"
        );
        assert!(
            text.lines().all(|l| l.trim_end() == l),
            "trailing whitespace in:\n{text}"
        );
    }

    /// Every field a human needs to choose a row without opening anything: what
    /// ran, when, how it ended, and the evidence for that.
    #[test]
    fn a_listing_row_carries_the_evidence_for_its_outcome() {
        let cs = candidates(&ledger());
        let text = listing(&filter_state(&cs, None));
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 4);

        assert!(lines[0].contains("implement"), "{}", lines[0]);
        assert!(lines[0].contains("crashed"), "{}", lines[0]);
        // A crash has no summary, so the recorded error stands in for one.
        assert!(lines[0].ends_with("error: executor lost"), "{}", lines[0]);
        assert!(lines[1].ends_with("Added the guard."), "{}", lines[1]);
        // Nothing to say about an attempt still running — so the row stops after
        // its id rather than padding out an empty evidence column.
        assert!(lines[3].contains("incomplete"), "{}", lines[3]);
        assert!(lines[3].ends_with("s-t-1-1"), "{}", lines[3]);
    }

    /// A summary that spans lines would break one-row-per-attempt, which is the
    /// only promise the listing makes to a pipeline.
    #[test]
    fn a_multiline_summary_still_occupies_one_row() {
        let events = vec![
            entered("2026-07-26T12:00:00.000Z", "implement", 1, 1, Some("s-1")),
            output(
                "2026-07-26T12:01:00.000Z",
                "implement",
                1,
                "first line\nsecond line\tand a tab",
            ),
        ];
        let cs = candidates(&events);
        let text = listing(&filter_state(&cs, None));
        assert_eq!(text.lines().count(), 1);
        assert!(text.contains("first line second line and a tab"), "{text}");
    }

    #[test]
    fn states_are_distinct_and_in_first_seen_order() {
        let cs = candidates(&ledger());
        assert_eq!(states(&cs), vec!["implement", "review", "test"]);
        assert!(states(&[]).is_empty());
    }

    #[test]
    fn the_opening_line_reads_the_way_the_contract_says() {
        let cs = candidates(&ledger());
        let c = cs.iter().find(|c| c.attempt == 2).unwrap();
        // Rendered in the local zone, so assert on the stable parts.
        let headline = c.headline();
        assert!(
            headline.starts_with("implement — cycle 1, attempt 2 — "),
            "{headline}"
        );
        assert!(headline.ends_with(" — finished"), "{headline}");
        assert_eq!(c.detail().as_deref(), Some("Added the guard."));
    }

    #[test]
    fn ticket_comes_off_run_started_and_is_absent_without_one() {
        assert_eq!(ticket(&ledger()).as_deref(), Some("PROJ-1"));
        assert!(ticket(&[]).is_none());
        assert!(
            ticket(&[ev(
                "2026-07-26T12:00:00.000Z",
                EventPayload::RunFinished {
                    status: RunStatus::Done,
                    terminal_state: None,
                    totals: Totals::default(),
                }
            )])
            .is_none()
        );
    }

    #[test]
    fn an_empty_ledger_yields_no_candidates_and_no_listing() {
        assert!(candidates(&[]).is_empty());
        assert!(latest(&[], None).is_none());
        assert!(listing(&[]).is_empty());
    }
}
