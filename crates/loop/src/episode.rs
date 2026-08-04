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
//! Nothing is dropped here and nothing is reordered. Consumers project:
//! `sessions` discards episodes with no session id, since there is nothing to
//! reopen; the recap keeps every one, because a failed attempt that produced
//! nothing is exactly what it exists to report.

use crate::core::{Event, EventPayload, StateEntered};

/// One `state_entered` and everything the harness recorded before the next one.
///
/// The entry's fields are reached through [`Episode::header`] rather than
/// copied out, so `state_entered` is declared once.
#[derive(Clone, Copy, Debug)]
pub struct Episode<'e> {
    /// The index of this episode's `state_entered` in the ledger it came from.
    ///
    /// The stable identity of an attempt, never persisted and never shown. It
    /// is also the only way back to the events *before* the first attempt,
    /// which is where the recap finds `run_started`.
    pub ordinal: usize,
    pub entered: &'e Event,
    pub header: &'e StateEntered,
    /// The events after this `state_entered`, up to the next one, in ledger
    /// order.
    pub body: &'e [Event],
}

impl<'e> Episode<'e> {
    pub fn state(&self) -> &'e str {
        &self.header.state
    }

    pub fn cycle(&self) -> u32 {
        self.header.cycle
    }

    pub fn attempt(&self) -> u32 {
        self.header.attempt
    }

    /// Present only when non-empty: a blank id is nothing to reopen.
    pub fn session(&self) -> Option<&'e str> {
        self.header.session()
    }
}

/// Group a ledger into attempts, in ledger order, losing nothing.
///
/// Pure and total: a truncated or surprising log yields sensible episodes
/// rather than a panic.
pub fn episodes(events: &[Event]) -> Vec<Episode<'_>> {
    // Where each episode ends, so the scan below is a single pass with a known
    // right edge rather than a nested search.
    let starts: Vec<usize> = events
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e.payload, EventPayload::StateEntered(_)))
        .map(|(i, _)| i)
        .collect();

    let mut out = Vec::with_capacity(starts.len());
    for (n, &start) in starts.iter().enumerate() {
        let end = starts.get(n + 1).copied().unwrap_or(events.len());
        let EventPayload::StateEntered(header) = &events[start].payload else {
            continue;
        };
        out.push(Episode {
            ordinal: start,
            entered: &events[start],
            header,
            body: &events[start + 1..end],
        });
    }
    out
}
