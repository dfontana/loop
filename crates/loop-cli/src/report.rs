//! Text reports for the read-only inspection commands.
//!
//! `loop preview` and `loop recap` render here rather than in `commands.rs` so
//! the layout — one value column, sections in a fixed order, every list in the
//! machine's own deterministic order — lives in one place. Nothing in this
//! module opens a file for writing or creates a directory: a report is a pure
//! function of the machine, the config, the ledger, and what the toolbox
//! already has on disk.
//!
//! The two reports answer the same questions on either side of a run — preview
//! explains the declaration, recap explains the execution — so they share this
//! module's vocabulary deliberately: the same labels, in the same order, for
//! ticket, budgets, state, model, skills, MCP, transitions, guards and loops.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use loop_core::{
    Actor, ArtifactRef, Budgets, Check, Context, Event, EventPayload, GuardOutcome, LoopSpec,
    Machine, ModelSpec, OnExhausted, OnFail, PlaybookRef, ResumePoint, RunState, StateId, Totals,
    Transition, Usage,
};
use loop_engine::{Diagnostic, Severity};
use loop_toolbox::render;

use crate::episode::Episode;
use crate::output::{summarize, truncate};
use crate::stage::{Resolved, Resolver};

/// Column every value starts at, whatever depth its label sits at. Labels are
/// padded to reach it, so a nested field's value still lines up with an
/// outer one.
const VALUE_COL: usize = 20;

/// The stand-in for an empty list or an absent optional value. One spelling
/// throughout, so "nothing here" never reads as a missing line.
const NONE: &str = "(none)";

#[derive(Default)]
pub struct Report {
    out: String,
}

impl Report {
    pub fn new() -> Self {
        Self::default()
    }

    /// The report's first line, followed by a blank one.
    pub fn title(&mut self, text: impl AsRef<str>) {
        let _ = writeln!(self.out, "{}\n", text.as_ref());
    }

    /// A section heading at column zero, preceded by a blank line.
    pub fn section(&mut self, name: &str) {
        let _ = writeln!(self.out, "\n{name}");
    }

    /// A `label   value` pair. A multi-line value keeps its continuation lines
    /// aligned under the first, so a prose `:criteria` stays inside the column.
    pub fn field_at(&mut self, indent: usize, label: &str, value: impl AsRef<str>) {
        let pad = VALUE_COL.saturating_sub(indent + label.len());
        let value = value.as_ref();
        let value = if value.is_empty() { NONE } else { value };
        for (i, line) in value.lines().enumerate() {
            if i == 0 {
                let _ = writeln!(
                    self.out,
                    "{:indent$}{label}{:pad$}{line}",
                    "",
                    "",
                    indent = indent,
                    pad = pad
                );
            } else {
                let _ = writeln!(self.out, "{:VALUE_COL$}{line}", "");
            }
        }
    }

    pub fn field(&mut self, label: &str, value: impl AsRef<str>) {
        self.field_at(2, label, value);
    }

    /// A line of its own — a state heading, an edge, a bullet.
    pub fn line_at(&mut self, indent: usize, text: impl AsRef<str>) {
        let _ = writeln!(self.out, "{:indent$}{}", "", text.as_ref(), indent = indent);
    }

    pub fn blank(&mut self) {
        self.out.push('\n');
    }

    /// Verbatim text, indented as a block and never reflowed. Used for prompt
    /// bodies, where every byte matters.
    pub fn block(&mut self, indent: usize, text: &str) {
        for line in text.lines() {
            if line.is_empty() {
                self.out.push('\n');
            } else {
                let _ = writeln!(self.out, "{:indent$}{line}", "", indent = indent);
            }
        }
    }

    pub fn finish(self) -> String {
        self.out
    }

    // ── the Markdown half, used by `recap` ───────────────────────────────────
    //
    // `loop recap > run-recap.md` is in the command's contract, so recap's
    // structure is real Markdown — headings, bullets, fences — rather than the
    // aligned columns above, which collapse into one paragraph the moment a
    // renderer touches them. The labels and their order stay shared with
    // preview; only the punctuation around them differs.

    /// A Markdown heading, separated from whatever precedes it by exactly one
    /// blank line.
    pub fn heading(&mut self, level: usize, text: impl AsRef<str>) {
        self.ensure_blank();
        let _ = writeln!(self.out, "{} {}\n", "#".repeat(level), text.as_ref());
    }

    /// `- label: value`. A multi-line value keeps its continuation lines inside
    /// the bullet, so a Judge rationale stays attached to its label.
    pub fn bullet(&mut self, label: impl AsRef<str>, value: impl AsRef<str>) {
        let label = label.as_ref();
        let value = value.as_ref();
        let value = if value.trim().is_empty() { NONE } else { value };
        for (i, line) in value.lines().enumerate() {
            if i == 0 {
                let _ = writeln!(self.out, "- {label}: {line}");
            } else {
                let _ = writeln!(self.out, "  {line}");
            }
        }
    }

    /// A paragraph, blank-line separated on both sides.
    pub fn para(&mut self, text: impl AsRef<str>) {
        self.ensure_blank();
        let _ = writeln!(self.out, "{}\n", text.as_ref());
    }

    /// A fenced block for text the report must reproduce exactly — check
    /// output, a Judge's rationale, an error detail.
    ///
    /// The fence is grown past the longest backtick run inside the text. Check
    /// output is arbitrary bytes from someone else's build tool; a fixed
    /// three-backtick fence would let it terminate its own block and let the
    /// rest of a run's output masquerade as the report's own prose.
    pub fn fence(&mut self, text: &str) {
        self.ensure_blank();
        let longest = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
        let fence = "`".repeat(longest.max(2) + 1);
        let _ = writeln!(self.out, "{fence}");
        for line in text.lines() {
            let _ = writeln!(self.out, "{line}");
        }
        let _ = writeln!(self.out, "{fence}\n");
    }

    /// End the current block, without stacking blank lines.
    fn ensure_blank(&mut self) {
        if self.out.is_empty() || self.out.ends_with("\n\n") {
            return;
        }
        if !self.out.ends_with('\n') {
            self.out.push('\n');
        }
        self.out.push('\n');
    }
}

// ── the whole-machine preview ────────────────────────────────────────────────

/// The concise report: what the machine is, what every state resolves to, and
/// how the edges between them are gated.
pub fn machine_preview(r: &Resolver<'_>) -> String {
    let m = r.machine;
    let mut rep = Report::new();
    rep.title(format!(
        "{} — {} state(s), {} transition(s), {} loop(s)",
        m.ticket,
        m.states.len(),
        m.transitions.len(),
        m.loops.len()
    ));

    rep.field("source", m.source_path.display().to_string());
    rep.field("entry", &m.entry);
    rep.field("terminals", join(m.terminals.iter().cloned()));
    rep.field("escalation", m.escalation_state.clone().unwrap_or_default());
    rep.field("budgets", fmt_budgets(&m.budgets));
    rep.field("judge", fmt_model(&m.judge));
    rep.field(
        "navigator",
        format!(
            "{} (max {} invocation(s))",
            fmt_model(&m.navigator),
            m.navigator_max_invocations
        ),
    );

    rep.section("context");
    rep.field("task", prose_summary(&m.task));
    rep.field("plan", prose_summary(&m.plan));
    if m.qa_cases.is_empty() {
        rep.field("qa cases", NONE);
    } else {
        rep.field("qa cases", format!("{}", m.qa_cases.len()));
        for c in &m.qa_cases {
            rep.field_at(6, &c.id, first_line(&c.desc));
        }
    }

    rep.section("states");
    for id in m.states.keys() {
        rep.blank();
        state_block(&mut rep, r, id);
    }

    rep.section("loops");
    if m.loops.is_empty() {
        rep.field("declared", NONE);
    } else {
        for l in &m.loops {
            rep.line_at(2, format!("{} — {}", l.name, fmt_loop_head(l)));
            rep.field_at(4, "states", join(l.states.iter().cloned()));
            rep.field_at(4, "max cycles", l.max_cycles.to_string());
            rep.field_at(4, "on exhausted", fmt_on_exhausted(l, r));
        }
    }

    rep.finish()
}

/// One state's block: what it resolves to, and every edge out of it. Shared by
/// both preview forms so a state reads the same either way.
fn state_block(rep: &mut Report, r: &Resolver<'_>, id: &StateId) {
    let m = r.machine;
    let description = m
        .state(id)
        .and_then(|s| s.description.clone())
        .unwrap_or_else(|| "(no description)".into());
    rep.line_at(2, format!("{id} — {}", first_line(&description)));

    match r.resolve(id) {
        Ok(res) => {
            rep.field_at(4, "playbook", fmt_playbook_source(&res));
            rep.field_at(4, "model", fmt_model(&res.model));
            rep.field_at(4, "skills", fmt_skills(&res));
            rep.field_at(4, "mcp", join(res.mcp.iter().cloned()));
        }
        // A state whose playbook or skill does not resolve still gets a block:
        // the diagnostics at the end of the report say the same thing with a
        // severity attached, and preview exits non-zero for it either way.
        Err(e) => rep.field_at(4, "unresolved", e.to_string()),
    }
    rep.field_at(4, "reachable", join(m.neighbors(id)));

    for t in m.edges_from(id) {
        rep.line_at(4, format!("→ {}", t.to));
        edge_fields(rep, t);
    }
}

fn edge_fields(rep: &mut Report, t: &Transition) {
    match &t.check {
        Some(Check { cmd, timeout_s }) => {
            rep.field_at(6, "check", cmd);
            rep.field_at(6, "timeout", format!("{timeout_s}s"));
        }
        None => rep.field_at(6, "check", NONE),
    }
    rep.field_at(6, "criteria", t.criteria.clone().unwrap_or_default());
    rep.field_at(6, "on fail", fmt_on_fail(&t.on_fail));
    rep.field_at(
        6,
        "backoff",
        t.backoff_s.map(|s| format!("{s}s")).unwrap_or_default(),
    );
}

// ── the single-state preview ─────────────────────────────────────────────────

/// The detailed report for one state: everything the whole-machine preview
/// shows for it, plus the resolved playbook, the exact Worker invocation, and
/// a representative render of the prompt.
pub fn state_preview(r: &Resolver<'_>, id: &StateId) -> String {
    let m = r.machine;
    let mut rep = Report::new();
    rep.title(format!("{} — state `{id}`", m.ticket));
    state_block(&mut rep, r, id);

    let resolved = match r.resolve(id) {
        Ok(res) => res,
        // Nothing below this point exists without a playbook. The report stops
        // here rather than guessing; the diagnostics still print, and preview
        // still exits non-zero.
        Err(_) => return rep.finish(),
    };

    rep.section("playbook");
    rep.field("reference", fmt_playbook_ref(&resolved.state.playbook));
    rep.field(
        "resolved path",
        resolved
            .playbook
            .path
            .as_deref()
            .map(display)
            .unwrap_or_else(|| "(inline — no file)".into()),
    );
    rep.section("playbook frontmatter");
    rep.field("name", &resolved.playbook.name);
    rep.field(
        "description",
        resolved.playbook.description.clone().unwrap_or_default(),
    );
    rep.field("model", resolved.playbook.model.clone().unwrap_or_default());
    rep.field(
        "thinking",
        resolved
            .playbook
            .thinking
            .map(|t| t.to_string())
            .unwrap_or_default(),
    );

    let config = r.config;
    rep.section("worker invocation");
    rep.field(
        "model flag",
        format!("--model {}", resolved.model.pi_model_arg()),
    );
    rep.field("provider", &resolved.model.provider);
    if resolved.skill_paths.is_empty() {
        rep.field("skills", NONE);
    } else {
        for (name, path) in resolved.skills.iter().zip(&resolved.skill_paths) {
            rep.field_at(2, name, display(path));
        }
    }
    rep.field("mcp", join(resolved.mcp.iter().cloned()));
    rep.field("reachable", join(m.neighbors(id)));
    rep.field("cwd", display(&config.paths.project_dir));
    rep.field(
        "system prompt",
        format!(
            "{}",
            config
                .paths
                .render_dir(&m.ticket)
                .join(format!("{id}-<cycle>-<attempt>-system.md"))
                .display()
        ),
    );
    rep.field("env", "TICKET_ID, STATE, CYCLE, ATTEMPT");
    rep.field("session id", r.session_id(id, 1, 1));

    // The variables the body actually writes. A variable only reaches the
    // agent where the playbook interpolates it (docs/03-customizing.md), so
    // this list is the stage's real context, not the namespace's size.
    let context = Context::representative(m, id);
    let vars = context.to_map();
    let referenced = render::referenced_vars(&resolved.playbook.body);
    let (known, passthrough): (Vec<_>, Vec<_>) = referenced
        .into_iter()
        .partition(|name| vars.contains_key(name));

    rep.section("template variables");
    rep.field("referenced", join(known.iter().map(|n| format!("${n}"))));
    rep.field(
        "passed through",
        join(passthrough.iter().map(|n| format!("${n}"))),
    );

    rep.section(&format!(
        "playbook body — as authored, {} line(s), unrendered",
        resolved.playbook.body.lines().count()
    ));
    rep.blank();
    rep.block(2, &resolved.playbook.body);

    rep.section("representative render");
    rep.line_at(
        2,
        "Cycle 1, attempt 1, no previous state, no artifacts, empty ledger digest.",
    );
    rep.line_at(
        2,
        "NOT the prompt a future run will send: $PREV_STATE, $LEDGER_DIGEST, the",
    );
    rep.line_at(
        2,
        "artifact variables, $CYCLE, $ATTEMPT, $CRASHED and $ENTRY_ADDENDUM all",
    );
    rep.line_at(
        2,
        "depend on where the run has already been. Read it for shape, not text.",
    );

    rep.blank();
    rep.line_at(2, "--- system prompt ---");
    rep.blank();
    rep.block(2, &render::substitute(&resolved.playbook.body, &vars));
    rep.blank();
    rep.line_at(2, "--- entry message ---");
    rep.blank();
    rep.block(2, &render::entry_message(&context, &resolved.mcp));

    rep.finish()
}

// ── the run recap ────────────────────────────────────────────────────────────

/// The `run_started` line, destructured once so no consumer has to re-match a
/// variant [`timeline`] has already established.
struct Started<'e> {
    ts: &'e str,
    ticket: &'e str,
    machine_hash: &'e str,
    budgets: &'e Budgets,
}

/// The `run_finished` line, likewise.
struct Finished<'e> {
    ts: &'e str,
    status: loop_core::RunStatus,
    terminal_state: Option<&'e str>,
    totals: &'e Totals,
}

/// The ledger, grouped into attempts without any of it being dropped.
///
/// Attempt grouping itself belongs to [`crate::episode`]; what this adds is
/// lifting the two events that bracket the whole run out of whichever attempt
/// happened to be open around them, and keeping the events that landed before
/// any attempt started.
#[derive(Default)]
struct Timeline<'e> {
    started: Option<Started<'e>>,
    finished: Option<Finished<'e>>,
    /// Events before the first `state_entered` — an error during startup, a
    /// note. Rare, and precisely what a report that only walked attempts would
    /// silently lose.
    prelude: Vec<&'e Event>,
    /// One entry per episode, in ledger order, holding only the events that are
    /// that attempt's own.
    attempts: Vec<(Episode<'e>, Vec<&'e Event>)>,
}

/// Group a ledger into a [`Timeline`]. Pure, total, and order-preserving: every
/// event lands in exactly one of `started`, `finished`, `prelude`, or one
/// attempt's body.
///
/// `sort` is deliberately absent: the ledger's own order *is* the ordering, and
/// a report that re-sorted by timestamp would reorder the sub-second burst a
/// stage exit writes.
fn timeline<'e>(events: &'e [Event]) -> Timeline<'e> {
    let mut t = Timeline::default();

    // Returns whether the event brackets the run, and so is not any one
    // attempt's to report.
    let bracket = |t: &mut Timeline<'e>, e: &'e Event| -> bool {
        match &e.payload {
            EventPayload::RunStarted {
                ticket,
                machine_hash,
                budgets,
            } => {
                // First one wins: a hand-concatenated ledger with two starts
                // still describes the run its first line opened.
                t.started.get_or_insert(Started {
                    ts: &e.ts,
                    ticket,
                    machine_hash,
                    budgets,
                });
                true
            }
            EventPayload::RunFinished {
                status,
                terminal_state,
                totals,
            } => {
                t.finished = Some(Finished {
                    ts: &e.ts,
                    status: *status,
                    terminal_state: terminal_state.as_deref(),
                    totals,
                });
                true
            }
            _ => false,
        }
    };

    let episodes = crate::episode::episodes(events);
    let first = episodes.first().map_or(events.len(), |ep| ep.ordinal);
    for e in &events[..first] {
        if !bracket(&mut t, e) {
            t.prelude.push(e);
        }
    }

    for ep in episodes {
        let body: Vec<&Event> = ep.body.iter().filter(|e| !bracket(&mut t, e)).collect();
        t.attempts.push((ep, body));
    }
    t
}

/// Whether the `machine.fnl` on disk may be used to explain this ledger.
///
/// Three states rather than a bool, because the report says something different
/// about each and the difference matters to a reader: a machine that was edited
/// after the run is a warning, a machine that would not load is a shrug.
pub enum Provenance<'m> {
    /// The machine on disk hashes to what `run_started` recorded. Only this
    /// variant may explain the run.
    Matches(&'m Machine),
    /// A machine loaded, but it is not the one that ran.
    Changed { current: String },
    /// No machine loaded, or the ledger has no `run_started` to compare against.
    NotLoaded,
}

impl Provenance<'_> {
    /// The description the machine gives a state, or `""` — empty on every
    /// variant but [`Provenance::Matches`], so a description written after the
    /// run cannot label a historical attempt. The invariant is the type's, not
    /// the caller's.
    fn describe(&self, state: &str) -> &str {
        match self {
            Self::Matches(m) => m
                .state(state)
                .and_then(|s| s.description.as_deref())
                .map_or("", first_line),
            _ => "",
        }
    }

    fn trusted(&self) -> bool {
        matches!(self, Self::Matches(_))
    }
}

/// Everything `recap` renders from. Assembled by `commands::recap`, which owns
/// reading the ledger and establishing the machine's [`Provenance`].
pub struct Recap<'a> {
    pub events: &'a [Event],
    pub folded: &'a RunState,
    pub machine: Provenance<'a>,
}

/// `loop recap` — what this run did, answered from the ledger alone.
///
/// Deterministic by construction: no LLM, no filesystem beyond the ledger, and
/// no dependence on the machine currently on disk. The same ledger renders the
/// same report, which is the only property that makes a recap usable as
/// evidence rather than as a second, softer history.
pub fn recap(r: &Recap<'_>) -> String {
    let t = timeline(r.events);
    let mut rep = Report::new();

    let ticket = t
        .started
        .as_ref()
        .map_or("(no run_started — ticket unknown)", |s| s.ticket);
    rep.heading(1, format!("{ticket} — recap"));
    rep.para(
        "Reconstructed from `.loop/ledger.jsonl`. Every figure below is what the ledger \
         recorded, not what the machine on disk currently declares.",
    );

    run_summary(&mut rep, r, &t);
    attempt_timeline(&mut rep, r, &t);
    why_it_ended(&mut rep, r, &t);
    inspection_pointers(&mut rep, &t);

    // Exactly one trailing newline, so appending the report to a file or
    // diffing two of them does not turn on how a fence happened to end.
    let mut out = rep.finish();
    out.truncate(out.trim_end().len());
    out.push('\n');
    out
}

fn run_summary(rep: &mut Report, r: &Recap<'_>, t: &Timeline<'_>) {
    rep.heading(2, "Run summary");

    match &t.started {
        Some(s) => {
            rep.bullet("started", s.ts);
            rep.bullet("budgets", fmt_budgets(s.budgets));
            rep.bullet("machine hash", s.machine_hash);
            rep.bullet(
                "machine on disk",
                match &r.machine {
                    Provenance::Matches(_) => "unchanged since the run started".to_string(),
                    Provenance::Changed { current } => format!(
                        "CHANGED — now {current}. Current declarations are not used to \
                         explain anything below."
                    ),
                    Provenance::NotLoaded => {
                        "not loaded — the recap reports the ledger only".to_string()
                    }
                },
            );
        }
        // A ledger with no `run_started` is a repaired or hand-assembled one.
        // It still gets a recap; it just cannot be told what it was started
        // with.
        None => rep.bullet(
            "started",
            "no `run_started` in this ledger — budgets and machine identity unknown",
        ),
    }

    rep.bullet("outcome", fmt_outcome(r.folded));
    rep.bullet("totals", fmt_totals(&r.folded.totals));
    // Without a trustworthy machine there is nothing to say which states are
    // loop heads, so the fold counts every re-entry. Labelling that is the
    // difference between a cycle count and a state-visit count.
    let cycles = fmt_cycles(r.folded);
    rep.bullet(
        "cycles",
        if r.machine.trusted() {
            cycles
        } else {
            format!(
                "{cycles} — re-entries of every state, not declared loops: the machine \
                 that ran is not available"
            )
        },
    );
    rep.bullet(
        "navigator",
        format!("{} invocation(s)", r.folded.navigator_invocations),
    );
    rep.bullet(
        "attempts",
        format!(
            "{} Worker attempt(s) across {} state(s)",
            t.attempts.len(),
            t.attempts
                .iter()
                .map(|(ep, _)| ep.state.as_str())
                .collect::<std::collections::BTreeSet<_>>()
                .len()
        ),
    );

    if !t.prelude.is_empty() {
        rep.para("Before the first attempt:");
        for e in &t.prelude {
            rep.bullet(&e.ts, summarize(e));
        }
    }
}

fn attempt_timeline(rep: &mut Report, r: &Recap<'_>, t: &Timeline<'_>) {
    rep.heading(2, "Attempt timeline");
    rep.para(
        "One section per `state_entered`, in ledger order. Evidence is labelled by who \
         authored it: **Worker** lines are the Worker's own account and prove nothing on \
         their own, **Check** is the command the harness ran itself, **Judge** is the \
         independent verdict on the edge's criteria, and **Committed** is the harness's \
         decision.",
    );

    if t.attempts.is_empty() {
        rep.para("No `state_entered` was ever recorded — no stage got as far as spawning.");
        return;
    }

    for (n, (ep, body)) in t.attempts.iter().enumerate() {
        let mut title = format!(
            "{}. {} — cycle {}, attempt {}",
            n + 1,
            ep.state,
            ep.cycle,
            ep.attempt
        );
        let description = r.machine.describe(ep.state);
        if !description.is_empty() {
            let _ = write!(title, " — {description}");
        }
        rep.heading(3, title);

        rep.bullet(
            "entered",
            format!(
                "{} — {} into the run",
                ep.entered.ts,
                fmt_duration(ep.entered.elapsed_s)
            ),
        );
        rep.bullet("model", format!("{}:{}", ep.model, ep.thinking));
        rep.bullet("skills", join(ep.skills));
        rep.bullet("mcp", join(ep.mcp));
        rep.bullet(
            "session",
            ep.session_id
                .unwrap_or("(none recorded — this attempt cannot be reopened)"),
        );

        if body.is_empty() {
            rep.para(
                "Nothing further was recorded for this attempt — the run was interrupted \
                 here, or is still inside it.",
            );
        }
        for e in body {
            episode_event(rep, e);
        }
    }
}

/// One event inside an attempt, rendered in ledger order.
fn episode_event(rep: &mut Report, e: &Event) {
    match &e.payload {
        EventPayload::WorkerOutput {
            summary,
            artifacts,
            usage,
            ..
        } => {
            rep.para("**Worker** — the Worker's own account of what it did:");
            rep.fence(summary.trim());
            rep.bullet("worker usage", fmt_usage(usage));
            rep.bullet("artifacts", fmt_artifacts(artifacts));
        }
        EventPayload::TransitionProposed {
            from,
            to,
            blocked,
            rationale,
            by,
        } => {
            let target = if *blocked {
                "blocked — no target proposed".to_string()
            } else {
                format!("→ `{}`", to.as_deref().unwrap_or("(none)"))
            };
            rep.para(format!(
                "**Proposal** by {} — `{from}` {target}:",
                fmt_actor(*by)
            ));
            rep.fence(rationale.trim());
        }
        EventPayload::GuardChecked {
            from,
            to,
            structural,
            check,
            criteria,
            check_output,
            judge_rationale,
            usage,
        } => {
            rep.para(format!("**Guards** on `{from}` → `{to}`:"));
            rep.bullet("structural", fmt_guard(*structural));
            rep.bullet("check", fmt_guard(*check));
            rep.bullet("criteria", fmt_guard(*criteria));
            rep.bullet("judge usage", fmt_usage(usage));
            match check_output {
                Some(out) => {
                    rep.para("**Check** output — harness evidence, not the Worker's:");
                    rep.fence(out);
                }
                None => rep.para("**Check** output: none recorded."),
            }
            match judge_rationale {
                Some(text) => {
                    rep.para("**Judge** rationale:");
                    rep.fence(text.trim());
                }
                None => rep.para("**Judge** rationale: none — the criteria tier did not run."),
            }
        }
        EventPayload::NavigatorInvoked {
            from,
            proposal,
            chosen_to,
            entry_prompt,
            usage,
        } => {
            rep.para(format!(
                "**Navigator** was asked to route out of `{from}` and chose `{chosen_to}`:"
            ));
            rep.bullet("rejected proposal", proposal.as_str());
            rep.bullet("navigator usage", fmt_usage(usage));
            if let Some(prompt) = entry_prompt {
                rep.para("Addendum handed to the next stage:");
                rep.fence(prompt.trim());
            }
        }
        EventPayload::TransitionCommitted { from, to, cycle } => {
            rep.para(format!(
                "**Committed** `{from}` → `{to}` (cycle {cycle}) — the harness's decision."
            ));
        }
        EventPayload::Error {
            state,
            kind,
            detail,
        } => {
            rep.para(format!(
                "**Error** ({}){}:",
                format!("{kind:?}").to_lowercase(),
                state
                    .as_deref()
                    .map(|s| format!(" at `{s}`"))
                    .unwrap_or_default(),
            ));
            rep.fence(detail.trim());
        }
        EventPayload::Note { text } => {
            rep.para("**Note**:");
            rep.fence(text.trim());
        }
        // Lifted into `Timeline` before an episode is built; unreachable in
        // practice, and cheaper to render than to prove impossible.
        EventPayload::RunStarted { .. }
        | EventPayload::StateEntered { .. }
        | EventPayload::RunFinished { .. } => rep.bullet(&e.ts, summarize(e)),
    }
}

fn why_it_ended(rep: &mut Report, r: &Recap<'_>, t: &Timeline<'_>) {
    rep.heading(2, "Why it ended");

    let Some(f) = &t.finished else {
        rep.bullet("status", "unfinished — no `run_finished` in this ledger");
        rep.bullet("resume point", fmt_resume(&r.folded.resume));
        match r.events.last() {
            Some(e) => rep.bullet("last durable event", format!("{}  {}", e.ts, summarize(e))),
            None => rep.bullet("last durable event", NONE),
        }
        if let Some(detail) = last_fatal(r.events) {
            rep.para("A fatal error was recorded:");
            rep.fence(detail.trim());
        }
        rep.para("`loop resume` continues from the resume point above.");
        return;
    };

    rep.bullet("status", format!("{:?}", f.status));
    rep.bullet(
        "terminal state",
        f.terminal_state
            .unwrap_or("(none — the run stopped without reaching one)"),
    );
    rep.bullet("recorded at", f.ts);
    rep.bullet("final totals", fmt_totals(f.totals));

    // A non-`Done` run ended on something. The guardrail or the fatal error is
    // the answer to "why", so it is repeated here rather than left for the
    // reader to find in the timeline.
    if f.status != loop_core::RunStatus::Done {
        match last_fatal(r.events) {
            Some(detail) => {
                rep.para("The last fatal error recorded before it stopped:");
                rep.fence(detail.trim());
            }
            None => rep.para(
                "No fatal `error` was recorded: the run reached this terminal through the \
                 machine's own edges — see the last committed transition above.",
            ),
        }
    }
}

fn inspection_pointers(rep: &mut Report, t: &Timeline<'_>) {
    rep.heading(2, "Inspecting further");
    rep.para(
        "This recap is a digest. The Worker's full history is pi's, and the complete event \
         stream is the ledger's.",
    );

    // One line per state that has a reopenable attempt, because that is the
    // granularity `loop session` filters at.
    let mut by_state: BTreeMap<&str, usize> = BTreeMap::new();
    for (ep, _) in t.attempts.iter().filter(|(e, _)| e.session_id.is_some()) {
        *by_state.entry(ep.state.as_str()).or_insert(0) += 1;
    }
    if by_state.is_empty() {
        rep.para("No attempt recorded a pi session id, so there is nothing to reopen.");
    } else {
        rep.para("Reopen a Worker's session:");
        for (state, n) in by_state {
            rep.bullet(
                format!("`loop session {state}`"),
                format!("{n} reopenable attempt(s); add `--latest` to skip the picker"),
            );
        }
    }

    rep.para("Read the complete event stream:");
    rep.fence(
        "loop logs --raw | jq -r 'select(.type==\"transition_committed\")\n\
         \x20      | \"\\(.ts)  cycle \\(.cycle)  \\(.from) -> \\(.to)\"'\n\
         \n\
         loop logs --raw | jq -r 'select(.type==\"guard_checked\" and (.check==\"fail\" or \
         .criteria==\"fail\"))\n\
         \x20      | \"=== \\(.from) -> \\(.to)\", (.check_output // \"\"), \
         (.judge_rationale // \"\")'",
    );
}

/// The detail of the last `error` marked fatal, if any.
fn last_fatal(events: &[Event]) -> Option<&str> {
    events.iter().rev().find_map(|e| match &e.payload {
        EventPayload::Error {
            kind: loop_core::ErrorKind::Fatal,
            detail,
            ..
        } => Some(detail.as_str()),
        _ => None,
    })
}

fn fmt_outcome(rs: &RunState) -> String {
    match rs.fold_status() {
        loop_core::FoldStatus::NotStarted => "not started".into(),
        loop_core::FoldStatus::Running => format!(
            "unfinished — last at `{}`",
            rs.current.as_deref().unwrap_or("?")
        ),
        loop_core::FoldStatus::Finished(s) => format!(
            "finished — {s:?} at `{}`",
            rs.current.as_deref().unwrap_or("?")
        ),
    }
}

/// `10 transition(s), $3.58, 56m54s`. Deliberately not shared with `status`'s
/// one-screen header, which stays terse; a report has room to spell things out.
fn fmt_totals(t: &Totals) -> String {
    format!(
        "{} transition(s), ${:.2}, {}",
        t.transitions,
        t.cost_usd,
        fmt_duration(t.wallclock_s)
    )
}

fn fmt_usage(u: &Usage) -> String {
    format!("${:.2}, {} token(s)", u.cost_usd, u.tokens)
}

fn fmt_guard(o: GuardOutcome) -> &'static str {
    match o {
        GuardOutcome::Pass => "pass",
        GuardOutcome::Fail => "fail",
        GuardOutcome::Skip => "skip (not configured on this edge)",
    }
}

fn fmt_actor(a: Actor) -> &'static str {
    match a {
        Actor::Worker => "the Worker",
        Actor::Navigator => "the Navigator",
        Actor::Harness => "the harness",
    }
}

fn fmt_artifacts(artifacts: &[ArtifactRef]) -> String {
    join(artifacts.iter().map(|a| format!("{} → {}", a.name, a.path)))
}

fn fmt_resume(r: &ResumePoint) -> String {
    match r {
        ResumePoint::Done => "nothing to resume".into(),
        ResumePoint::Fresh => "fresh, at the machine's entry state".into(),
        ResumePoint::EnterState { state, crashed } => format!(
            "re-enter `{state}`{}",
            if *crashed {
                " — the stage died mid-flight and re-runs from scratch"
            } else {
                ""
            }
        ),
        ResumePoint::GuardCheck { from, proposal } => format!(
            "re-run the guards on `{from}` → `{}`",
            proposal.to.as_deref().unwrap_or("(blocked)")
        ),
    }
}

// ── diagnostics ──────────────────────────────────────────────────────────────

/// The `loop validate` diagnostics, in the same `{tag}  {where}: {message}`
/// form that command prints — preview reuses the real linter, so it must also
/// reuse its wording.
pub fn validation(diagnostics: &[Diagnostic]) -> String {
    let mut rep = Report::new();
    rep.section("validation");
    if diagnostics.is_empty() {
        rep.line_at(2, "no problems found");
        return rep.finish();
    }
    for d in diagnostics {
        rep.line_at(
            2,
            format!("{}  {}: {}", tag(d.severity), d.where_, d.message),
        );
    }
    rep.finish()
}

pub fn tag(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warn ",
    }
}

// ── formatting helpers ───────────────────────────────────────────────────────

fn fmt_model(m: &ModelSpec) -> String {
    format!("{}/{}", m.provider, m.pi_model_arg())
}

fn fmt_budgets(b: &Budgets) -> String {
    let mut parts = Vec::new();
    if let Some(usd) = b.usd {
        parts.push(format!("${usd:.2}"));
    }
    if let Some(s) = b.wallclock_s {
        parts.push(fmt_duration(s));
    }
    if let Some(n) = b.max_transitions {
        parts.push(format!("{n} transition(s)"));
    }
    if parts.is_empty() {
        return "unbounded".into();
    }
    parts.join(", ")
}

fn fmt_playbook_ref(r: &PlaybookRef) -> String {
    match r {
        PlaybookRef::Named(n) => format!("`{n}` (name, local-first)"),
        PlaybookRef::Path(p) => format!("`{}` (path)", p.display()),
        PlaybookRef::Inline(_) => "inline `:prompt`".into(),
    }
}

fn fmt_playbook_source(res: &Resolved<'_>) -> String {
    match &res.playbook.path {
        Some(p) => format!("{} ({})", res.playbook.name, p.display()),
        None => format!("{} (inline)", res.playbook.name),
    }
}

fn fmt_skills(res: &Resolved<'_>) -> String {
    join(
        res.skills
            .iter()
            .zip(&res.skill_paths)
            .map(|(name, path)| format!("{name} ({})", path.display())),
    )
}

fn fmt_on_fail(on_fail: &OnFail) -> String {
    match on_fail {
        OnFail::Retry => "retry the source state".into(),
        OnFail::Abort => "abort the run".into(),
        OnFail::Route(to) => format!("route to `{to}`"),
    }
}

fn fmt_loop_head(l: &LoopSpec) -> String {
    match l.head() {
        Some(head) => format!("head `{head}`"),
        None => "no head (empty loop)".into(),
    }
}

fn fmt_on_exhausted(l: &LoopSpec, r: &Resolver<'_>) -> String {
    match l.on_exhausted {
        OnExhausted::Abort => "abort the run".into(),
        OnExhausted::Escalate => match &r.machine.escalation_state {
            Some(esc) => format!("escalate to `{esc}`"),
            None => "escalate (no escalation state declared)".into(),
        },
    }
}

/// A prose field's shape without printing the whole thing: enough to tell an
/// empty `task.md` from a real one, and to spot the wrong file entirely.
fn prose_summary(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return "(empty)".into();
    }
    format!(
        "{} line(s), {} chars — {}",
        trimmed.lines().count(),
        trimmed.chars().count(),
        truncate(first_line(trimmed), 56)
    )
}

fn join(items: impl IntoIterator<Item = impl AsRef<str>>) -> String {
    let mut out = String::new();
    for item in items {
        if !out.is_empty() {
            out.push_str(", ");
        }
        out.push_str(item.as_ref());
    }
    if out.is_empty() { NONE.into() } else { out }
}

fn display(p: &Path) -> String {
    p.display().to_string()
}

pub fn first_line(s: &str) -> &str {
    s.lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
}

/// `implement#2, qa#1` — the cycle counters, in one spelling shared by `status`
/// and `recap` so the two never disagree on the notation.
pub fn fmt_cycles(rs: &RunState) -> String {
    join(rs.cycles.iter().map(|(s, n)| format!("{s}#{n}")))
}

pub fn fmt_duration(secs: u64) -> String {
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m{}s", s / 60, s % 60),
        s => format!("{}h{}m", s / 3600, (s % 3600) / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use loop_core::{Budgets, ErrorKind, RunStatus};

    fn ev(payload: EventPayload) -> Event {
        Event {
            ts: "2026-07-26T12:00:00.000Z".into(),
            elapsed_s: 0,
            payload,
        }
    }

    fn started() -> Event {
        ev(EventPayload::RunStarted {
            ticket: "T-1".into(),
            machine_hash: "abc".into(),
            budgets: Budgets::default(),
        })
    }

    fn entered(state: &str, cycle: u32, attempt: u32, session: Option<&str>) -> Event {
        ev(EventPayload::StateEntered {
            state: state.into(),
            cycle,
            attempt,
            session_id: session.map(str::to_string),
            model: "claude-sonnet-5".into(),
            thinking: "medium".into(),
            skills: vec![],
            mcp: vec![],
        })
    }

    fn output(state: &str, cycle: u32, summary: &str) -> Event {
        ev(EventPayload::WorkerOutput {
            state: state.into(),
            cycle,
            summary: summary.into(),
            artifacts: vec![],
            usage: Usage::default(),
        })
    }

    fn finished() -> Event {
        ev(EventPayload::RunFinished {
            status: RunStatus::Done,
            terminal_state: Some("done".into()),
            totals: Totals::default(),
        })
    }

    fn note(text: &str) -> Event {
        ev(EventPayload::Note { text: text.into() })
    }

    fn recap_of(events: &[Event]) -> String {
        recap(&Recap {
            events,
            folded: &loop_core::fold(events),
            machine: Provenance::NotLoaded,
        })
    }

    /// The whole point of episode grouping: `worker_output` carries no attempt
    /// field, so the only thing keeping attempt 2's summary off attempt 1 is
    /// the boundary at the next `state_entered`.
    #[test]
    fn an_attempts_output_is_not_credited_to_the_previous_attempt() {
        let events = vec![
            started(),
            entered("implement", 1, 1, None),
            entered("implement", 1, 2, None),
            output("implement", 1, "the second try"),
            finished(),
        ];
        let t = timeline(&events);
        assert_eq!(t.attempts.len(), 2);
        assert_eq!(t.attempts[0].0.attempt, 1);
        assert!(t.attempts[0].1.is_empty());
        assert_eq!(t.attempts[1].0.attempt, 2);
        assert_eq!(t.attempts[1].1.len(), 1);
    }

    /// `run_started` and `run_finished` bracket the run rather than belonging
    /// to whichever attempt happened to be open around them.
    #[test]
    fn the_run_brackets_are_lifted_out_of_the_episodes() {
        let events = vec![started(), entered("a", 1, 1, None), finished()];
        let t = timeline(&events);
        assert!(t.started.is_some());
        assert!(t.finished.is_some());
        assert!(t.prelude.is_empty());
        assert_eq!(t.attempts.len(), 1);
        assert!(t.attempts[0].1.is_empty());
    }

    /// Events before the first `state_entered` are the ones a report that only
    /// walked attempts would silently drop.
    #[test]
    fn events_before_the_first_attempt_are_kept() {
        let events = vec![
            started(),
            note("staging the toolbox"),
            entered("a", 1, 1, None),
        ];
        let t = timeline(&events);
        assert_eq!(t.prelude.len(), 1);
        assert!(recap_of(&events).contains("staging the toolbox"));
    }

    #[test]
    fn a_ledger_with_no_state_entered_still_groups() {
        let events = vec![started(), note("nothing ran")];
        let t = timeline(&events);
        assert!(t.attempts.is_empty());
        assert_eq!(t.prelude.len(), 1);
        assert!(timeline(&[]).attempts.is_empty());
    }

    /// Ledger order is the report's order, including a re-entry of a state
    /// long after another state ran. Sorting by anything else would reorder
    /// the sub-second burst a stage exit writes.
    #[test]
    fn episodes_keep_ledger_order_across_states() {
        let events = vec![
            started(),
            entered("implement", 1, 1, None),
            entered("review", 1, 1, None),
            entered("implement", 2, 1, None),
        ];
        let order: Vec<_> = timeline(&events)
            .attempts
            .iter()
            .map(|(e, _)| (e.state.as_str(), e.cycle))
            .collect();
        assert_eq!(
            order,
            vec![("implement", 1), ("review", 1), ("implement", 2)]
        );
    }

    /// Check output is somebody else's build tool's bytes. A fixed fence would
    /// let it close its own block and let the rest of a run's stdout pass as
    /// the report's prose.
    #[test]
    fn a_fence_grows_past_backticks_in_the_text_it_quotes() {
        let mut rep = Report::new();
        rep.fence("before\n```\n## not a heading\n```\nafter");
        let out = rep.finish();
        assert!(out.starts_with("````\n"), "{out}");
        assert!(out.trim_end().ends_with("\n````"), "{out}");
    }

    /// A failed attempt that produced no `worker_output` and no commit is
    /// exactly the thing a recap must not omit.
    #[test]
    fn an_attempt_that_produced_nothing_still_gets_a_section() {
        let events = vec![
            started(),
            entered("implement", 1, 1, Some("T-1-implement-1-1")),
            ev(EventPayload::Error {
                state: Some("implement".into()),
                kind: ErrorKind::Fatal,
                detail: "the spawn died".into(),
            }),
        ];
        let out = recap_of(&events);
        assert!(
            out.contains("### 1. implement — cycle 1, attempt 1"),
            "{out}"
        );
        assert!(out.contains("the spawn died"), "{out}");
        // Unfinished: the resume point and the last durable event answer "why
        // did it stop" in place of a `run_finished`.
        assert!(out.contains("unfinished"), "{out}");
        assert!(out.contains("re-enter `implement`"), "{out}");
    }

    #[test]
    fn an_attempt_without_a_session_says_so_rather_than_omitting_the_line() {
        let events = vec![started(), entered("implement", 1, 1, Some("   "))];
        let out = recap_of(&events);
        assert!(out.contains("session: (none recorded"), "{out}");
        assert!(out.contains("No attempt recorded a pi session id"), "{out}");
    }

    /// Two recaps of the same ledger must be byte-identical — the property
    /// that makes the report usable as evidence.
    #[test]
    fn the_same_ledger_renders_the_same_report() {
        let events = vec![
            started(),
            entered("implement", 1, 1, Some("s")),
            output("implement", 1, "did it"),
            finished(),
        ];
        assert_eq!(recap_of(&events), recap_of(&events));
        assert!(recap_of(&events).ends_with("\n"));
        assert!(!recap_of(&events).ends_with("\n\n"));
    }
}
