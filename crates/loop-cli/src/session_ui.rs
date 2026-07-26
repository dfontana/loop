//! The terminal half of `loop session`: an inline picker over `session_picker`.
//!
//! Deliberately thin. It decodes keys into [`Key`], draws whatever
//! [`Picker`] says is visible, and hands back an ordinal — every decision about
//! *which* attempts exist, how they rank, and what a row means lives in the pure
//! module next door. That is also why there is no test here: there is nothing to
//! test that a PTY wouldn't be testing on ratatui's behalf.
//!
//! Two invariants this file owns:
//!
//! 1. **The terminal is restored on every exit path** — accept, cancel, and
//!    error alike — via a `Drop` guard rather than a happy-path cleanup call. A
//!    picker that leaves raw mode on hands the user a dead shell.
//! 2. **The UI draws on stderr.** stdout carries exactly one line (the selection
//!    line), so nothing a human sees here can end up in a pipe.

use anyhow::{Context as _, Result};
use crossterm::event::{self, Event as TermEvent, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};
use ratatui::{Terminal, TerminalOptions, Viewport};

use crate::session_picker::{Candidate, CandidateOrdinal, Key, Picker, Step};

/// Restores the terminal when it goes out of scope, however it goes out.
struct RawGuard;

impl Drop for RawGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}

/// Whether an interactive picker is possible: both stdin and stdout must be
/// terminals.
///
/// stdout is checked as well as stdin because a piped stdout means someone is
/// consuming this command's output, and prompting a human to choose in that
/// situation would either hang or — worse — quietly pick something.
pub fn is_interactive() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

/// Run the picker to a decision. `Ok(None)` is a deliberate cancellation, not a
/// failure.
pub fn pick(picker: &mut Picker<'_>, ticket: Option<&str>) -> Result<Option<CandidateOrdinal>> {
    enable_raw_mode().context("entering raw mode for the session picker")?;
    let _guard = RawGuard;

    let rows = crossterm::terminal::size().map(|(_, h)| h).unwrap_or(24);
    let height = 16.min(rows.saturating_sub(1)).max(6);
    let mut terminal = Terminal::with_options(
        CrosstermBackend::new(std::io::stderr()),
        TerminalOptions {
            viewport: Viewport::Inline(height),
        },
    )
    .context("starting the inline session picker")?;

    let outcome = event_loop(&mut terminal, picker, ticket);

    // Give the viewport's lines back before pi is handed the terminal. Done
    // unconditionally, so an error inside the loop still leaves a usable screen.
    let _ = terminal.clear();
    let _ = terminal.show_cursor();
    outcome
}

fn event_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    picker: &mut Picker<'_>,
    ticket: Option<&str>,
) -> Result<Option<CandidateOrdinal>> {
    loop {
        terminal
            .draw(|frame| draw(frame, picker, ticket))
            .context("drawing the session picker")?;

        let ev = event::read().context("reading a key from the session picker")?;
        let TermEvent::Key(key) = ev else { continue };
        // Windows delivers press *and* release; acting on both double-types.
        if key.kind != KeyEventKind::Press {
            continue;
        }
        let Some(decoded) = decode(key.code, key.modifiers) else {
            continue;
        };
        match picker.on_key(decoded) {
            Step::Continue => {}
            Step::Accept(ordinal) => return Ok(Some(ordinal)),
            Step::Cancel => return Ok(None),
        }
    }
}

/// Terminal key → picker key. `None` for anything the picker has no opinion on.
fn decode(code: KeyCode, mods: KeyModifiers) -> Option<Key> {
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    match (code, ctrl) {
        (KeyCode::Esc, _) => Some(Key::Cancel),
        (KeyCode::Char('c'), true) | (KeyCode::Char('d'), true) => Some(Key::Cancel),
        (KeyCode::Char('o'), true) => Some(Key::CycleScope),
        (KeyCode::Enter, _) => Some(Key::Enter),
        (KeyCode::Backspace, _) => Some(Key::Backspace),
        (KeyCode::Up, _) | (KeyCode::Char('p'), true) => Some(Key::Up),
        (KeyCode::Down, _) | (KeyCode::Char('n'), true) => Some(Key::Down),
        // Ctrl-anything-else is a binding we don't have, not text to type.
        (KeyCode::Char(c), false) if !mods.contains(KeyModifiers::ALT) => Some(Key::Char(c)),
        _ => None,
    }
}

fn draw(frame: &mut ratatui::Frame, picker: &Picker<'_>, ticket: Option<&str>) {
    let [header, query, list, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    let dim = Style::default().add_modifier(Modifier::DIM);
    let bold = Style::default().add_modifier(Modifier::BOLD);

    let mut head = Vec::new();
    if let Some(t) = ticket {
        head.push(Span::styled(format!("{t}  "), bold));
    }
    head.push(Span::styled(picker.scope().label().to_string(), bold));
    head.push(Span::styled(
        format!("  {} attempt(s)", picker.visible().len()),
        dim,
    ));
    frame.render_widget(Paragraph::new(Line::from(head)), header);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("> ", dim),
            Span::raw(picker.query().to_string()),
        ])),
        query,
    );

    if picker.visible().is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  no attempt matches — Backspace to widen, Ctrl+O for another mode",
                dim,
            ))),
            list,
        );
    } else {
        let items: Vec<ListItem> = picker.visible().iter().map(|c| row(picker, c)).collect();
        let mut state = ListState::default().with_selected(Some(picker.cursor()));
        frame.render_stateful_widget(
            List::new(items).highlight_symbol("▌").highlight_style(bold),
            list,
            &mut state,
        );
    }

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "↑/↓ move · type to filter · Ctrl+O mode · Enter open · Esc cancel",
            dim,
        ))),
        footer,
    );
}

/// Two lines per attempt: the headline, then the Worker's own words followed by
/// what ran the attempt. Evidence the ledger has but the headline can't hold — a
/// crash's error detail, the model, the cost, the artifact count — rides on the
/// second line so a row still reads in one glance.
fn row<'a>(picker: &Picker<'_>, c: &'a Candidate) -> ListItem<'a> {
    let dim = Style::default().add_modifier(Modifier::DIM);
    let lines = vec![
        Line::from(c.headline(picker.describe(&c.state).as_deref())),
        Line::from(format!("  {} · {}", evidence(c), c.meta())).style(dim),
    ];
    ListItem::new(lines)
}

/// The most informative thing the ledger knows about this attempt, in one line.
fn evidence(c: &Candidate) -> String {
    if let Some(detail) = c.detail() {
        detail
    } else if let Some(err) = c.errors.first() {
        format!("error: {err}")
    } else {
        "no worker_output — still running, or killed without a trace".to_string()
    }
}
