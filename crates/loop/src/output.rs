//! Shared human-readable CLI output.

use crate::core::text::brief;
use crate::core::{Event, FoldStatus, RunState, Totals};

/// Where a run got to, in one sentence.
///
/// Beside [`fmt_totals`] and for the same reason: `loop status` and `loop
/// recap` both match all three [`FoldStatus`] variants, and they had three
/// different wordings between them — the same ledger described itself as
/// "running — at `x`" or "unfinished — last at `x`" depending on which command
/// you happened to type.
pub(crate) fn fmt_status(rs: &RunState) -> String {
    let at = || rs.current.as_deref().unwrap_or("?");
    match rs.fold_status() {
        FoldStatus::NotStarted => "not started".into(),
        FoldStatus::Running => format!("unfinished — last at `{}`", at()),
        FoldStatus::Finished(s) => format!("finished — {s:?} at `{}`", at()),
    }
}

/// `10 transition(s), $3.58, 41200 token(s), 56m54s` — what a run has spent.
///
/// One spelling, here rather than in `report`, because the four callers span
/// three layers: the digest fed to every stage, `loop status`, `loop run`'s
/// closing line, and `loop recap`. They had four wordings between them, in
/// three different field orders, over the same four numbers — so the same
/// ledger could be quoted back at you three ways in one session.
pub(crate) fn fmt_totals(t: &Totals) -> String {
    format!(
        "{} transition(s), ${:.2}, {} token(s), {}",
        t.transitions,
        t.usage.cost_usd,
        t.usage.tokens,
        fmt_duration(t.wallclock_s)
    )
}

/// Seconds as `45s`, `1m30s`, `2h05m`. Beside [`fmt_totals`] because that is
/// its main caller, and reachable from `report`'s budget line, which is the
/// other place a bound in seconds is shown to a human.
pub(crate) fn fmt_duration(secs: u64) -> String {
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m{}s", s / 60, s % 60),
        s => format!("{}h{}m", s / 3600, (s % 3600) / 60),
    }
}

/// Render one ledger event using the summary grammar shared by `status` and
/// `logs`.
pub(crate) fn summarize(e: &Event) -> String {
    use crate::core::EventPayload::*;
    match &e.payload {
        RunStarted { ticket, .. } => format!("run_started {ticket}"),
        StateEntered(h) => format!("→ {} (cycle {}, attempt {})", h.state, h.cycle, h.attempt),
        WorkerOutput { state, usage, .. } => {
            format!("{state} done (${:.2})", usage.cost_usd)
        }
        TransitionProposed {
            from,
            to,
            blocked,
            rationale,
            ..
        } => {
            if *blocked {
                format!("{from} blocked: {}", brief(rationale, 60))
            } else {
                format!(
                    "{from} proposes → {}: {}",
                    to.as_deref().unwrap_or("?"),
                    brief(rationale, 60)
                )
            }
        }
        GuardChecked {
            from,
            to,
            check,
            criteria,
            ..
        } => format!("guard {from}→{to}: check={check:?} criteria={criteria:?}"),
        NavigatorInvoked {
            from, chosen_to, ..
        } => format!("navigator {from} → {chosen_to}"),
        TransitionCommitted { from, to, .. } => format!("committed {from} → {to}"),
        Error { kind, detail, .. } => format!("error ({kind:?}): {}", brief(detail, 60)),
        Note { text } => format!("note: {}", brief(text, 70)),
        RunFinished { status, .. } => format!("run_finished {status:?}"),
    }
}
