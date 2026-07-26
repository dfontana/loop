//! Shared human-readable CLI output.

use loop_core::Event;

/// Render one ledger event using the summary grammar shared by `status` and
/// `logs`.
pub(crate) fn summarize(e: &Event) -> String {
    use loop_core::EventPayload::*;
    match &e.payload {
        RunStarted { ticket, .. } => format!("run_started {ticket}"),
        StateEntered {
            state,
            cycle,
            attempt,
            ..
        } => format!("→ {state} (cycle {cycle}, attempt {attempt})"),
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
                format!("{from} blocked: {}", truncate(rationale, 60))
            } else {
                format!(
                    "{from} proposes → {}: {}",
                    to.as_deref().unwrap_or("?"),
                    truncate(rationale, 60)
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
        Error { kind, detail, .. } => format!("error ({kind:?}): {}", truncate(detail, 60)),
        Note { text } => format!("note: {}", truncate(text, 70)),
        RunFinished { status, .. } => format!("run_finished {status:?}"),
    }
}

pub fn truncate(s: &str, n: usize) -> String {
    let one_line = s.replace('\n', " ");
    if one_line.chars().count() <= n {
        one_line
    } else {
        let head: String = one_line.chars().take(n - 1).collect();
        format!("{head}…")
    }
}
