//! The twelve subcommands.

use std::io::Write as _;
use std::path::Path;

use anyhow::{Context as _, Result, anyhow, bail};

use crate::core::{Config, LedgerSink, Machine, Paths};
use crate::engine::{Diagnostic, Engine, Severity};
use crate::fennel::FennelVm;
use crate::ledger::{ArtifactStore, Ledger};
use crate::output::{summarize, truncate};
use crate::report::{self, fmt_duration};
use crate::runner::PiRunner;
use crate::sessions::{self, Candidate};
use crate::stage::{CliStage, Resolver};
use crate::toolbox::Toolbox;

// Templates shipped in the binary, so a fresh install needs no fetch.
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

/// Copy one directory tree into another, never overwriting. Returns what it
/// wrote, for `init` to print.
fn copy_tree(from: &Path, to: &Path, created: &mut Vec<String>) -> Result<()> {
    for entry in std::fs::read_dir(from)
        .with_context(|| format!("reading template directory {}", from.display()))?
    {
        let entry = entry?;
        let src = entry.path();
        let dst = to.join(entry.file_name());
        if src.is_dir() {
            std::fs::create_dir_all(&dst)?;
            copy_tree(&src, &dst, created)?;
        } else if !dst.exists() {
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&src, &dst)
                .with_context(|| format!("copying {} to {}", src.display(), dst.display()))?;
            created.push(dst.display().to_string());
        }
    }
    Ok(())
}

/// Keep the derived `run/` directory out of version control.
///
/// `.loop/` is meant to be committed — it is the record of what drove the
/// ticket — but the rendered prompts and handoff files under `run/` are
/// regenerated every stage and would be pure churn in a diff.
fn ignore_run_dir(paths: &Paths, created: &mut Vec<String>) -> Result<()> {
    let gitignore = paths.loop_dir().join(".gitignore");
    if write_if_absent(&gitignore, "run/\n")? {
        created.push(gitignore.display().to_string());
    }
    Ok(())
}

pub fn init(paths: Paths, ticket: &str, from: Option<&Path>) -> Result<()> {
    let machine_file = paths.machine_file();
    if machine_file.exists() {
        bail!(
            "{} already exists — delete .loop/ to start a new ticket",
            machine_file.display()
        );
    }

    let loop_dir = paths.loop_dir();
    std::fs::create_dir_all(&loop_dir)?;
    let mut created = Vec::new();

    match from {
        // Reuse is a copy, not a resolution path. What you started from is
        // recorded in the ticket rather than looked up at run time, so editing
        // the source later cannot change a run already in flight.
        Some(src) => {
            let src = crate::core::config::expand_tilde(src);
            if !src.is_dir() {
                bail!("--from {} is not a directory", src.display());
            }
            copy_tree(&src, &loop_dir, &mut created)?;
            if !machine_file.exists() {
                bail!(
                    "{} has no machine.fnl — --from wants a directory shaped like .loop/",
                    src.display()
                );
            }
            // The template's own ticket id is whoever's it was; this one is ours.
            let body = std::fs::read_to_string(&machine_file)?;
            std::fs::write(&machine_file, body.replace("$TICKET", ticket))?;
        }
        None => {
            if write_if_absent(&machine_file, &STANDARD_TICKET.replace("$TICKET", ticket))? {
                created.push(machine_file.display().to_string());
            }
            for (name, body) in PLAYBOOKS {
                let p = paths.playbooks().join(name);
                if write_if_absent(&p, body)? {
                    created.push(p.display().to_string());
                }
            }
        }
    }

    for (name, body) in [("task.md", TASK_MD), ("plan.md", PLAN_MD)] {
        let p = loop_dir.join(name);
        if write_if_absent(&p, &body.replace("$TICKET", ticket))? {
            created.push(p.display().to_string());
        }
    }
    std::fs::create_dir_all(paths.playbooks())?;
    std::fs::create_dir_all(paths.skills())?;
    ignore_run_dir(&paths, &mut created)?;

    for c in &created {
        println!("  created {c}");
    }
    println!("\ninitialized {} for {ticket}", loop_dir.display());
    println!("  1. write .loop/task.md and .loop/plan.md");
    println!("  2. hack .loop/machine.fnl into the shape this ticket needs");
    println!("  3. loop validate");
    println!("  4. loop run");
    Ok(())
}

/// Load config + machine. Shared by every command that needs the graph.
fn load(paths: Paths) -> Result<(FennelVm, Config, Machine)> {
    let vm = FennelVm::new()?;
    let config = Config::defaults(paths.clone());
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

/// Lint a loaded machine against the ticket directory. `validate` and
/// `preview` share it, so preview reports exactly the problems `validate`
/// would — there is no weaker preview-only linter.
fn diagnose(config: &Config, machine: &Machine) -> Vec<Diagnostic> {
    let toolbox = Toolbox::new(config);
    crate::engine::validate(
        machine,
        &|r| toolbox.resolve_playbook(r, &machine.dir).is_ok(),
        &|name| toolbox.resolve_skill(name, &machine.dir).is_ok(),
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
    print!("{}", crate::engine::mermaid(&machine));
    Ok(())
}

/// `loop preview` — what this machine will do, answered before anything is
/// spawned.
///
/// Read-only and deterministic by construction: it resolves through
/// `Resolver`, the same code `build_stage` runs, but stops short of every
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
        machine.budgets = machine.budgets.tighten(crate::core::Budgets {
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
        crate::core::RunStatus::Done => Ok(()),
        crate::core::RunStatus::Failed => bail!(
            "run ended at `{}` without completing — see `loop status`",
            outcome.terminal_state.as_deref().unwrap_or("?")
        ),
        crate::core::RunStatus::Aborted => {
            bail!("run aborted — see `loop status` for the guardrail")
        }
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
                    "totals": crate::core::Totals::default(),
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
        Some(m) => crate::core::fold_with_loop_heads(&events, &|s| m.loop_with_head(s).is_some()),
        None => crate::core::fold(&events),
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
        crate::core::FoldStatus::NotStarted => println!("not started"),
        crate::core::FoldStatus::Running => println!(
            "running — at `{}`",
            folded.current.as_deref().unwrap_or("?")
        ),
        crate::core::FoldStatus::Finished(s) => println!("finished — {s:?}"),
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
        crate::core::EventPayload::RunStarted { machine_hash, .. } => Some(machine_hash.as_str()),
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
            crate::core::fold_with_loop_heads(&events, &|s| m.loop_with_head(s).is_some())
        }
        _ => crate::core::fold(&events),
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

/// The failure both `loop sessions` and `loop session --latest` end at when the
/// ledger holds nothing to reopen.
///
/// It names the ledger it read and the filter it applied, because "no sessions
/// here" and "that state never ran" are the same silence otherwise — and the
/// second one is a typo the operator can fix.
fn no_candidate(ledger_path: &Path, state: Option<&str>) -> anyhow::Error {
    anyhow!(
        "no Worker session in {} matching {} — sessions come from \
         `state_entered.session_id`; `loop status` shows what the ledger holds",
        ledger_path.display(),
        match state {
            Some(s) => format!("state `{s}`"),
            None => "any state".to_string(),
        },
    )
}

/// `loop sessions` — every Worker attempt this ledger recorded a session for.
///
/// Prints and stops. Choosing is the shell's job now: `loop sessions | fzf` is
/// the picker this replaced, and `loop sessions implement | awk '{print $6}'` is
/// every pipeline the picker could never be part of.
///
/// Needs neither a valid machine nor a staged toolbox: the ledger and the
/// project directory are the whole input, and a mid-edit `machine.fnl` is
/// exactly when you want to read what the last Worker did.
pub fn sessions(paths: Paths, state: Option<String>) -> Result<()> {
    let ledger_path = paths.ledger_file();
    // Opening repairs a torn trailing line, so an attempt interrupted mid-write
    // is still listed rather than making the whole ledger unreadable.
    let events = Ledger::open(&ledger_path)?.read_all()?;
    let candidates = sessions::candidates(&events);
    let listed = sessions::filter_state(&candidates, state.as_deref());
    if listed.is_empty() {
        return Err(no_candidate(&ledger_path, state.as_deref()));
    }
    print!("{}", sessions::listing(&listed));
    Ok(())
}

/// `loop session <ID>` with an id this ledger does not hold.
///
/// A state name is the likeliest thing to land here — it is what the positional
/// meant back when this command opened a picker — so when the argument names a
/// state, the message is the working command rather than a correction.
fn unknown_id(ledger_path: &Path, id: &str, candidates: &[Candidate]) -> anyhow::Error {
    let mut msg = format!(
        "no attempt in {} has session id `{id}` — `loop sessions` lists every recorded id",
        ledger_path.display(),
    );
    if sessions::states(candidates).contains(&id) {
        msg.push_str(&format!(
            "\n`{id}` is a state, not a session id: `loop sessions {id}` lists its attempts, and \
             `loop session --latest {id}` opens the newest"
        ));
    }
    anyhow!(msg)
}

/// Reopen a Worker's pi session.
///
/// loop keeps no transcript of its own: a stage's full history — assistant
/// messages, tool calls, results, usage — already lives in the session pi
/// persisted under the deterministic id on that stage's `state_entered` line.
/// This command's whole job is to hand that id back to pi and get out of the
/// way; finding it is [`sessions`]'s job.
///
/// Needs neither a valid machine nor a staged toolbox: a ledger line and the
/// project directory are the whole input, and a mid-edit `machine.fnl` is
/// exactly when you want to read what the last Worker did.
pub fn session(paths: Paths, id: Option<String>, latest: bool) -> Result<()> {
    let config = Config::defaults(paths.clone());
    let ledger_path = paths.ledger_file();
    let events = Ledger::open(&ledger_path)?.read_all()?;
    let candidates = sessions::candidates(&events);
    let ticket = sessions::ticket(&events);

    let chosen: &Candidate = match (id.as_deref(), latest) {
        // The deterministic path scripts and CI use: last candidate in ledger
        // order, after the state filter. The positional means a state here,
        // because there is nothing left for an id to disambiguate.
        (state, true) => {
            sessions::latest(&candidates, state).ok_or_else(|| no_candidate(&ledger_path, state))?
        }
        (Some(id), false) => sessions::find(&candidates, id)
            .ok_or_else(|| unknown_id(&ledger_path, id, &candidates))?,
        // The interactive picker this command used to open is gone. Saying so,
        // with the command that replaced it, is the whole difference between a
        // removed feature and a broken one.
        (None, false) => bail!(
            "`loop session` no longer opens a picker — run `loop sessions` to list every recorded \
             attempt, then `loop session <ID>` with an id from that listing. \
             `loop session --latest [STATE]` still opens the newest attempt without naming one."
        ),
    };

    // What is about to open, in the terms the listing used, because an opaque id
    // is not an answer to "which attempt was that?".
    let mut line = String::from("opening ");
    if let Some(t) = &ticket {
        line.push_str(&format!("{t}  "));
    }
    line.push_str(&chosen.headline());
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
    // Every label names the path actually tested. Printing one path while
    // checking another is worst precisely when you are running doctor to find
    // out where loop is looking.
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
