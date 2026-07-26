//! Text reports for the read-only inspection commands.
//!
//! `loop preview` renders here rather than in `commands.rs` so the layout —
//! one value column, sections in a fixed order, every list in the machine's
//! own deterministic order — lives in one place. Nothing in this module opens
//! a file for writing or creates a directory: a report is a pure function of
//! the machine, the config, and what the toolbox already has on disk.

use std::fmt::Write as _;
use std::path::Path;

use loop_core::{
    Budgets, Check, Context, LoopSpec, ModelSpec, OnExhausted, OnFail, PlaybookRef, StateId,
    Transition,
};
use loop_engine::{Diagnostic, Severity};
use loop_toolbox::render;

use crate::output::truncate;
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
    rep.field(
        "transition mode",
        format!("{:?}", m.transition_mode).to_lowercase(),
    );
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
    rep.field(
        "transition mode",
        format!("{:?}", m.transition_mode).to_lowercase(),
    );
    rep.field("reachable", join(m.neighbors(id)));
    rep.field("cwd", display(&config.paths.project_dir));
    rep.field(
        "ext",
        display(&config.paths.ext_dir().join("transition-tool.ts")),
    );
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

fn join(items: impl IntoIterator<Item = String>) -> String {
    let joined = items.into_iter().collect::<Vec<_>>().join(", ");
    if joined.is_empty() {
        NONE.into()
    } else {
        joined
    }
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

pub fn fmt_duration(secs: u64) -> String {
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m{}s", s / 60, s % 60),
        s => format!("{}h{}m", s / 3600, (s % 3600) / 60),
    }
}
