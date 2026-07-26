//! What an *attempt* is, as far as the ledger is concerned.
//!
//! One rule, in one place, because two commands depend on it and they must not
//! be allowed to disagree. `loop session` groups attempts to decide which pi
//! session a row reopens; `loop recap` groups them to decide which evidence
//! belongs under which heading. Both are asking the same question.
//!
//! The rule: an attempt owns the events from its own `state_entered` up to the
//! next one. `worker_output` carries no attempt field, so bounding by the
//! episode is what keeps attempt 2's summary from being credited to attempt 1 —
//! and it needs no change to the ledger's wire format to do it.
//!
//! Nothing is dropped here and nothing is reordered. Consumers project: the
//! session picker discards episodes with no session id and reverses; the recap
//! keeps every one, in ledger order, because a failed attempt that produced
//! nothing is exactly what it exists to report.

use loop_core::{Event, EventPayload, StateId};

/// One `state_entered` and everything the harness recorded before the next one.
pub struct Episode<'e> {
    /// The index of this episode's `state_entered` in the ledger it came from.
    ///
    /// The stable identity of an attempt. Never persisted and never shown — the
    /// session picker uses it to map a highlighted row back to the right
    /// session, which is what keeps two identical-looking rows distinct.
    pub ordinal: usize,
    pub entered: &'e Event,
    pub state: &'e StateId,
    pub cycle: u32,
    pub attempt: u32,
    /// Present only when non-empty: a blank id is nothing to reopen.
    pub session_id: Option<&'e str>,
    pub model: &'e str,
    pub thinking: &'e str,
    pub skills: &'e [String],
    pub mcp: &'e [String],
    /// The events after this `state_entered`, up to the next one, in ledger
    /// order.
    pub body: &'e [Event],
}

/// Group a ledger into attempts, in ledger order, losing nothing.
///
/// Pure and total: a truncated or surprising log yields sensible episodes
/// rather than a panic.
pub fn episodes(events: &[Event]) -> Vec<Episode<'_>> {
    // Where each episode ends, so the scan below is a single pass with a known
    // right edge rather than a nested search.
    let entered: Vec<usize> = events
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e.payload, EventPayload::StateEntered { .. }))
        .map(|(i, _)| i)
        .collect();

    let mut out = Vec::with_capacity(entered.len());
    for (n, &start) in entered.iter().enumerate() {
        let end = entered.get(n + 1).copied().unwrap_or(events.len());
        let EventPayload::StateEntered {
            state,
            cycle,
            attempt,
            session_id,
            model,
            thinking,
            skills,
            mcp,
        } = &events[start].payload
        else {
            continue;
        };
        out.push(Episode {
            ordinal: start,
            entered: &events[start],
            state,
            cycle: *cycle,
            attempt: *attempt,
            session_id: session_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty()),
            model,
            thinking,
            skills,
            mcp,
            body: &events[start + 1..end],
        });
    }
    out
}
