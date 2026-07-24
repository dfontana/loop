//! `loop` — the CLI. Wires the concrete implementations into the engine.
//!
//! Wave 2 work; the command surface is fixed here so the other crates know what
//! they must support.

use clap::{Parser, Subcommand};

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
    /// Scaffold ./.loop/ (machine.fnl, task.md, plan.md) from a template, and
    /// ~/.config/loop/ on first use.
    Init {
        /// Ticket id, e.g. PROJ-1487.
        ticket: String,
        /// Machine template from ~/.config/loop/machines/.
        #[arg(long, default_value = "standard-ticket")]
        template: String,
    },
    /// Lint the machine: graph reachability, dangling references, guard sanity.
    Validate,
    /// Drive the machine to a terminal.
    Run {
        /// Stop after this many transitions (on top of the machine's budget).
        #[arg(long)]
        max_transitions: Option<u32>,
    },
    /// Pretty-print the folded ledger: where the run is and how it got there.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Continue an interrupted run from the folded resume point.
    Resume,
    /// Check the environment: pi on PATH, extensions installed, toolbox staged.
    Doctor,
}

fn main() -> anyhow::Result<()> {
    let _cli = Cli::parse();
    todo!("wave 2")
}
