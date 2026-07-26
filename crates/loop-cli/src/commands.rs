//! The six subcommands.

use std::path::Path;

use anyhow::{Context as _, Result, bail};
use loop_core::{Config, Machine, Paths};
use loop_engine::{Engine, Severity};
use loop_fennel::FennelVm;
use loop_ledger::{ArtifactStore, Ledger};
use loop_runner::PiRunner;
use loop_toolbox::Toolbox;

use crate::stage::CliStage;

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
    std::fs::create_dir_all(paths.toolbox_tools())?;
    Toolbox::new(config).materialize_ext()?;
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

pub fn validate(paths: Paths) -> Result<()> {
    let (_vm, config, machine) = load(paths)?;
    let toolbox = Toolbox::new(&config);
    let diagnostics = loop_engine::validate(&machine, &|r| {
        toolbox.resolve_playbook(r, &machine.dir).is_ok()
    });

    let errors = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count();
    for d in &diagnostics {
        let tag = match d.severity {
            Severity::Error => "error",
            Severity::Warning => "warn ",
        };
        println!("{tag}  {}: {}", d.where_, d.message);
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

pub fn run(paths: Paths, max_transitions: Option<u32>, resuming: bool) -> Result<()> {
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

    let toolbox = Toolbox::new(&config);
    let (agent_dir, warnings) = toolbox.stage_agent_dir()?;
    for w in &warnings {
        eprintln!("warn  {w}");
    }
    let ext = toolbox.materialize_ext()?;

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
    let artifacts = ArtifactStore::new(paths.artifacts_dir(), &paths.project_dir);
    let runner = PiRunner::new(&config);
    let stage = CliStage {
        machine: &machine,
        config: &config,
        toolbox: Toolbox::new(&config),
        agent_dir,
        ext,
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
    use loop_core::LedgerSink;
    let ledger = Ledger::open(paths.ledger_file())?;
    let events = ledger.read_all()?;
    if events.is_empty() {
        println!("no run yet — `loop run` starts one");
        return Ok(());
    }
    // Fold against the machine's real loop heads when the machine still loads,
    // so "cycles" means cycles. The bare `fold` treats every state as a head,
    // which would list `done#1` alongside a genuine `implement#2`. Status has
    // to keep working when the machine is missing or mid-edit, though — that is
    // often exactly when you want it — so a load failure just costs the line.
    let loop_heads: Option<Vec<String>> = load(paths.clone()).ok().map(|(_vm, _cfg, m)| {
        m.loops
            .iter()
            .filter_map(|l| l.head().cloned())
            .collect::<Vec<_>>()
    });
    let folded = match &loop_heads {
        Some(heads) => loop_core::fold_with_loop_heads(&events, &|s| heads.iter().any(|h| h == s)),
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
    if loop_heads.is_some() && !folded.cycles.is_empty() {
        let cycles: Vec<String> = folded
            .cycles
            .iter()
            .map(|(s, n)| format!("{s}#{n}"))
            .collect();
        println!("  cycles: {}", cycles.join(", "));
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
    check(
        paths.config_file().exists(),
        "~/.config/loop/config.fnl",
        "run `loop init <TICKET>` to scaffold the toolbox",
    );
    check(
        paths.ext_dir().join("transition-tool.ts").exists(),
        "vendored ext materialized",
        "run `loop init` to write them",
    );
    check(
        paths.machine_file().exists(),
        ".loop/machine.fnl",
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

fn fmt_duration(secs: u64) -> String {
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m{}s", s / 60, s % 60),
        s => format!("{}h{}m", s / 3600, (s % 3600) / 60),
    }
}

fn summarize(e: &loop_core::Event) -> String {
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

fn truncate(s: &str, n: usize) -> String {
    let one_line = s.replace('\n', " ");
    if one_line.chars().count() <= n {
        one_line
    } else {
        let head: String = one_line.chars().take(n - 1).collect();
        format!("{head}…")
    }
}
