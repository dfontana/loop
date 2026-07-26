//! The pure half of `loop session`: turning the ledger into selectable Worker
//! attempts, and reducing keystrokes into a selection.
//!
//! Nothing here touches a terminal, a process, or the filesystem. The terminal
//! layer in `session_ui` owns rendering and key decoding; this module owns what
//! a candidate *is*, which candidates a mode shows, how a query ranks them, and
//! which one a highlighted row means. That split is what makes the interesting
//! part testable without a PTY — and what makes it provable that two rows which
//! *read* identically still open different sessions.
//!
//! The internal identity of a candidate is its **ledger ordinal**: the index of
//! its `state_entered` event in the event list. It is never persisted and never
//! shown. The opaque `session_id` is likewise a selection key handed to pi, not
//! something a human is asked to recognize.

use loop_core::{ArtifactRef, Event, EventPayload, Usage};

/// The index of a candidate's `state_entered` event in the ledger it came from.
///
/// A newtype rather than a bare `usize` because the picker also deals in
/// *visible row* indices, and confusing the two is exactly how a duplicate-
/// looking row opens the wrong session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CandidateOrdinal(pub usize);

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
    pub ordinal: CandidateOrdinal,
    /// The id pi persisted the session under. Non-empty by construction.
    pub session_id: String,
    pub state: String,
    pub cycle: u32,
    pub attempt: u32,
    /// The `state_entered` timestamp, ISO-8601 as written.
    pub ts: String,
    pub model: String,
    pub thinking: String,
    /// The Worker's own digest, when it got as far as reporting one.
    pub summary: Option<String>,
    pub usage: Option<Usage>,
    pub artifacts: Vec<ArtifactRef>,
    /// Error details recorded inside this attempt's episode.
    pub errors: Vec<String>,
    pub outcome: Outcome,
}

impl Candidate {
    pub fn is_complete(&self) -> bool {
        self.outcome == Outcome::Finished
    }

    /// `2026-07-26 12:04` — minutes are enough to disambiguate attempts, and a
    /// full RFC-3339 stamp crowds out the summary.
    pub fn short_ts(&self) -> String {
        match chrono::DateTime::parse_from_rfc3339(&self.ts) {
            Ok(dt) => dt
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string(),
            // A hand-edited or foreign timestamp still has to render: the row
            // is for recognition, and an unparseable stamp is better shown than
            // swallowed.
            Err(_) => self.ts.clone(),
        }
    }

    /// The first line of a row: state, cycle/attempt, time, outcome.
    ///
    /// `description` is the machine's own text for the state when a machine
    /// happens to load. It *enriches*, never replaces — the state id stays
    /// visible so no two rows collapse into each other.
    pub fn headline(&self, description: Option<&str>) -> String {
        let mut line = format!(
            "{} — cycle {}, attempt {} — {} — {}",
            self.state,
            self.cycle,
            self.attempt,
            self.short_ts(),
            self.outcome.label(),
        );
        if let Some(d) = description.map(str::trim).filter(|d| !d.is_empty()) {
            line.push_str(&format!(" — {}", one_line(d)));
        }
        line
    }

    /// The indented second line, when the Worker left a summary.
    pub fn detail(&self) -> Option<String> {
        self.summary
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(one_line)
    }

    /// What ran this attempt and what it cost — `claude-sonnet-5:medium · $0.02
    /// · 1 artifact`.
    ///
    /// Kept off [`Self::haystack`] on purpose. The contract lists what fuzzy
    /// search covers, and a model name is a thing you read off a row rather than
    /// a thing you search for; letting it match would let a query for `sonnet`
    /// return every attempt in the run.
    pub fn meta(&self) -> String {
        let mut parts = vec![format!("{}:{}", self.model, self.thinking)];
        if let Some(u) = &self.usage {
            parts.push(format!("${:.2}", u.cost_usd));
        }
        match self.artifacts.len() {
            0 => {}
            1 => parts.push("1 artifact".to_string()),
            n => parts.push(format!("{n} artifacts")),
        }
        parts.join(" · ")
    }

    /// What fuzzy search matches against: everything visible on the row, and
    /// deliberately **not** the session id — a query must never have to name an
    /// opaque id to find an attempt.
    pub fn haystack(&self, description: Option<&str>) -> String {
        let mut s = self.headline(description);
        if let Some(d) = self.detail() {
            s.push(' ');
            s.push_str(&d);
        }
        s
    }
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Which attempts the picker is offering. `Ctrl+O` walks the cycle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scope {
    /// Every `state_entered` with a usable session id.
    #[default]
    All,
    /// The newest usable attempt for each exact state.
    LatestPerState,
    /// Attempts with no matching `worker_output` yet.
    Incomplete,
}

impl Scope {
    pub fn next(self) -> Self {
        match self {
            Self::All => Self::LatestPerState,
            Self::LatestPerState => Self::Incomplete,
            Self::Incomplete => Self::All,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All attempts",
            Self::LatestPerState => "Latest per state",
            Self::Incomplete => "Incomplete",
        }
    }
}

/// Build one candidate per usable `state_entered`, **newest first**.
///
/// Association is by *ledger episode*: a candidate owns the events from its own
/// `state_entered` up to the next one. Within that window the matching
/// `worker_output` (same state and cycle) and any errors belong to it.
/// `worker_output` carries no attempt field, so bounding by the episode is what
/// keeps attempt 2's summary from being credited to attempt 1 — and it needs no
/// change to the ledger's wire format to do it.
///
/// A `state_entered` with no session id is dropped: there is nothing to reopen.
/// Judge and Navigator spawns are sessionless by design and never appear here.
pub fn candidates(events: &[Event]) -> Vec<Candidate> {
    // Where each episode ends, so the scan below is a single pass with a known
    // right edge rather than a nested search.
    let entered: Vec<usize> = events
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e.payload, EventPayload::StateEntered { .. }))
        .map(|(i, _)| i)
        .collect();

    let mut out = Vec::new();
    for (n, &start) in entered.iter().enumerate() {
        let end = entered.get(n + 1).copied().unwrap_or(events.len());
        let EventPayload::StateEntered {
            state,
            cycle,
            attempt,
            session_id,
            model,
            thinking,
            ..
        } = &events[start].payload
        else {
            continue;
        };
        let Some(session_id) = session_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };

        let mut summary = None;
        let mut usage = None;
        let mut artifacts = Vec::new();
        let mut errors = Vec::new();
        for e in &events[start + 1..end] {
            match &e.payload {
                EventPayload::WorkerOutput {
                    state: s,
                    cycle: c,
                    summary: text,
                    artifacts: arts,
                    usage: u,
                } if s == state && c == cycle => {
                    summary = Some(text.clone());
                    usage = Some(*u);
                    artifacts = arts.clone();
                }
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

        out.push(Candidate {
            ordinal: CandidateOrdinal(start),
            session_id: session_id.to_string(),
            state: state.clone(),
            cycle: *cycle,
            attempt: *attempt,
            ts: events[start].ts.clone(),
            model: model.clone(),
            thinking: thinking.clone(),
            summary,
            usage,
            artifacts,
            errors,
            outcome,
        });
    }

    // Newest-first in reverse ledger order. The timestamp is display metadata,
    // never the sort key: a hand-edited or clock-skewed `ts` must not be able to
    // reorder history.
    out.reverse();
    out
}

/// The ticket this ledger belongs to, for the header and the selection line.
pub fn ticket(events: &[Event]) -> Option<String> {
    events.iter().find_map(|e| match &e.payload {
        EventPayload::RunStarted { ticket, .. } => Some(ticket.clone()),
        _ => None,
    })
}

/// Keep only attempts at exactly this state. An *exact* prefilter, not a fuzzy
/// one: `loop session implement` must not also offer `implement-hotfix`.
pub fn filter_state<'a>(candidates: &'a [Candidate], state: Option<&str>) -> Vec<&'a Candidate> {
    candidates
        .iter()
        .filter(|c| state.is_none_or(|s| c.state == s))
        .collect()
}

/// Apply a `Ctrl+O` scope to an already newest-first, state-filtered list.
pub fn apply_scope<'a>(candidates: &[&'a Candidate], scope: Scope) -> Vec<&'a Candidate> {
    match scope {
        Scope::All => candidates.to_vec(),
        Scope::Incomplete => candidates
            .iter()
            .copied()
            .filter(|c| !c.is_complete())
            .collect(),
        Scope::LatestPerState => {
            // The input is newest-first, so the first sighting of a state *is*
            // its newest attempt.
            let mut seen: Vec<&str> = Vec::new();
            let mut out = Vec::new();
            for c in candidates.iter().copied() {
                if !seen.contains(&c.state.as_str()) {
                    seen.push(&c.state);
                    out.push(c);
                }
            }
            out
        }
    }
}

/// Rank by fuzzy match against the visible row text, best first.
///
/// An empty query is not a match-everything pattern, it is *no filter at all*:
/// the newest-first order survives untouched, which is what makes an untyped
/// picker behave like a plain reverse-chronological list.
pub fn rank<'a>(
    candidates: &[&'a Candidate],
    query: &str,
    describe: &dyn Fn(&str) -> Option<String>,
) -> Vec<&'a Candidate> {
    use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
    use nucleo_matcher::{Config, Matcher, Utf32Str};

    if query.trim().is_empty() {
        return candidates.to_vec();
    }
    let mut matcher = Matcher::new(Config::DEFAULT);
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);

    let mut buf = Vec::new();
    let mut scored: Vec<(u32, &'a Candidate)> = candidates
        .iter()
        .copied()
        .filter_map(|c| {
            let hay = c.haystack(describe(&c.state).as_deref());
            pattern
                .score(Utf32Str::new(&hay, &mut buf), &mut matcher)
                .map(|score| (score, c))
        })
        .collect();
    // `sort_by` is stable, so equal scores keep their newest-first order rather
    // than shuffling under the user's cursor between keystrokes.
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored.into_iter().map(|(_, c)| c).collect()
}

/// The `--latest` policy: the last usable candidate in reverse ledger order,
/// after the state prefilter. No scope, no query — automation asking for "the
/// last implement attempt" wants one deterministic answer.
pub fn latest<'a>(candidates: &'a [Candidate], state: Option<&str>) -> Option<&'a Candidate> {
    filter_state(candidates, state).first().copied()
}

/// Supplies a state's description, when a machine happens to load. A closure
/// rather than a map so the picker never depends on one being available.
type Describe<'a> = Box<dyn Fn(&str) -> Option<String> + 'a>;

/// A key the picker understands, decoded from the terminal by `session_ui`.
///
/// Our own enum rather than crossterm's, so the reducer's whole behavior is
/// reachable from a unit test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Backspace,
    Up,
    Down,
    Enter,
    Cancel,
    /// `Ctrl+O`.
    CycleScope,
}

/// What the terminal loop should do after a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Continue,
    Accept(CandidateOrdinal),
    Cancel,
}

/// The picker's whole state. Owns nothing terminal-shaped.
pub struct Picker<'a> {
    /// Newest-first, already state-filtered. Never re-ordered.
    pool: Vec<&'a Candidate>,
    scope: Scope,
    query: String,
    /// Rows as currently shown, in display order.
    visible: Vec<&'a Candidate>,
    cursor: usize,
    describe: Describe<'a>,
}

impl<'a> Picker<'a> {
    /// `describe` supplies an optional state description. It is a closure so a
    /// missing or invalid `machine.fnl` costs the enrichment and nothing else.
    pub fn new(
        candidates: &'a [Candidate],
        state: Option<&str>,
        describe: impl Fn(&str) -> Option<String> + 'a,
    ) -> Self {
        let pool = filter_state(candidates, state);
        let mut picker = Self {
            pool,
            scope: Scope::All,
            query: String::new(),
            visible: Vec::new(),
            cursor: 0,
            describe: Box::new(describe),
        };
        picker.recompute();
        picker
    }

    fn recompute(&mut self) {
        let scoped = apply_scope(&self.pool, self.scope);
        self.visible = rank(&scoped, &self.query, &*self.describe);
        self.cursor = self.cursor.min(self.visible.len().saturating_sub(1));
    }

    pub fn scope(&self) -> Scope {
        self.scope
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn visible(&self) -> &[&'a Candidate] {
        &self.visible
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// The candidate the highlighted row stands for. This is the only mapping
    /// from a row back to a session, so it is the only place a duplicate-looking
    /// row could go wrong.
    pub fn selected(&self) -> Option<&'a Candidate> {
        self.visible.get(self.cursor).copied()
    }

    pub fn describe(&self, state: &str) -> Option<String> {
        (self.describe)(state)
    }

    pub fn on_key(&mut self, key: Key) -> Step {
        match key {
            Key::Cancel => return Step::Cancel,
            Key::Enter => {
                return match self.selected() {
                    Some(c) => Step::Accept(c.ordinal),
                    // Enter on an empty result set must do nothing, not launch
                    // whatever happened to be highlighted before the query.
                    None => Step::Continue,
                };
            }
            Key::Up => self.cursor = self.cursor.saturating_sub(1),
            Key::Down => {
                if self.cursor + 1 < self.visible.len() {
                    self.cursor += 1;
                }
            }
            Key::CycleScope => {
                self.scope = self.scope.next();
                // A scope change is a new list; starting at the top is the only
                // honest cursor, since the old row may not even be present.
                self.cursor = 0;
                self.recompute();
            }
            Key::Char(c) => {
                self.query.push(c);
                self.cursor = 0;
                self.recompute();
            }
            Key::Backspace => {
                self.query.pop();
                self.cursor = 0;
                self.recompute();
            }
        }
        Step::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use loop_core::{ArtifactRef, Budgets, ErrorKind, RunStatus, Totals};

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
    fn candidates_are_newest_first_and_carry_their_episode() {
        let cs = candidates(&ledger());
        let rows: Vec<(&str, u32, Outcome)> = cs
            .iter()
            .map(|c| (c.state.as_str(), c.attempt, c.outcome))
            .collect();
        assert_eq!(
            rows,
            vec![
                ("test", 1, Outcome::Incomplete),
                ("review", 1, Outcome::Finished),
                ("implement", 2, Outcome::Finished),
                ("implement", 1, Outcome::Crashed),
            ]
        );
        assert_eq!(cs[1].summary.as_deref(), Some("Found a defect."));
        assert_eq!(cs[1].artifacts.len(), 1);
        assert_eq!(cs[3].errors, vec!["executor lost".to_string()]);
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
        assert!(crashed.artifacts.is_empty(), "{crashed:?}");
        assert_eq!(crashed.outcome, Outcome::Crashed);
        // …while the retry that *did* report keeps its own summary.
        let retried = cs
            .iter()
            .find(|c| c.state == "implement" && c.attempt == 2)
            .unwrap();
        assert_eq!(retried.summary.as_deref(), Some("Added the guard."));
    }

    #[test]
    fn a_state_entered_without_a_session_id_is_not_selectable() {
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
        // A clock that jumped backwards. Ledger order still wins.
        events.push(entered(
            "1999-01-01T00:00:00.000Z",
            "open-pr",
            1,
            1,
            Some("s-p"),
        ));
        let cs = candidates(&events);
        assert_eq!(cs[0].state, "open-pr");
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
    fn scopes_cycle_and_select_the_right_sets() {
        let cs = candidates(&ledger());
        let all = filter_state(&cs, None);

        assert_eq!(apply_scope(&all, Scope::All).len(), 4);

        let latest = apply_scope(&all, Scope::LatestPerState);
        let rows: Vec<(&str, u32)> = latest
            .iter()
            .map(|c| (c.state.as_str(), c.attempt))
            .collect();
        assert_eq!(rows, vec![("test", 1), ("review", 1), ("implement", 2)]);

        let incomplete = apply_scope(&all, Scope::Incomplete);
        let rows: Vec<(&str, u32)> = incomplete
            .iter()
            .map(|c| (c.state.as_str(), c.attempt))
            .collect();
        assert_eq!(rows, vec![("test", 1), ("implement", 1)]);

        assert_eq!(Scope::All.next(), Scope::LatestPerState);
        assert_eq!(Scope::LatestPerState.next(), Scope::Incomplete);
        assert_eq!(Scope::Incomplete.next(), Scope::All);
    }

    /// The state prefilter is not a mode: switching scope must not widen it back
    /// out to every state.
    #[test]
    fn the_state_prefilter_holds_in_every_scope() {
        let cs = candidates(&ledger());
        let mut picker = Picker::new(&cs, Some("implement"), |_| None);
        for _ in 0..4 {
            assert!(
                picker.visible().iter().all(|c| c.state == "implement"),
                "{} leaked non-implement rows",
                picker.scope().label()
            );
            picker.on_key(Key::CycleScope);
        }
    }

    #[test]
    fn latest_respects_the_state_filter_and_reverse_order() {
        let cs = candidates(&ledger());
        assert_eq!(latest(&cs, None).unwrap().state, "test");
        let li = latest(&cs, Some("implement")).unwrap();
        assert_eq!((li.state.as_str(), li.attempt), ("implement", 2));
        assert!(latest(&cs, Some("nope")).is_none());
    }

    #[test]
    fn a_row_reads_the_way_the_contract_says() {
        let cs = candidates(&ledger());
        let c = cs.iter().find(|c| c.attempt == 2).unwrap();
        // Rendered in the local zone, so assert on the stable parts.
        let headline = c.headline(None);
        assert!(
            headline.starts_with("implement — cycle 1, attempt 2 — "),
            "{headline}"
        );
        assert!(headline.ends_with(" — finished"), "{headline}");
        assert_eq!(c.detail().as_deref(), Some("Added the guard."));
    }

    #[test]
    fn a_description_enriches_the_row_without_hiding_the_state_id() {
        let cs = candidates(&ledger());
        let c = cs.iter().find(|c| c.state == "review").unwrap();
        let headline = c.headline(Some("Review the diff."));
        assert!(headline.starts_with("review — "), "{headline}");
        assert!(headline.ends_with("— Review the diff."), "{headline}");
    }

    #[test]
    fn meta_carries_model_cost_and_artifacts_but_stays_out_of_the_haystack() {
        let cs = candidates(&ledger());
        let finished = cs.iter().find(|c| c.state == "review").unwrap();
        assert_eq!(
            finished.meta(),
            "claude-sonnet-5:medium · $0.02 · 1 artifact"
        );
        assert!(!finished.haystack(None).contains("sonnet"));

        // A crashed attempt has no usage and no artifacts to report.
        let crashed = cs.iter().find(|c| c.outcome == Outcome::Crashed).unwrap();
        assert_eq!(crashed.meta(), "claude-sonnet-5:medium");
    }

    #[test]
    fn fuzzy_search_matches_visible_text_and_not_the_session_id() {
        let cs = candidates(&ledger());
        let all = filter_state(&cs, None);

        let hits = rank(&all, "defect", &|_| None);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].state, "review");

        let hits = rank(&all, "incomplete", &|_| None);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].state, "test");

        // A query naming the opaque id finds nothing: the id is not searchable
        // on purpose, so nobody learns to reach for it.
        assert!(rank(&all, "s-r-1-1", &|_| None).is_empty());
    }

    #[test]
    fn an_empty_query_preserves_newest_first_order() {
        let cs = candidates(&ledger());
        let all = filter_state(&cs, None);
        let ranked = rank(&all, "   ", &|_| None);
        let rows: Vec<&str> = ranked.iter().map(|c| c.state.as_str()).collect();
        assert_eq!(rows, vec!["test", "review", "implement", "implement"]);
    }

    /// The heart of the contract: rows a human cannot tell apart must still map
    /// to distinct sessions, and the mapping must follow the cursor rather than
    /// the text.
    #[test]
    fn duplicate_display_text_cannot_open_the_wrong_session() {
        // Two attempts at the same state, cycle, and wall-clock minute, with the
        // same summary — every visible field identical except the attempt.
        let events = vec![
            entered("2026-07-26T12:00:00.000Z", "flaky", 1, 1, Some("sess-A")),
            output("2026-07-26T12:00:10.000Z", "flaky", 1, "same words"),
            entered("2026-07-26T12:00:20.000Z", "flaky", 1, 1, Some("sess-B")),
            output("2026-07-26T12:00:30.000Z", "flaky", 1, "same words"),
        ];
        let cs = candidates(&events);
        assert_eq!(cs.len(), 2);
        assert_eq!(cs[0].headline(None), cs[1].headline(None));

        let mut picker = Picker::new(&cs, None, |_| None);
        // Newest first: the top row is sess-B.
        assert_eq!(picker.selected().unwrap().session_id, "sess-B");
        assert_eq!(picker.on_key(Key::Down), Step::Continue);
        assert_eq!(picker.selected().unwrap().session_id, "sess-A");
        let ordinal = picker.selected().unwrap().ordinal;
        assert_eq!(picker.on_key(Key::Enter), Step::Accept(ordinal));
        // And the ordinal resolves back to that same session, not the first row
        // whose text matches.
        assert_eq!(
            cs.iter().find(|c| c.ordinal == ordinal).unwrap().session_id,
            "sess-A"
        );
    }

    #[test]
    fn navigation_clamps_at_both_ends() {
        let cs = candidates(&ledger());
        let mut picker = Picker::new(&cs, None, |_| None);
        picker.on_key(Key::Up);
        assert_eq!(picker.cursor(), 0);
        for _ in 0..10 {
            picker.on_key(Key::Down);
        }
        assert_eq!(picker.cursor(), picker.visible().len() - 1);
    }

    #[test]
    fn typing_narrows_and_backspace_widens_again() {
        let cs = candidates(&ledger());
        let mut picker = Picker::new(&cs, None, |_| None);
        for c in "review".chars() {
            picker.on_key(Key::Char(c));
        }
        assert_eq!(picker.query(), "review");
        assert_eq!(picker.visible().len(), 1);
        for _ in 0..6 {
            picker.on_key(Key::Backspace);
        }
        assert_eq!(picker.query(), "");
        assert_eq!(picker.visible().len(), 4);
    }

    #[test]
    fn enter_on_an_empty_result_set_does_not_accept_a_stale_row() {
        let cs = candidates(&ledger());
        let mut picker = Picker::new(&cs, None, |_| None);
        for c in "zzzzzz".chars() {
            picker.on_key(Key::Char(c));
        }
        assert!(picker.visible().is_empty());
        assert_eq!(picker.on_key(Key::Enter), Step::Continue);
        assert_eq!(picker.on_key(Key::Cancel), Step::Cancel);
    }

    #[test]
    fn cycling_scope_resets_the_cursor_to_a_row_that_exists() {
        let cs = candidates(&ledger());
        let mut picker = Picker::new(&cs, None, |_| None);
        for _ in 0..3 {
            picker.on_key(Key::Down);
        }
        assert_eq!(picker.cursor(), 3);
        picker.on_key(Key::CycleScope);
        assert_eq!(picker.scope(), Scope::LatestPerState);
        assert_eq!(picker.cursor(), 0);
        assert!(picker.selected().is_some());
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
    fn an_empty_ledger_yields_no_candidates() {
        assert!(candidates(&[]).is_empty());
        let cs: Vec<Candidate> = Vec::new();
        let picker = Picker::new(&cs, None, |_| None);
        assert!(picker.selected().is_none());
    }
}
