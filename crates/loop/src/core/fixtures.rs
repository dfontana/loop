//! Ledger event builders, for tests anywhere in the crate.
//!
//! Every builder returns a whole [`Event`] stamped with [`TS`]. The
//! [`EventExt`] setters cover the fields individual tests actually vary; a test
//! that needs a payload rather than an event reaches for `.payload`.
//!
//! A fixed default timestamp rather than `Utc::now()`: a fixture that changes
//! on every run cannot be asserted on, and `report`'s "same ledger renders the
//! same report" test wants the whole input to be a constant.

use crate::core::event::{
    Artifact, ErrorKind, Event, EventPayload, GuardOutcome, RunStatus, StateEntered, Totals, Usage,
};
use crate::core::machine::Budgets;

/// The timestamp every builder stamps unless a test says otherwise.
pub const TS: &str = "2026-07-26T12:00:00.000Z";

/// The machine source every fixture ledger claims to have run.
///
/// Hashed through [`crate::core::machine_hash`] rather than written as a
/// `"sha256:test"` literal, so a fixture's `run_started` carries a hash the
/// shape `loop recap` compares against — the real one is bare hex, and a
/// fixture that spells it with a prefix is a fixture no provenance check could
/// ever match.
pub const MACHINE_SRC: &str = "{:ticket \"T-1\"}";

fn ev(payload: EventPayload) -> Event {
    Event {
        ts: TS.into(),
        elapsed_s: 0,
        payload,
    }
}

pub fn started(ticket: &str) -> Event {
    ev(EventPayload::RunStarted {
        ticket: ticket.into(),
        machine_hash: crate::core::machine_hash(MACHINE_SRC),
        budgets: Budgets::default(),
    })
}

pub fn entered(state: &str, cycle: u32, attempt: u32) -> Event {
    ev(EventPayload::StateEntered(StateEntered {
        state: state.into(),
        cycle,
        attempt,
        session_id: None,
        model: "claude-sonnet-5".into(),
        thinking: "medium".into(),
        skills: vec![],
        mcp: vec![],
    }))
}

pub fn output(state: &str, cycle: u32) -> Event {
    ev(EventPayload::WorkerOutput {
        state: state.into(),
        cycle,
        summary: "did the work".into(),
        artifacts: vec![],
        usage: Usage::default(),
    })
}

pub fn proposed(from: &str, to: &str) -> Event {
    ev(EventPayload::TransitionProposed {
        from: from.into(),
        to: Some(to.into()),
        blocked: false,
        rationale: "looks good".into(),
    })
}

pub fn blocked(from: &str, rationale: &str) -> Event {
    ev(EventPayload::TransitionProposed {
        from: from.into(),
        to: None,
        blocked: true,
        rationale: rationale.into(),
    })
}

pub fn guard_checked(from: &str, to: &str) -> Event {
    ev(EventPayload::GuardChecked {
        from: from.into(),
        to: to.into(),
        check: GuardOutcome::Skip,
        criteria: GuardOutcome::Skip,
        check_output: None,
        judge_rationale: None,
        usage: Usage::default(),
    })
}

pub fn committed(from: &str, to: &str, cycle: u32) -> Event {
    ev(EventPayload::TransitionCommitted {
        from: from.into(),
        to: to.into(),
        cycle,
    })
}

pub fn navigator(from: &str, chosen_to: &str) -> Event {
    ev(EventPayload::NavigatorInvoked {
        from: from.into(),
        proposal: "blocked".into(),
        chosen_to: chosen_to.into(),
        entry_prompt: None,
        usage: Usage::default(),
    })
}

pub fn error(state: &str, detail: &str) -> Event {
    ev(EventPayload::Error {
        state: Some(state.into()),
        kind: ErrorKind::Transient,
        detail: detail.into(),
    })
}

pub fn note(text: &str) -> Event {
    ev(EventPayload::Note { text: text.into() })
}

pub fn finished(status: RunStatus, terminal: &str) -> Event {
    ev(EventPayload::RunFinished {
        status,
        terminal_state: Some(terminal.into()),
        totals: Totals::default(),
    })
}

/// The fields tests vary, as setters rather than as extra parameters on every
/// builder.
///
/// Declared once each. This was a thirteen-method trait beside a
/// thirteen-method impl — two lists nothing related, each entry a copy of one
/// `match &mut self.payload { .. => .., other => panic!(..) }` shape. The panic
/// arm is the load-bearing part: silently ignoring a setter that does not apply
/// would let a test assert on something it never set, and written out per
/// setter it was eleven chances to forget. Here it is generated, from the
/// setter's own name.
///
/// The `envelope` section is for the two fields every event has, which have no
/// variant to match and so no arm to reject. They name their receiver because
/// `self` does not survive a macro boundary hygienically.
///
/// Same argument as `fennel::wire::model_keys!`, which exists because three
/// hand-written copies of a field triple meant a fourth knob was four edits.
macro_rules! event_setters {
    (
        envelope {
            $(
                $(#[$emeta:meta])*
                fn $ename:ident($ev:ident $(, $earg:ident: $ety:ty)* $(,)?) $ebody:block
            )*
        }
        payload {
            $(
                $(#[$pmeta:meta])*
                fn $pname:ident($($parg:ident: $pty:ty),* $(,)?) { $($pat:pat => $arm:expr,)+ }
            )*
        }
    ) => {
        pub trait EventExt {
            $($(#[$emeta])* fn $ename(self $(, $earg: $ety)*) -> Event;)*
            $($(#[$pmeta])* fn $pname(self $(, $parg: $pty)*) -> Event;)*
        }

        impl EventExt for Event {
            $(
                fn $ename(mut self $(, $earg: $ety)*) -> Event {
                    {
                        let $ev = &mut self;
                        $ebody
                    }
                    self
                }
            )*
            $(
                fn $pname(mut self $(, $parg: $pty)*) -> Event {
                    match &mut self.payload {
                        $($pat => $arm,)+
                        other => panic!(
                            concat!("`", stringify!($pname), "` does not apply to {}"),
                            other.kind()
                        ),
                    }
                    self
                }
            )*
        }
    };
}

event_setters! {
    envelope {
        /// The line's timestamp.
        fn at(ev, ts: &str) {
            ev.ts = ts.into();
        }

        /// Run seconds accumulated when this line was written.
        fn elapsed(ev, secs: u64) {
            ev.elapsed_s = secs;
        }
    }

    payload {
        /// The session id pi filed this attempt under.
        ///
        /// Takes an `Option` because "no id recorded" and "an id" are both
        /// things a test needs to plant, and three modules had grown the same
        /// five-line `match session { Some(id) => e.session(id), None => e }`
        /// wrapper to say so — `report`, `sessions`, and `e2e`.
        fn session(id: Option<&str>) {
            EventPayload::StateEntered(h) => h.session_id = id.map(str::to_string),
        }

        fn summary(text: &str) {
            EventPayload::WorkerOutput { summary, .. } => *summary = text.into(),
        }

        fn usage(tokens: u64, cost_usd: f64) {
            EventPayload::WorkerOutput { usage, .. }
            | EventPayload::GuardChecked { usage, .. }
            | EventPayload::NavigatorInvoked { usage, .. } => {
                *usage = Usage { tokens, cost_usd }
            },
        }

        fn artifact(name: &str, path: &str) {
            EventPayload::WorkerOutput { artifacts, .. } => artifacts.push(Artifact {
                name: name.into(),
                path: path.into(),
            }),
        }

        fn rationale(text: &str) {
            EventPayload::TransitionProposed { rationale, .. } => *rationale = text.into(),
        }

        fn guards(check: GuardOutcome, criteria: GuardOutcome) {
            EventPayload::GuardChecked { check: c, criteria: cr, .. } => {
                *c = check;
                *cr = criteria;
            },
        }

        /// What the two tiers left behind: the check's output and the Judge's
        /// rationale. `Option` on both, because "the tier did not run" and "it
        /// ran and said nothing" are different rows in a recap.
        fn evidence(check_output: Option<&str>, judge_rationale: Option<&str>) {
            EventPayload::GuardChecked { check_output: co, judge_rationale: jr, .. } => {
                *co = check_output.map(str::to_string);
                *jr = judge_rationale.map(str::to_string);
            },
        }

        /// The Navigator's input and its note: the rejected proposal it was
        /// handed, and the addendum it wrote for the state it routed to.
        fn routing(proposal: &str, entry_prompt: Option<&str>) {
            EventPayload::NavigatorInvoked { proposal: p, entry_prompt: ep, .. } => {
                *p = proposal.into();
                *ep = entry_prompt.map(str::to_string);
            },
        }

        /// Drop the terminal state — an aborted run that stopped without
        /// reaching one, which a recap reports differently from finishing at
        /// a terminal.
        fn no_terminal() {
            EventPayload::RunFinished { terminal_state, .. } => *terminal_state = None,
        }

        fn totals(totals: Totals) {
            EventPayload::RunFinished { totals: t, .. } => *t = totals,
        }

        /// Promote an `error` from the default transient to fatal — the kind
        /// `report::last_fatal` looks for.
        fn fatal() {
            EventPayload::Error { kind, .. } => *kind = ErrorKind::Fatal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The property the generated arm exists for. A setter that quietly did
    /// nothing on the wrong payload would let a test assert on a field it
    /// never set — so it panics, and it names both sides.
    #[test]
    #[should_panic(expected = "`summary` does not apply to note")]
    fn a_setter_rejects_a_payload_it_does_not_apply_to() {
        let _ = note("not a worker output").summary("nope");
    }

    /// …and the envelope setters apply to everything, so they never reject.
    #[test]
    fn envelope_setters_apply_to_any_event() {
        for e in [note("n"), started("T-1"), committed("a", "b", 1)] {
            let e = e.at("2020-01-01T00:00:00.000Z").elapsed(7);
            assert_eq!(e.ts, "2020-01-01T00:00:00.000Z");
            assert_eq!(e.elapsed_s, 7);
        }
    }

    /// A multi-variant setter reaches every variant it lists.
    #[test]
    fn usage_applies_to_all_three_billable_events() {
        for e in [output("s", 1), guard_checked("a", "b"), navigator("a", "b")] {
            let e = e.usage(10, 0.5);
            let json = serde_json::to_value(&e.payload).unwrap();
            assert_eq!(json["usage"]["tokens"], 10, "{json}");
        }
    }
}
