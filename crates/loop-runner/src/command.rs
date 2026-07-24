//! Assembling the `pi` command line for each of the three roles.
//!
//! Confirmed against the installed pi (`pi --help`):
//! `--print`, `--mode json`, `--session-id`, `--provider`, `--model <m>:<thinking>`,
//! `--tools`, `--exclude-tools`, `--no-builtin-tools`, `--no-session`,
//! `-e <path>`, `--append-system-prompt <text|@file>`, then the positional message.

use std::process::Command;

use loop_core::{JudgeSpec, NavigatorSpec, WorkerSpec};

/// Build the Worker command.
///
/// TASK T4. Required environment:
/// - `PI_AGENT_DIR` → the staged agent dir, so `scoped-tools` finds the merged
///   YAML and `mcp` finds `mcp.json`.
/// - `LOOP_REACHABLE` → comma-separated neighbors; the transition tool builds
///   its enum from this.
/// - `LOOP_TRANSITION_MODE` → `constrained` | `open`.
/// - every `spec.env` pair, so a scoped-tool's `valueFromCmd` can read
///   `$TICKET_ID` / `$CYCLE` and key its idempotency on them.
pub fn worker_command(pi_bin: &str, spec: &WorkerSpec) -> Command {
    let _ = (pi_bin, spec);
    todo!("T4")
}

/// TASK T4. The Judge gets no code tools at all — `--no-builtin-tools` plus the
/// single `-e verdict-tool.ts`. That is what makes its verdict independent
/// (docs/07-risks.md #1); do not add `read` "for convenience".
pub fn judge_command(pi_bin: &str, spec: &JudgeSpec) -> Command {
    let _ = (pi_bin, spec);
    todo!("T4")
}

/// TASK T4.
pub fn navigator_command(pi_bin: &str, spec: &NavigatorSpec) -> Command {
    let _ = (pi_bin, spec);
    todo!("T4")
}
