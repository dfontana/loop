//! `loop` — a local, ticket-level agent orchestrator.
//!
//! The CLI is the thin wiring layer: it resolves paths, loads the machine
//! through the Fennel VM, stages the toolbox, and hands concrete
//! implementations to the engine. Every actual decision lives in `loop-engine`.

use clap::{Parser, Subcommand};
use loop_core::Paths;

mod commands;
mod output;
mod stage;

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
    /// Scaffold ./.loop/ from a machine template, and ~/.config/loop/ on first use.
    Init {
        /// Ticket id, e.g. PROJ-1487.
        ticket: String,
        /// Machine template from ~/.config/loop/machines/.
        #[arg(long, default_value = "standard-ticket")]
        template: String,
    },
    /// Lint the machine: reachability, dangling references, guard sanity.
    Validate,
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
    /// Continue an interrupted run from the folded resume point.
    Resume {
        #[arg(long)]
        max_transitions: Option<u32>,
        /// Echo each pi spawn's stderr as it runs.
        #[arg(long, short = 'v')]
        verbose: bool,
    },
    /// Check the environment: pi on PATH, toolbox staged, machine present.
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
        Command::Init { ticket, template } => commands::init(paths, &ticket, &template),
        Command::Validate => commands::validate(paths),
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
        Command::Doctor => commands::doctor(paths),
    }
}
