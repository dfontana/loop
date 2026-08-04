//! The eleven subcommands.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::Path;

use anyhow::{Context as _, Result, bail};
use loop_core::{Config, LedgerSink, Machine, Paths};
use loop_engine::{Diagnostic, Engine, Severity};
use loop_fennel::FennelVm;
use loop_ledger::{ArtifactStore, Ledger};
use loop_runner::PiRunner;
use loop_toolbox::Toolbox;

use crate::output::{summarize, truncate};
use crate::report::{self, fmt_duration};
use crate::session_picker::{self, Candidate, Picker, Scope};
use crate::session_ui;
use crate::stage::{CliStage, Resolver};

// Templates shipped in the binary, so a fresh install needs no fetch.
const CONFIG_FNL: &str = include_str!("../templates/config.fnl");
const STANDARD_TICKET: &str = include_str!("../templates/machines/standard-ticket.fnl");
const TASK_MD: &str = include_str!("../templates/task.md");
const PLAN_MD: &str = include_str!("../templates/plan.md");
const PLAYBOOKS: &[(&str, &str)] = &[
    (
        "implement.md",
        include_str!("../templates/playbooks/implement.md"),
    ),
    (
        "review.md",
        include_str!("../templates/playbooks/review.md"),
    ),
    ("qa.md", include_str!("../templates/playbooks/qa.md")),
    (
        "open-pr.md",
        include_str!("../templates/playbooks/open-pr.md"),
    ),
    (
        "debug-transient.md",
        include_str!("../templates/playbooks/debug-transient.md"),
    ),
];

/// Write `content` to `path` unless it already exists. Returns whether it wrote.
fn write_if_absent(path: &Path, content: &str) -> Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(path, content).with_context(|| format!("writing {}", path.display()))?;
    Ok(true)
}

/// Materialize the global toolbox if it isn't there yet. Idempotent, and never
/// overwrites anything the user has edited.
fn ensure_toolbox(config: &Config) -> Result<Vec<String>> {
    let paths = &config.paths;
    let mut created = Vec::new();
    if write_if_absent(&paths.config_file(), CONFIG_FNL)? {
        created.push(paths.config_file().display().to_string());
    }
    let machines = paths.toolbox_machines().join("standard-ticket.fnl");
    if write_if_absent(&machines, STANDARD_TICKET)? {
        created.push(machines.display().to_string());
    }
    for (name, body) in PLAYBOOKS {
        let p = paths.toolbox_playbooks().join(name);
        if write_if_absent(&p, body)? {
            created.push(p.display().to_string());
        }
    }
    std::fs::create_dir_all(paths.toolbox_skills())?;
    Ok(created)
}

pub fn init(paths: Paths, ticket: &str, template: &str) -> Result<()> {
    let config = Config::defaults(paths.clone());
    for created in ensure_toolbox(&config)? {
        println!("  created {created}");
    }

    let machine_file = paths.machine_file();
    if machine_file.exists() {
        bail!(
            "{} already exists — delete .loop/ to start a new ticket",
            machine_file.display()
        );
    }

    let template_path = paths.toolbox_machines().join(format!("{template}.fnl"));
    let body = std::fs::read_to_string(&template_path)
        .with_context(|| format!("no machine template at {}", template_path.display()))?;
    let body = body.replace("$TICKET", ticket);

    write_if_absent(&machine_file, &body)?;
    write_if_absent(
        &paths.loop_dir().join("task.md"),
        &TASK_MD.replace("$TICKET", ticket),
    )?;
    write_if_absent(
        &paths.loop_dir().join("plan.md"),
        &PLAN_MD.replace("$TICKET", ticket),
    )?;
    std::fs::create_dir_all(paths.local_playbooks())?;

    println!("\ninitialized {} for {ticket}", paths.loop_dir().display());
    println!("  1. write .loop/task.md and .loop/plan.md");
    println!("  2. hack .loop/machine.fnl into the shape this ticket needs");
    println!("  3. loop validate");
    println!("  4. loop run");
    Ok(())
}

/// Load config + machine. Shared by every command that needs the graph.
fn load(paths: Paths) -> Result<(FennelVm, Config, Machine)> {
    let vm = FennelVm::new()?;
    let config = vm.load_config(paths.clone())?;
    let machine_file = paths.machine_file();
    if !machine_file.exists() {
        bail!(
            "no machine at {} — run `loop init <TICKET>` first",
            machine_file.display()
        );
    }
    let machine = vm.load_machine(&machine_file, &config)?;
    Ok((vm, config, machine))
}

/// Lint a loaded machine against the toolbox on disk. `validate` and `preview`
/// share it, so preview reports exactly the problems `validate` would — there
/// is no weaker preview-only linter.
fn diagnose(config: &Config, machine: &Machine) -> Vec<Diagnostic> {
    let toolbox = Toolbox::new(config);
    loop_engine::validate(
        machine,
        &|r| toolbox.resolve_playbook(r, &machine.dir).is_ok(),
        &|name| toolbox.resolve_skill(name, &machine.dir).is_ok(),
        config.pi_extensions.iter().any(|e| e == "mcp"),
        &config.default_skills,
        &config.default_mcp,
    )
}

fn error_count(diagnostics: &[Diagnostic]) -> usize {
    diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count()
}

pub fn validate(paths: Paths) -> Result<()> {
    let (_vm, config, machine) = load(paths)?;
    let diagnostics = diagnose(&config, &machine);

    let errors = error_count(&diagnostics);
    for d in &diagnostics {
        println!("{}  {}: {}", report::tag(d.severity), d.where_, d.message);
    }
    if diagnostics.is_empty() {
        println!(
            "{} — {} states, {} transitions, no problems found",
            machine.ticket,
            machine.states.len(),
            machine.transitions.len()
        );
    }
    if errors > 0 {
        bail!("{errors} error(s)");
    }
    Ok(())
}

/// Print the machine as mermaid. Bare — no fences, no prose — so it pipes:
/// `loop diagram > machine.mmd`. Unlike `validate` this doesn't touch the
/// toolbox, since drawing the graph needs nothing off the filesystem beyond
/// the machine file itself; a machine with a dangling playbook still draws.
pub fn diagram(paths: Paths) -> Result<()> {
    let (_vm, _config, machine) = load(paths)?;
    print!("{}", loop_engine::mermaid(&machine));
    Ok(())
}

/// `loop preview` — what this machine will do, answered before anything is
/// spawned.
///
/// Read-only and deterministic by construction: it resolves through
/// [`Resolver`], the same code `build_stage` runs, but stops short of every
/// write that stage building does. No `ext/*.ts` is materialized, no ledger or
/// artifact directory is created, and nothing lands under `LOOP_STATE_DIR` —
/// the render a state preview shows is built in memory and printed.
pub fn preview(paths: Paths, state: Option<String>) -> Result<()> {
    let (_vm, config, machine) = load(paths)?;

    // An unknown state is the operator's typo, not the machine's problem: fail
    // on it before printing a report about something they did not ask for.
    if let Some(id) = &state
        && !machine.states.contains_key(id)
    {
        bail!(
            "no state `{id}` in {} — states: {}",
            machine.source_path.display(),
            machine
                .states
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    let resolver = Resolver::new(&machine, &config);
    print!(
        "{}",
        match &state {
            Some(id) => report::state_preview(&resolver, id),
            None => report::machine_preview(&resolver),
        }
    );

    // Diagnostics come last so the report reads top-down and the problems are
    // the final thing on screen — the same reason `run`'s summary precedes its
    // error.
    let diagnostics = diagnose(&config, &machine);
    print!("{}", report::validation(&diagnostics));
    let errors = error_count(&diagnostics);
    if errors > 0 {
        bail!("{errors} error(s) — this machine will not run as previewed");
    }
    Ok(())
}

pub fn run(
    paths: Paths,
    max_transitions: Option<u32>,
    resuming: bool,
    verbose: bool,
) -> Result<()> {
    // The VM is only needed to *load* the machine. It used to have to outlive
    // the run because it held the `when` guard closures; the IR is plain data
    // now, so it can be dropped here.
    let (_vm, config, mut machine) = load(paths.clone())?;
    if let Some(max) = max_transitions {
        machine.budgets = machine.budgets.tighten(loop_core::Budgets {
            usd: None,
            wallclock_s: None,
            max_transitions: Some(max),
        });
    }

    let ledger_path = paths.ledger_file();
    let started = Ledger::open(&ledger_path)?.started();
    match (started, resuming) {
        (false, true) => bail!("nothing to resume: {} is empty", ledger_path.display()),
        (true, false) => bail!(
            "{} already has a run — use `loop resume`, or delete it to start over",
            ledger_path.display()
        ),
        _ => {}
    }

    let mut ledger = Ledger::open(&ledger_path)?;
    // Read before the engine borrows the ledger: the time budget bounds the
    // run, so a resume starts its clock at what the interrupted session
    // already burned rather than at zero.
    let elapsed_offset_s = ledger.elapsed_offset_s();
    let artifacts = ArtifactStore::new(paths.artifacts_dir(), &paths.project_dir);
    let runner = PiRunner::new(&config).verbose(verbose);
    let stage = CliStage {
        machine: &machine,
        config: &config,
        toolbox: Toolbox::new(&config),
        ledger_path: ledger_path.clone(),
    };

    let mut engine = Engine {
        machine: &machine,
        config: &config,
        runner: &runner,
        checks: &stage,
        ledger: &mut ledger,
        artifacts: &artifacts,
        stage: &stage,
        started_at: None,
        elapsed_offset_s,
    };
    let outcome = engine.run()?;

    println!(
        "\n{:?} — {} after {} transitions, ${:.2}, {}",
        outcome.status,
        outcome.terminal_state.as_deref().unwrap_or("(no terminal)"),
        outcome.totals.transitions,
        outcome.totals.cost_usd,
        fmt_duration(outcome.totals.wallclock_s),
    );

    // A run that escalated or blew a budget must not report success: this exit
    // status is what a CI wrapper or a `loop run && gh pr merge` gates on.
    match outcome.status {
        loop_core::RunStatus::Done => Ok(()),
        loop_core::RunStatus::Failed => bail!(
            "run ended at `{}` without completing — see `loop status`",
            outcome.terminal_state.as_deref().unwrap_or("?")
        ),
        loop_core::RunStatus::Aborted => bail!("run aborted — see `loop status` for the guardrail"),
    }
}

pub fn status(paths: Paths, json: bool) -> Result<()> {
    let ledger = Ledger::open(paths.ledger_file())?;
    let events = ledger.read_all()?;
    // An empty ledger still has to answer in the mode it was asked in: this
    // branch used to print prose in both, so `loop status --json` on a fresh
    // project handed a parser a sentence.
    if events.is_empty() {
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "current": null,
                    "status": null,
                    "cycles": {},
                    "totals": loop_core::Totals::default(),
                    "navigator_invocations": 0,
                }))?
            );
        } else {
            println!("no run yet — `loop run` starts one");
        }
        return Ok(());
    }
    // Fold against the machine's real loop heads when the machine still loads,
    // so "cycles" means cycles. The bare `fold` treats every state as a head,
    // which would list `done#1` alongside a genuine `implement#2`. Status has
    // to keep working when the machine is missing or mid-edit, though — that is
    // often exactly when you want it — so a load failure just costs the line.
    let machine = load(paths.clone()).ok().map(|(_vm, _cfg, m)| m);
    let folded = match &machine {
        Some(m) => loop_core::fold_with_loop_heads(&events, &|s| m.loop_with_head(s).is_some()),
        None => loop_core::fold(&events),
    };

    if json {
        let out = serde_json::json!({
            "current": folded.current,
            "status": folded.status,
            "cycles": folded.cycles,
            "totals": folded.totals,
            "navigator_invocations": folded.navigator_invocations,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    match folded.fold_status() {
        loop_core::FoldStatus::NotStarted => println!("not started"),
        loop_core::FoldStatus::Running => println!(
            "running — at `{}`",
            folded.current.as_deref().unwrap_or("?")
        ),
        loop_core::FoldStatus::Finished(s) => println!("finished — {s:?}"),
    }
    println!(
        "  {} transitions, ${:.2}, {}",
        folded.totals.transitions,
        folded.totals.cost_usd,
        fmt_duration(folded.totals.wallclock_s)
    );
    if machine.is_some() && !folded.cycles.is_empty() {
        println!("  cycles: {}", report::fmt_cycles(&folded));
    }
    println!("\nrecent:");
    for e in events
        .iter()
        .rev()
        .take(12)
        .collect::<Vec<_>>()
        .iter()
        .rev()
    {
        println!("  {}  {}", e.ts, summarize(e));
    }
    Ok(())
}

pub fn logs(paths: Paths, n: usize, raw: bool) -> Result<()> {
    let ledger = Ledger::open(paths.ledger_file())?;
    if raw {
        std::io::stdout().write_all(&ledger.read_raw()?)?;
        return Ok(());
    }

    let events = ledger.read_all()?;
    if events.is_empty() {
        println!("no run yet — `loop run` starts one");
        return Ok(());
    }

    for event in events.iter().rev().take(n).collect::<Vec<_>>().iter().rev() {
        println!("{}  {}", event.ts, summarize(event));
    }
    Ok(())
}

/// `loop recap` — what this run did and why, answered from the ledger.
///
/// The deterministic counterpart to `preview`: that command explains the
/// declaration before a run, this one explains the observed execution after or
/// during one. No LLM is involved and nothing is stored — the report is a pure
/// function of the ledger, so the same ledger renders the same recap.
///
/// The ledger is the only source of truth. `machine.fnl` is loaded
/// opportunistically and trusted *only* when its hash still matches the one
/// `run_started` recorded: a machine edited since the run cannot be used to
/// explain decisions the run made under the old one, and saying so is more
/// useful than quietly labelling a historical attempt with a description
/// somebody wrote afterwards.
pub fn recap(paths: Paths) -> Result<()> {
    let ledger_path = paths.ledger_file();
    let events = Ledger::open(&ledger_path)?.read_all()?;
    // An empty ledger has no run to report on. Unlike `status` and `logs`,
    // which are asked "where is it?" and can truthfully answer "nowhere", a
    // recap of nothing is a request that cannot be served — and a report file
    // containing only headings is worse than an error.
    if events.is_empty() {
        bail!(
            "no run to recap: {} is empty — `loop run` starts one",
            ledger_path.display()
        );
    }

    let recorded_hash = events.iter().find_map(|e| match &e.payload {
        loop_core::EventPayload::RunStarted { machine_hash, .. } => Some(machine_hash.as_str()),
        _ => None,
    });
    // No recorded hash means nothing on disk could ever be proven to be the
    // machine that ran, so the Fennel VM is never started for one.
    let loaded = recorded_hash.and_then(|_| load(paths.clone()).ok().map(|(_vm, _cfg, m)| m));

    let machine = match (recorded_hash, &loaded) {
        (Some(recorded), Some(m)) if recorded == m.source_hash => report::Provenance::Matches(m),
        (Some(recorded), Some(m)) => {
            // stderr, so `loop recap > run-recap.md` still produces a clean
            // file. The report repeats the mismatch in its own summary, so the
            // warning is a nudge rather than the only place it appears.
            eprintln!(
                "warning: {} has changed since this run started (ledger {recorded}, on disk \
                 {}) — the recap reports only what the ledger recorded",
                paths.machine_file().display(),
                m.source_hash,
            );
            report::Provenance::Changed {
                current: m.source_hash.clone(),
            }
        }
        _ => report::Provenance::NotLoaded,
    };

    let folded = match &machine {
        report::Provenance::Matches(m) => {
            loop_core::fold_with_loop_heads(&events, &|s| m.loop_with_head(s).is_some())
        }
        _ => loop_core::fold(&events),
    };

    print!(
        "{}",
        report::recap(&report::Recap {
            events: &events,
            folded: &folded,
            machine,
        })
    );
    Ok(())
}

/// Reopen a Worker's pi session.
///
/// loop keeps no transcript of its own: a stage's full history — assistant
/// messages, tool calls, results, usage — already lives in the session pi
/// persisted under the deterministic id on that stage's `state_entered` line.
/// This command's whole job is to help a human *find* the right one of those
/// ids and then get out of the way.
///
/// Which is why the picker is the normal path rather than a convenience. The
/// ids are `<ticket>-<state>-<cycle>-<attempt>` — machine-recognizable, not
/// human-recognizable — so asking someone to name one would mean asking them to
/// remember which cycle the review failed on. The rows carry that instead.
///
/// Needs neither a valid machine nor a staged toolbox: a ledger line and the
/// project directory are the whole input, and a mid-edit `machine.fnl` is
/// exactly when you want to read what the last Worker did.
pub fn session(paths: Paths, state: Option<String>, latest: bool) -> Result<()> {
    use loop_core::LedgerSink;

    let config = Config::defaults(paths.clone());
    let ledger_path = paths.ledger_file();
    // Opening repairs a torn trailing line, so an attempt interrupted mid-write
    // is still selectable rather than making the whole ledger unreadable.
    let events = Ledger::open(&ledger_path)?.read_all()?;
    let candidates = session_picker::candidates(&events);
    let ticket = session_picker::ticket(&events);
    let state_filter = state.as_deref();
    let descriptions = state_descriptions(&paths);
    let describe = |s: &str| descriptions.get(s).cloned();

    if session_picker::filter_state(&candidates, state_filter).is_empty() {
        bail!(
            "no Worker session in {} matching {} (selection mode: {}) — \
             sessions come from `state_entered.session_id`; `loop status` shows what the ledger holds",
            ledger_path.display(),
            match state_filter {
                Some(s) => format!("state `{s}`"),
                None => "any state".to_string(),
            },
            if latest {
                "--latest"
            } else {
                Scope::All.label()
            },
        );
    }

    let chosen: &Candidate = if latest {
        // The deterministic escape hatch: last usable candidate in reverse
        // ledger order, after the prefilter. Nothing to choose, so no terminal
        // is required.
        session_picker::latest(&candidates, state_filter).expect("filter_state was non-empty above")
    } else {
        if !session_ui::is_interactive() {
            bail!(
                "`loop session` needs a terminal to show the picker (stdin and stdout must both be \
                 TTYs) — use `loop session {}--latest` to select without one",
                state_filter.map(|s| format!("{s} ")).unwrap_or_default(),
            );
        }
        let mut picker = Picker::new(&candidates, state_filter, describe);
        let Some(ordinal) = session_ui::pick(&mut picker, ticket.as_deref())? else {
            // A cancelled picker is a completed command that launched nothing.
            println!("cancelled");
            return Ok(());
        };
        candidates
            .iter()
            .find(|c| c.ordinal == ordinal)
            .expect("the picker only ever returns an ordinal it was given")
    };

    // The human-readable identification of what is about to open. The opaque
    // session id is deliberately not here: it is a key for pi, not a name for a
    // person, and printing it as the normal selection UI teaches the wrong habit.
    let mut line = String::from("opening ");
    if let Some(t) = &ticket {
        line.push_str(&format!("{t}  "));
    }
    line.push_str(&chosen.headline(describe(&chosen.state).as_deref()));
    if let Some(detail) = chosen.detail() {
        line.push_str(&format!(" — {}", truncate(&detail, 72)));
    }
    println!("{line}");

    if !chosen.is_complete() {
        eprintln!(
            "warning: no worker_output for this attempt — the session may still be active, or the \
             spawn crashed"
        );
        if let Some(err) = chosen.errors.first() {
            eprintln!("warning: the ledger recorded: {}", truncate(err, 120));
        }
    }

    launch_pi_session(&config, &chosen.session_id)
}

/// Hand the terminal to pi with the recorded session id, in the project
/// directory, with stdin/stdout/stderr inherited untouched.
///
/// `--session` rather than `--session-id`: this command exists to *read*
/// history, so a session pi no longer has must fail loudly instead of silently
/// creating a fresh empty one under the same id and looking like the work
/// vanished.
fn launch_pi_session(config: &Config, session_id: &str) -> Result<()> {
    let mut cmd = std::process::Command::new(&config.pi_bin);
    cmd.arg("--session")
        .arg(session_id)
        .current_dir(&config.paths.project_dir);

    let status = cmd.status().with_context(|| {
        format!(
            "launching `{} --session {session_id}` — install pi, or set LOOP_PI_BIN",
            config.pi_bin
        )
    })?;
    if !status.success() {
        bail!(
            "`{} --session {session_id}` exited {}",
            config.pi_bin,
            status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "on a signal".into()),
        );
    }
    Ok(())
}

/// State descriptions from the machine, if one happens to load.
///
/// Best-effort by construction: the descriptions only enrich a row's text, so a
/// missing, unparseable, or half-edited `machine.fnl` costs that enrichment and
/// nothing else. Failing here instead would make history unreadable at exactly
/// the moment the machine is being rewritten.
fn state_descriptions(paths: &Paths) -> BTreeMap<String, String> {
    let Ok((_vm, _config, machine)) = load(paths.clone()) else {
        return BTreeMap::new();
    };
    machine
        .states
        .iter()
        .filter_map(|(id, st)| st.description.clone().map(|d| (id.clone(), d)))
        .collect()
}

pub fn doctor(paths: Paths) -> Result<()> {
    let mut problems = 0;
    let mut check = |ok: bool, label: &str, hint: &str| {
        if ok {
            println!("  ok    {label}");
        } else {
            problems += 1;
            println!("  FAIL  {label} — {hint}");
        }
    };

    let config = Config::defaults(paths.clone());
    let pi_found = which(&config.pi_bin).is_some();
    check(
        pi_found,
        &format!("`{}` on PATH", config.pi_bin),
        "install pi, or set LOOP_PI_BIN",
    );
    // Every label names the path actually tested. Printing `~/.config/loop/…`
    // while checking somewhere else is worst precisely when you are running
    // doctor to find out where loop is looking.
    check(
        paths.config_file().exists(),
        &paths.config_file().display().to_string(),
        "run `loop init <TICKET>` to scaffold the toolbox",
    );
    check(
        paths.machine_file().exists(),
        &paths.machine_file().display().to_string(),
        "run `loop init <TICKET>` in this project",
    );

    if problems > 0 {
        bail!("{problems} problem(s)");
    }
    println!("\nall good");
    Ok(())
}

fn which(bin: &str) -> Option<std::path::PathBuf> {
    if bin.contains('/') {
        let p = std::path::PathBuf::from(bin);
        return p.exists().then_some(p);
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(bin))
            .find(|p| p.is_file())
    })
}
