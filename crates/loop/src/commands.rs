//! The twelve subcommands.

use std::io::Write as _;
use std::path::Path;

use anyhow::{Context as _, Result, anyhow, bail};

use crate::core::text::brief;
use crate::core::{Floor, Machine, Paths};
use crate::engine::{Diagnostic, Engine, Severity};
use crate::fennel::FennelVm;
use crate::ledger::{self, ArtifactStore, Ledger};
use crate::output::{fmt_status, fmt_totals, summarize};
use crate::report;
use crate::runner::PiRunner;
use crate::sessions::{self, Candidate};
use crate::stage::{CliStage, Resolver};
use crate::toolbox;

// Templates shipped in the binary, so a fresh install needs no fetch.
const STANDARD_TICKET: &str = include_str!("../templates/machines/standard-ticket.fnl");
const TASK_MD: &str = include_str!("../templates/task.md");
const PLAN_MD: &str = include_str!("../templates/plan.md");
const STAGE_PROMPTS: &[(&str, &str)] = &[
    (
        "implement.md",
        include_str!("../templates/stage-prompts/implement.md"),
    ),
    (
        "review.md",
        include_str!("../templates/stage-prompts/review.md"),
    ),
    ("qa.md", include_str!("../templates/stage-prompts/qa.md")),
    (
        "open-pr.md",
        include_str!("../templates/stage-prompts/open-pr.md"),
    ),
];

/// Bundled skills. Separate from [`STAGE_PROMPTS`] because they land in a
/// different directory and are reached a different way: a stage prompt is bound
/// to one state and always in its system prompt, a skill is offered to whatever
/// states name it and loaded only if the model elects to.
///
/// `debug-transient` used to ship here as a stage prompt, which it never was —
/// no bundled state named it, and a `:skills ["debug-transient"]` could not
/// find it, because skills resolve under `skills/` and it was sitting in
/// `stage-prompts/`.
const SKILLS: &[(&str, &str)] = &[(
    "debug-transient.md",
    include_str!("../templates/skills/debug-transient.md"),
)];

/// What `loop init` has written, so it can print it.
///
/// `claim` is the whole of scaffolding: **never overwrite**, create parents,
/// and record what actually landed. Every file `init` writes goes through it,
/// whether it is rendered from a template ([`Scaffold::place`]) or copied off
/// disk ([`Scaffold::place_copy`]) — [`Scaffold::copy_tree`] used to carry its
/// own don't-overwrite rule and its own `create_dir_all`, so the module had two
/// implementations of one invariant and five call sites of the other. What
/// differs between the two writers is only *how the bytes arrive*, which is why
/// that is the closure and everything else is here.
#[derive(Default)]
struct Scaffold {
    created: Vec<String>,
}

impl Scaffold {
    /// The invariant every writer shares: never overwrite, create parents,
    /// record what landed. `write` runs only once the path is known to be
    /// free, and only its success puts the path in [`Scaffold::created`].
    fn claim(&mut self, path: &Path, write: impl FnOnce(&Path) -> Result<()>) -> Result<()> {
        if path.exists() {
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        write(path)?;
        self.created.push(path.display().to_string());
        Ok(())
    }

    /// Write `content` to `path` unless something is already there.
    fn place(&mut self, path: &Path, content: impl AsRef<[u8]>) -> Result<()> {
        self.claim(path, |p| {
            std::fs::write(p, content).with_context(|| format!("writing {}", p.display()))
        })
    }

    /// Copy `src` onto `path` unless something is already there.
    ///
    /// `std::fs::copy` rather than read-then-[`Scaffold::place`], because it
    /// carries the mode across: a copied `skills/` tree ships `.sh` files that
    /// stages and `:check` commands invoke directly, and a 755 script that
    /// lands as 644 fails with `EACCES` at the first stage that runs it. It
    /// also streams, so `--from` on a ticket with a large ledger or artifact
    /// doesn't hold the biggest file in memory.
    fn place_copy(&mut self, src: &Path, path: &Path) -> Result<()> {
        self.claim(path, |p| {
            std::fs::copy(src, p)
                .map(|_| ())
                .with_context(|| format!("copying {} to {}", src.display(), p.display()))
        })
    }

    /// The same, with `$TICKET` substituted — a plain text replacement, not the
    /// `$VAR` render engine a run uses.
    fn place_for(&mut self, path: &Path, content: &str, ticket: &str) -> Result<()> {
        self.place(path, place_ticket(content, ticket))
    }

    /// Copy one directory tree into another, never overwriting. Names in `skip`
    /// are passed over at the **top level only** — they name what a run leaves
    /// in `.loop/`, which is meaningful only at its root.
    fn copy_tree(&mut self, from: &Path, to: &Path, skip: &[&str]) -> Result<()> {
        for entry in std::fs::read_dir(from)
            .with_context(|| format!("reading template directory {}", from.display()))?
        {
            let entry = entry?;
            if skip.contains(&entry.file_name().to_string_lossy().as_ref()) {
                continue;
            }
            let src = entry.path();
            let dst = to.join(entry.file_name());
            if src.is_dir() {
                std::fs::create_dir_all(&dst)?;
                self.copy_tree(&src, &dst, &[])?;
            } else {
                self.place_copy(&src, &dst)?;
            }
        }
        Ok(())
    }
}

/// What a *run* leaves in `.loop/`, as opposed to what defines the ticket.
///
/// `--from` is advertised as "a `.loop/` you already like" (README), and the
/// one you like is a ticket you finished — so its ledger is there, and copying
/// it made the new ticket start life owning a completed run: `loop run` refused
/// with "already has a run", and `loop status` reported the *source* ticket's
/// outcome. `init` documents that it "does not create `artifacts/`,
/// `ledger.jsonl`, or `run/`" (skills/loop-authoring/references/cli.md); copying them was that
/// promise being broken by the other branch.
const RUN_ARTIFACTS: &[&str] = &["ledger.jsonl", "run", "artifacts"];

/// Stamp this ticket's id onto a machine, whichever shape the source is in.
///
/// A bundled template still holds the literal `$TICKET`, so a text replacement
/// is enough. A `--from` source produced by an earlier `loop init` does **not**:
/// its own id was substituted in when it was created, and the replacement
/// silently did nothing — leaving `:ticket "PROJ-1"` on ticket PROJ-99, which
/// then named every session id, status line and recap header for the new run.
/// So the value itself is rewritten when the placeholder is already gone.
fn place_ticket(body: &str, ticket: &str) -> String {
    if body.contains("$TICKET") {
        return body.replace("$TICKET", ticket);
    }
    // Line-oriented, so that `:ticket` inside a comment cannot be mistaken for
    // the key: the real one opens the machine table, as `{:ticket "..."` or
    // `:ticket "..."`, and a Fennel comment starts with `;`.
    let mut out = Vec::with_capacity(body.lines().count());
    let mut done = false;
    for line in body.lines() {
        let code = line.trim_start().trim_start_matches('{');
        match (done, code.starts_with(":ticket")) {
            (false, true) => {
                done = true;
                out.push(replace_first_string(line, ticket));
            }
            _ => out.push(line.to_string()),
        }
    }
    let mut joined = out.join("\n");
    if body.ends_with('\n') {
        joined.push('\n');
    }
    joined
}

/// Swap the contents of the first double-quoted string on `line`, leaving the
/// indentation, the key and any trailing comment exactly where they were.
fn replace_first_string(line: &str, value: &str) -> String {
    let Some(open) = line.find('"') else {
        return line.to_string();
    };
    let rest = &line[open + 1..];
    let Some(close) = rest.find('"') else {
        return line.to_string();
    };
    format!("{}{}{}", &line[..=open], value, &rest[close..])
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
    let mut sc = Scaffold::default();

    match from {
        // Reuse is a copy, not a resolution path. What you started from is
        // recorded in the ticket rather than looked up at run time, so editing
        // the source later cannot change a run already in flight.
        Some(src) => {
            let src = crate::core::config::expand_tilde(src);
            if !src.is_dir() {
                bail!("--from {} is not a directory", src.display());
            }
            sc.copy_tree(&src, &loop_dir, RUN_ARTIFACTS)?;
            if !machine_file.exists() {
                bail!(
                    "{} has no machine.fnl — --from wants a directory shaped like .loop/",
                    src.display()
                );
            }
            // The template's own ticket id is whoever's it was; this one is ours.
            let body = std::fs::read_to_string(&machine_file)?;
            std::fs::write(&machine_file, place_ticket(&body, ticket))?;
        }
        None => {
            sc.place_for(&machine_file, STANDARD_TICKET, ticket)?;
            for (dir, files) in [
                (paths.stage_prompts(), STAGE_PROMPTS),
                (paths.skills(), SKILLS),
            ] {
                for (name, body) in files {
                    sc.place(&dir.join(name), body)?;
                }
            }
        }
    }

    for (name, body) in [("task.md", TASK_MD), ("plan.md", PLAN_MD)] {
        sc.place_for(&loop_dir.join(name), body, ticket)?;
    }
    std::fs::create_dir_all(paths.stage_prompts())?;
    std::fs::create_dir_all(paths.skills())?;
    // `.loop/` is meant to be committed — it is the record of what drove the
    // ticket — but the rendered prompts and handoff files under `run/` are
    // regenerated every stage and would be pure churn in a diff.
    sc.place(&loop_dir.join(".gitignore"), "run/\n")?;

    for c in &sc.created {
        println!("  created {c}");
    }
    println!("\ninitialized {} for {ticket}", loop_dir.display());
    println!("  1. write .loop/task.md and .loop/plan.md");
    println!("  2. hack .loop/machine.fnl into the shape this ticket needs");
    println!("  3. loop validate");
    println!("  4. loop run");
    Ok(())
}

/// Load the machine. Shared by every command that needs the graph.
///
/// The VM comes back with it: a caller that only wants the IR can drop it, but
/// it has to outlive the load itself.
fn load(paths: &Paths) -> Result<(FennelVm, Machine)> {
    let vm = FennelVm::new()?;
    let machine_file = paths.machine_file();
    if !machine_file.exists() {
        bail!(
            "no machine at {} — run `loop init <TICKET>` first",
            machine_file.display()
        );
    }
    let machine = vm.load_machine(&machine_file, &Floor::default())?;
    Ok((vm, machine))
}

/// [`load`], for a caller holding the bytes already — see
/// [`crate::fennel::FennelVm::load_machine_source`].
fn load_source(paths: &Paths, source: &str) -> Result<(FennelVm, Machine)> {
    let vm = FennelVm::new()?;
    let machine = vm.load_machine_source(&paths.machine_file(), source, &Floor::default())?;
    Ok((vm, machine))
}

/// Fold a ledger against the machine's real loop heads when the machine loads,
/// and machine-agnostically when it does not.
///
/// `status` and `recap` both need this and both used to spell it out: the bare
/// `fold` treats every state as a loop head, which turns a `done#1` into a
/// reported cycle beside a genuine `implement#2`. Both commands also have to
/// keep working when the machine is missing or mid-edit — often exactly when
/// they are wanted — so a load failure costs the distinction, not the command.
fn fold_against(machine: Option<&Machine>, events: &[crate::core::Event]) -> crate::core::RunState {
    match machine {
        Some(m) => m.fold(events),
        None => crate::core::fold(events),
    }
}

/// Lint a loaded machine against the ticket directory. `validate` and
/// `preview` share it, so preview reports exactly the problems `validate`
/// would — there is no weaker preview-only linter.
fn diagnose(machine: &Machine) -> Vec<Diagnostic> {
    crate::engine::validate(
        machine,
        &|r| toolbox::resolve_stage_prompt(r, &machine.dir).is_ok(),
        &|name| toolbox::resolve_skill(name, &machine.dir).is_ok(),
    )
}

/// What a read-only command says about a ticket nothing has run in yet. One
/// spelling, because `status` and `logs` are the same answer to the same
/// question and a second wording would read as a different condition.
const NO_RUN: &str = "no run yet — `loop run` starts one";

/// How many events `status` shows under `recent:`. A screenful — `loop logs
/// -n` is the knob, and `loop recap` is the whole history.
const STATUS_RECENT: usize = 12;

/// The last `n` events, oldest-first, in the one line `status` and `logs` both
/// print. Only the indent differs between them, so only the indent is the
/// caller's: the tail walk itself was written out twice.
fn recent(events: &[crate::core::Event], n: usize) -> Vec<String> {
    let mut lines: Vec<String> = events
        .iter()
        .rev()
        .take(n)
        .map(|e| format!("{}  {}", e.ts, summarize(e)))
        .collect();
    lines.reverse();
    lines
}

/// `Ok` when nothing is an error, and a bail naming the count when something
/// is — the ending `validate` and `preview` share, since both print the
/// diagnostics and then have to exit non-zero on the same condition. `tail` is
/// what each adds about its own command.
fn bail_on_errors(diagnostics: &[Diagnostic], tail: &str) -> Result<()> {
    let errors = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count();
    if errors > 0 {
        bail!("{errors} error(s){tail}");
    }
    Ok(())
}

pub fn validate(paths: Paths) -> Result<()> {
    let (_vm, machine) = load(&paths)?;
    let diagnostics = diagnose(&machine);

    for d in &diagnostics {
        println!("{d}");
    }
    if diagnostics.is_empty() {
        println!(
            "{} — {} states, {} transitions, no problems found",
            machine.ticket,
            machine.states.len(),
            machine.transitions.len()
        );
    }
    bail_on_errors(&diagnostics, "")
}

/// Print the machine as mermaid. Bare — no fences, no prose — so it pipes:
/// `loop diagram > machine.mmd`. Unlike `validate` this doesn't touch the
/// toolbox, since drawing the graph needs nothing off the filesystem beyond
/// the machine file itself; a machine with a dangling stage prompt still draws.
pub fn diagram(paths: Paths) -> Result<()> {
    let (_vm, machine) = load(&paths)?;
    print!("{}", crate::engine::mermaid(&machine));
    Ok(())
}

/// `loop preview` — what this machine will do, answered before anything is
/// spawned.
///
/// Read-only and deterministic by construction: it resolves through
/// `Resolver`, the same code `build_stage` runs, but stops short of every
/// write that stage building does. No ledger or artifact directory is
/// created, and nothing lands in `.loop/run/` —
/// the render a state preview shows is built in memory and printed.
pub fn preview(paths: Paths, state: Option<String>) -> Result<()> {
    let (_vm, machine) = load(&paths)?;

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

    let resolver = Resolver::new(&machine, &paths);
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
    let diagnostics = diagnose(&machine);
    print!("{}", report::validation(&diagnostics));
    bail_on_errors(&diagnostics, " — this machine will not run as previewed")
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
    let (_vm, mut machine) = load(&paths)?;
    if let Some(max) = max_transitions {
        machine.budgets = machine.budgets.tighten(crate::core::Budgets {
            usd: None,
            wallclock_s: None,
            max_transitions: Some(max),
        });
    }

    let ledger_path = paths.ledger_file();
    // One handle, consulted then kept. Opening repairs a torn tail and reads
    // the run clock, so throwing the first one away to ask a single question
    // paid for all of that twice.
    let mut ledger = Ledger::open(&ledger_path)?;
    match (ledger.started(), resuming) {
        (false, true) => bail!("nothing to resume: {} is empty", ledger_path.display()),
        (true, false) => bail!(
            "{} already has a run — use `loop resume`, or delete it to start over",
            ledger_path.display()
        ),
        _ => {}
    }

    // Read before the engine borrows the ledger: the time budget bounds the
    // run, so a resume starts its clock at what the interrupted session
    // already burned rather than at zero.
    let elapsed_offset_s = ledger.elapsed_offset_s();
    let artifacts = ArtifactStore::new(paths.artifacts_dir(), &paths.project_dir);
    let runner = PiRunner::new().verbose(verbose);
    let stage = CliStage::new(&machine, &paths, ledger_path.clone());

    let mut engine = Engine {
        machine: &machine,
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
        "\n{:?} — {} after {}",
        outcome.status,
        outcome.terminal_state.as_deref().unwrap_or("(no terminal)"),
        fmt_totals(&outcome.totals),
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

/// The five keys `loop status --json` emits, from a folded run.
///
/// One definition. The empty-ledger branch below used to hand-write the same
/// five keys with nulls and zeroes beside it — which is exactly
/// `RunState::default()`, so the copy was a second answer to a question the
/// fold already answers, fifteen lines from the first.
fn status_json(rs: &crate::core::RunState) -> serde_json::Value {
    serde_json::json!({
        "current": rs.current,
        "status": rs.status,
        "cycles": rs.cycles,
        "totals": rs.totals,
        "navigator_invocations": rs.navigator_invocations,
    })
}

pub fn status(paths: Paths, json: bool) -> Result<()> {
    let events = ledger::events(paths.ledger_file())?;
    // An empty ledger still has to answer in the mode it was asked in — this
    // used to print prose in both, so `loop status --json` on a fresh project
    // handed a parser a sentence. It returns early rather than falling
    // through: the path below loads the machine, and spinning up a Lua VM and
    // a 288 KB Fennel compile to fold nothing is not free.
    if events.is_empty() {
        match json {
            true => println!(
                "{}",
                serde_json::to_string_pretty(&status_json(&Default::default()))?
            ),
            false => println!("{NO_RUN}"),
        }
        return Ok(());
    }
    let machine = load(&paths).ok().map(|(_vm, m)| m);
    let folded = fold_against(machine.as_ref(), &events);

    if json {
        println!("{}", serde_json::to_string_pretty(&status_json(&folded))?);
        return Ok(());
    }

    println!("{}", fmt_status(&folded));
    println!("  {}", fmt_totals(&folded.totals));
    if machine.is_some() && !folded.cycles.is_empty() {
        println!("  cycles: {}", report::fmt_cycles(&folded));
    }
    println!("\nrecent:");
    for line in recent(&events, STATUS_RECENT) {
        println!("  {line}");
    }
    Ok(())
}

pub fn logs(paths: Paths, n: usize, raw: bool) -> Result<()> {
    // `--raw` hands back the bytes on disk, which is the one thing a decoded
    // event list cannot reproduce — so it wants the handle, not the events.
    if raw {
        let ledger = Ledger::open(paths.ledger_file())?;
        std::io::stdout().write_all(&ledger.read_raw()?)?;
        return Ok(());
    }

    let events = ledger::events(paths.ledger_file())?;
    if events.is_empty() {
        println!("{NO_RUN}");
        return Ok(());
    }

    for line in recent(&events, n) {
        println!("{line}");
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
    let events = ledger::events(&ledger_path)?;
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

    let recorded_hash = crate::core::run_started(&events).map(|s| s.machine_hash);
    // Hash what is on disk *before* deciding to load it. The hash is the whole
    // question — a mismatch reports the recorded run and discards the machine
    // anyway — and a mismatch is the common case, since editing the machine
    // after a run is the usual reason to read a recap. Loading costs a Lua VM
    // and a 288 KB Fennel compile; hashing costs a read.
    //
    // No recorded hash means nothing on disk could ever be proven to be the
    // machine that ran, so the VM is never started for one either. The source
    // is kept, not just its hash: on a match it is exactly what the load
    // needs, and re-reading the same file to hand it the same bytes was the
    // one cost this early-hash was supposed to avoid.
    let on_disk = recorded_hash
        .and_then(|_| std::fs::read_to_string(paths.machine_file()).ok())
        .map(|src| (crate::core::machine_hash(&src), src));

    // The two hashes settle it, so this is one match producing the answer —
    // not a match to decide whether to load, then a second match over the same
    // two hashes *and* the result to say what the load meant. (Which in turn
    // replaced a four-arm match wrapping a three-arm one, with the same
    // `eprintln!` written out twice inside it.)
    let machine = match (recorded_hash, &on_disk) {
        (Some(recorded), Some((on_disk, source))) if recorded == on_disk => {
            match load_source(&paths, source).ok().map(|(_vm, m)| m) {
                // Provably the machine that ran.
                Some(m) => report::Provenance::Matches(Box::new(m)),
                // It hashes right but will not load — a machine mid-edit in a
                // way that changes nothing the hash sees is impossible, so this
                // is a broken toolbox rather than a changed machine.
                None => report::Provenance::NotLoaded,
            }
        }
        (Some(recorded), Some((on_disk, _))) => {
            // stderr, so `loop recap > run-recap.md` still produces a clean
            // file. The report repeats the mismatch in its own summary, so the
            // warning is a nudge rather than the only place it appears.
            eprintln!(
                "warning: {} has changed since this run started (ledger {recorded}, on disk \
                 {on_disk}) — the recap reports only what the ledger recorded",
                paths.machine_file().display(),
            );
            report::Provenance::Changed {
                current: on_disk.clone(),
            }
        }
        _ => report::Provenance::NotLoaded,
    };

    let folded = fold_against(machine.machine(), &events);

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
    let events = ledger::events(&ledger_path)?;
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
    let ledger_path = paths.ledger_file();
    let events = ledger::events(&ledger_path)?;
    let candidates = sessions::candidates(&events);
    let ticket = crate::core::run_started(&events).map(|s| s.ticket);

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
        line.push_str(&format!(" — {}", brief(&detail, 72)));
    }
    println!("{line}");

    if !chosen.is_complete() {
        eprintln!(
            "warning: no worker_output for this attempt — the session may still be active, or the \
             spawn crashed"
        );
        if let Some(err) = chosen.errors.first() {
            eprintln!("warning: the ledger recorded: {}", brief(err, 120));
        }
    }

    launch_pi_session(&paths, chosen.session_id)
}

/// Hand the terminal to pi with the recorded session id, in the project
/// directory, with stdin/stdout/stderr inherited untouched.
///
/// The argv itself is [`crate::runner::command::session_command`]'s, beside the
/// three role builders — this used to assemble its own, which quietly made the
/// claim that all pi-specific code is one function untrue by a factor of four.
fn launch_pi_session(paths: &Paths, session_id: &str) -> Result<()> {
    let pi = crate::core::pi_bin();
    let mut cmd = crate::runner::command::session_command(&pi, session_id, &paths.project_dir);

    let status = cmd.status().with_context(|| {
        format!("launching `{pi} --session {session_id}` — install pi, or set LOOP_PI_BIN")
    })?;
    if !status.success() {
        bail!(
            "`{pi} --session {session_id}` exited {}",
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

    let pi = crate::core::pi_bin();
    check(
        on_path(&pi),
        &format!("`{pi}` on PATH"),
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

/// Whether `bin` names something runnable — an explicit path that exists, or a
/// bare name found on `$PATH`.
fn on_path(bin: &str) -> bool {
    if bin.contains('/') {
        return Path::new(bin).exists();
    }
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(bin).is_file()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `--from` copies a whole `.loop/` tree, and the interesting thing in one
    /// is `skills/`: the bundled examples ship `.sh` files that stages and
    /// `:check` commands invoke directly, so a 755 script arriving as 644 fails
    /// with `EACCES` at the first stage that runs it — a broken ticket, with
    /// nothing in the ledger explaining why. Copying through a read-then-write
    /// loses the bit silently, which is why this is asserted rather than left
    /// to `std::fs::copy`'s documentation.
    #[cfg(unix)]
    #[test]
    fn copy_tree_keeps_the_executable_bit() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let (src, dst) = (dir.path().join("src"), dir.path().join("dst"));
        std::fs::create_dir_all(src.join("skills/build")).unwrap();
        let script = src.join("skills/build/build.sh");
        std::fs::write(&script, "#!/bin/sh\necho hi\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(src.join("machine.fnl"), "{}").unwrap();

        let mut sc = Scaffold::default();
        sc.copy_tree(&src, &dst, &[]).unwrap();

        let copied = dst.join("skills/build/build.sh");
        assert_eq!(
            std::fs::read_to_string(&copied).unwrap(),
            "#!/bin/sh\necho hi\n"
        );
        assert!(
            copied.metadata().unwrap().permissions().mode() & 0o111 != 0,
            "a copied script has to stay runnable"
        );
        assert_eq!(sc.created.len(), 2, "both files recorded: {:?}", sc.created);
    }

    /// The invariant `claim` centralizes, exercised through the copying half:
    /// `--from` must never clobber something already in the destination.
    #[test]
    fn copy_tree_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let (src, dst) = (dir.path().join("src"), dir.path().join("dst"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(src.join("task.md"), "from the template").unwrap();
        std::fs::write(dst.join("task.md"), "mine, already here").unwrap();

        let mut sc = Scaffold::default();
        sc.copy_tree(&src, &dst, &[]).unwrap();

        assert_eq!(
            std::fs::read_to_string(dst.join("task.md")).unwrap(),
            "mine, already here"
        );
        assert!(
            sc.created.is_empty(),
            "nothing landed, so nothing is claimed"
        );
    }

    /// The workflow the README advertises — `--from` a `.loop/` you already
    /// like — hands over a directory belonging to a ticket you *finished*. Its
    /// ledger must not come along: `loop run` refuses to start on a ledger that
    /// already holds a run, so copying it made the new ticket dead on arrival,
    /// and `loop status` answered for the source ticket instead.
    #[test]
    fn from_a_finished_ticket_leaves_the_run_behind() {
        let dir = tempfile::tempdir().unwrap();
        let (src, dst) = (dir.path().join("src"), dir.path().join("proj"));
        std::fs::create_dir_all(src.join("stage-prompts")).unwrap();
        std::fs::create_dir_all(src.join("run")).unwrap();
        std::fs::create_dir_all(src.join("artifacts")).unwrap();
        std::fs::write(src.join("machine.fnl"), "{:ticket \"PROJ-1\"}\n").unwrap();
        std::fs::write(src.join("stage-prompts/implement.md"), "do it").unwrap();
        std::fs::write(src.join("ledger.jsonl"), "{\"type\":\"run_started\"}\n").unwrap();
        std::fs::write(src.join("run/implement.md"), "rendered").unwrap();
        std::fs::write(src.join("artifacts/diff.patch"), "a diff").unwrap();

        init(Paths::new(&dst), "PROJ-99", Some(&src)).unwrap();

        let loop_dir = dst.join(".loop");
        assert!(!loop_dir.join("ledger.jsonl").exists(), "no stale ledger");
        assert!(!loop_dir.join("run").exists(), "no rendered prompts");
        assert!(!loop_dir.join("artifacts").exists(), "no stale artifacts");
        // What does define the ticket still crosses over.
        assert_eq!(
            std::fs::read_to_string(loop_dir.join("stage-prompts/implement.md")).unwrap(),
            "do it"
        );
    }

    /// A `--from` source produced by an earlier `init` has no `$TICKET` left in
    /// it, so the substitution silently did nothing and the new ticket inherited
    /// the old id — in `:ticket`, and from there in every session id, status
    /// line and recap header the run went on to write.
    #[test]
    fn from_an_initialized_source_takes_the_new_ticket_id() {
        let dir = tempfile::tempdir().unwrap();
        let (src, dst) = (dir.path().join("src"), dir.path().join("proj"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("machine.fnl"),
            ";; a machine for :ticket handling\n{:ticket \"PROJ-1\" ; the old one\n :entry \"a\"}\n",
        )
        .unwrap();

        init(Paths::new(&dst), "PROJ-99", Some(&src)).unwrap();

        let body = std::fs::read_to_string(dst.join(".loop/machine.fnl")).unwrap();
        assert!(body.contains("{:ticket \"PROJ-99\""), "rewritten: {body}");
        assert!(!body.contains("PROJ-1\""), "no trace of the source: {body}");
        // The rest of the line, and the rest of the file, are untouched.
        assert!(body.contains("; the old one"), "comment kept: {body}");
        assert!(
            body.starts_with(";; a machine for :ticket handling\n"),
            "a comment mentioning the key is not the key: {body}"
        );
    }

    /// The other source shape: a hand-written template that still says
    /// `$TICKET`. That path worked before and has to keep working.
    #[test]
    fn a_template_placeholder_is_still_substituted() {
        assert_eq!(
            place_ticket("{:ticket \"$TICKET\" :task \"$TICKET.md\"}", "PROJ-7"),
            "{:ticket \"PROJ-7\" :task \"PROJ-7.md\"}"
        );
    }

    /// Only the first `:ticket` is the machine's, and a file with none is left
    /// alone rather than guessed at — the schema rejects it downstream, which is
    /// a better error than a silent edit.
    #[test]
    fn place_ticket_touches_only_the_key_and_nothing_else() {
        assert_eq!(
            place_ticket("{:ticket \"a\"\n :states {:x {:ticket-ish \"b\"}}}", "N"),
            "{:ticket \"N\"\n :states {:x {:ticket-ish \"b\"}}}"
        );
        assert_eq!(place_ticket("{:entry \"a\"}\n", "N"), "{:entry \"a\"}\n");
        assert_eq!(place_ticket("", "N"), "");
    }
}
