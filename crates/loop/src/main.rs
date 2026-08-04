//! `loop` — a local, ticket-level agent orchestrator.
//!
//! The CLI is the thin wiring layer: it resolves paths, loads the machine
//! through the Fennel VM, stages the toolbox, and hands concrete
//! implementations to the engine. Every actual decision lives in the `engine`
//! module.
//!
//! Everything below the argument parsing is in the library half of this crate
//! (`src/lib.rs`), which is also what the integration tests link against.

use clap::{Parser, Subcommand};
use r#loop::commands;
use r#loop::core::Paths;

#[derive(Parser)]
#[command(
    name = "loop",
    version,
    about = "A local, ticket-level agent orchestrator"
)]
struct Cli {
    /// Project directory (default: the current directory).
    #[arg(long, short = 'C', global = true)]
    dir: Option<std::path::PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Scaffold ./.loop/ — the machine, its prose, and its playbooks.
    Init {
        /// Ticket id, e.g. PROJ-1487.
        ticket: String,
        /// Copy an existing `.loop/`-shaped directory instead of the built-in
        /// template. This is how a toolbox works now: keep one somewhere, and
        /// start each ticket from it.
        #[arg(long, value_name = "DIR")]
        from: Option<std::path::PathBuf>,
    },
    /// Lint the machine: reachability, dangling references, guard sanity.
    Validate,
    /// Show what a run would resolve to, without spawning anything.
    Preview {
        /// Detail one state instead of summarizing the whole machine.
        state: Option<String>,
    },
    /// Render the machine as a mermaid state diagram, on stdout.
    Diagram,
    /// Drive the machine to a terminal.
    Run {
        /// Stop after this many transitions, on top of the machine's budget.
        #[arg(long)]
        max_transitions: Option<u32>,
        /// Echo each pi spawn's stderr as it runs.
        #[arg(long, short = 'v')]
        verbose: bool,
    },
    /// Pretty-print the folded ledger: where the run is and how it got there.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Show recent ledger events, or the complete ledger as JSONL.
    Logs {
        /// Number of recent events to show in human mode.
        #[arg(short = 'n', default_value_t = 20, conflicts_with = "raw")]
        n: usize,
        /// Write the complete repaired ledger as JSONL.
        #[arg(long, conflicts_with = "n")]
        raw: bool,
    },
    /// Explain the recorded run: every attempt, the evidence behind it, and why it ended.
    Recap,
    /// Continue an interrupted run from the folded resume point.
    Resume {
        #[arg(long)]
        max_transitions: Option<u32>,
        /// Echo each pi spawn's stderr as it runs.
        #[arg(long, short = 'v')]
        verbose: bool,
    },
    /// List every recorded Worker attempt: time, state, cycle, attempt, outcome, session id, evidence.
    Sessions {
        /// Only list attempts at exactly this state, e.g. `implement`.
        state: Option<String>,
    },
    /// Reopen a Worker's pi session by id — `loop sessions` prints the ids.
    Session {
        /// The session id to reopen, from `loop sessions`. With `--latest` this
        /// is a state filter instead, e.g. `implement`.
        #[arg(value_name = "ID")]
        id: Option<String>,
        /// Reopen the newest recorded attempt rather than naming an id.
        #[arg(long)]
        latest: bool,
    },
    /// Check the environment: pi on PATH, machine present.
    Doctor,
}

fn main() {
    if let Err(e) = try_main() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn try_main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let project_dir = match cli.dir {
        Some(d) => d,
        None => std::env::current_dir()?,
    };
    let paths = Paths::discover(project_dir);

    match cli.command {
        Command::Init { ticket, from } => commands::init(paths, &ticket, from.as_deref()),
        Command::Validate => commands::validate(paths),
        Command::Preview { state } => commands::preview(paths, state),
        Command::Diagram => commands::diagram(paths),
        Command::Run {
            max_transitions,
            verbose,
        } => commands::run(paths, max_transitions, false, verbose),
        Command::Resume {
            max_transitions,
            verbose,
        } => commands::run(paths, max_transitions, true, verbose),
        Command::Status { json } => commands::status(paths, json),
        Command::Logs { n, raw } => commands::logs(paths, n, raw),
        Command::Recap => commands::recap(paths),
        Command::Sessions { state } => commands::sessions(paths, state),
        Command::Session { id, latest } => commands::session(paths, id, latest),
        Command::Doctor => commands::doctor(paths),
    }
}
