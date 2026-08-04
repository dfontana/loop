//! Spawning `pi` and reading its JSON event stream.
//!
//! See docs/02-how-it-works.md. Three roles, three cost profiles: the Worker
//! does the stage's work and ends by writing a handoff file; the Judge
//! independently grades a `criteria`; the Navigator reroutes a blocked worker.
//! All three are `pi --print --mode json` subprocesses whose newline-delimited
//! events this module parses.
//!
//! Nothing here is pi-specific beyond `command.rs`. The roles communicate
//! through a written file and two first-line text contracts, so porting to
//! another headless agent CLI means writing another `*_command` builder and
//! nothing else.
//!
//! `mock-pi` is this module's offline stand-in, which is what lets everything
//! else be tested without an API key.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

use crate::core::{
    AgentRunner, Choice, CoreError, JudgeSpec, NavigatorSpec, Result, Verdict, WorkerResult,
    WorkerSpec, with_stderr_tail,
};

pub mod check;
pub mod command;
pub mod reply;
pub mod stream;

pub use check::exec_check;
pub use reply::{clear_handoff, parse_choice, parse_verdict, read_handoff};
pub use stream::{StreamOutcome, parse_stream};

/// How many trailing stderr lines to keep from a spawn. A pi crash puts its
/// diagnosis in the last few lines; keeping a bounded tail means a process
/// that logs a gigabyte still costs one small buffer.
const STDERR_TAIL_LINES: usize = 20;

/// Spawns real `pi` subprocesses.
pub struct PiRunner {
    pi_bin: String,
    /// Echo each spawn's stderr as it arrives, so a human watching `loop run
    /// --verbose` sees the worker working. Off by default and in tests — but
    /// the tail is captured either way, so a crash is diagnosable without it.
    pub verbose: bool,
}

/// One spawn's result: what the stream said, whether it exited clean, and the
/// tail of what it complained about on the way.
struct SpawnOutcome {
    stream: StreamOutcome,
    exit_ok: bool,
    stderr_tail: String,
}

impl Default for PiRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl PiRunner {
    pub fn new() -> Self {
        Self {
            pi_bin: crate::core::pi_bin(),
            verbose: false,
        }
    }

    pub fn verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self
    }

    /// Spawn `cmd`, stream-parse its stdout, and wait for it to exit.
    ///
    /// stdin is closed (these are non-interactive `--print` spawns). stderr is
    /// piped and drained by its own thread — never piped-but-unread, which
    /// would deadlock the moment pi writes enough to fill the pipe buffer, and
    /// never discarded, which is what used to reduce a failed spawn to a bare
    /// non-zero exit code with nothing to debug from.
    fn spawn_and_parse(&self, mut cmd: Command, role: &str) -> Result<SpawnOutcome> {
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| CoreError::agent(role, format!("failed to spawn pi: {e}")))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| CoreError::agent(role, "pi spawn had no stdout pipe"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| CoreError::agent(role, "pi spawn had no stderr pipe"))?;

        // Drained concurrently with stdout: both pipes have to be read while
        // the child is alive, or whichever we ignore fills and blocks it.
        let verbose = self.verbose;
        let drain = std::thread::spawn(move || {
            let mut tail: std::collections::VecDeque<String> = std::collections::VecDeque::new();
            for line in BufReader::new(stderr)
                .lines()
                .map_while(std::io::Result::ok)
            {
                if verbose {
                    eprintln!("{line}");
                }
                if tail.len() == STDERR_TAIL_LINES {
                    tail.pop_front();
                }
                tail.push_back(line);
            }
            tail.into_iter().collect::<Vec<_>>().join("\n")
        });

        // Parsing never fails the run on a bad line (see stream.rs); reading
        // stdout to EOF is exactly what tolerates a crash-truncated stream.
        let stream = parse_stream(BufReader::new(stdout))?;

        let status = child
            .wait()
            .map_err(|e| CoreError::agent(role, format!("failed waiting for pi: {e}")))?;
        let stderr_tail = drain.join().unwrap_or_default();

        Ok(SpawnOutcome {
            stream,
            exit_ok: status.success(),
            stderr_tail,
        })
    }
}

/// How many characters of an off-contract reply to keep in the ledger.
const OFF_CONTRACT_REPLY_CHARS: usize = 400;

impl SpawnOutcome {
    /// The reply, when the spawn both exited clean and answered on contract.
    ///
    /// The Judge and the Navigator have the same shape here — parse the final
    /// message, and treat *anything* else as no answer — so they share the
    /// check rather than each writing `if exit_ok { if let Some(..) { .. } }`.
    fn on_contract<T>(&self, parse: impl FnOnce(&str) -> Option<T>) -> Option<T> {
        self.exit_ok.then(|| parse(&self.stream.summary)).flatten()
    }

    /// Why there was no usable answer, in terms a human reading the ledger can
    /// act on.
    ///
    /// A judge that fails closed and a navigator that escalates both end a
    /// run's forward progress, and "returned no usable verdict" alone says
    /// nothing about whether the model refused, rambled, or answered fine in
    /// the wrong format. So this carries whatever the spawn said — bounded,
    /// because a reply can be long and this lands in an event — and whatever it
    /// complained about on stderr, labelled, because that is the subprocess
    /// talking rather than the harness.
    fn off_contract(&self, message: &str) -> String {
        let reply = self.stream.summary.trim();
        let said = if reply.is_empty() {
            format!("{message} (the spawn produced no text at all)")
        } else {
            let shown = crate::core::text::truncate(reply, OFF_CONTRACT_REPLY_CHARS);
            format!("{message}; it said:\n{shown}")
        };
        with_stderr_tail(said, &self.stderr_tail)
    }
}

impl AgentRunner for PiRunner {
    /// Spawn the worker, stream-parse its output, and read back its handoff.
    ///
    /// Contract details that matter:
    /// - The proposal is read from the file at `spec.handoff_path`, which the
    ///   spawn's prompt and `$LOOP_HANDOFF` both name. Any stale file there is
    ///   removed *before* the spawn, so a proposal can only ever come from the
    ///   attempt that just ran.
    /// - `usage` is summed off every `message_end` event for an assistant
    ///   message.
    /// - A worker that leaves no usable handoff is not an error here: we
    ///   return `proposal: None` and let the engine decide (it re-enters or
    ///   navigates).
    /// - The handoff is read even when the process exited non-zero. A stage
    ///   that wrote its decision and *then* died still decided, and throwing
    ///   that away would re-run work that had finished. The engine ignores it
    ///   on a crash anyway, but that is the engine's policy to set, not this
    ///   layer's to pre-empt.
    /// - `stderr_tail` travels beside them, because on a non-zero exit the
    ///   engine writes an `error` event and drops the summary entirely — the
    ///   tail is the only place a spawn failure can leave a diagnosis.
    fn run_worker(&self, spec: &WorkerSpec) -> Result<WorkerResult> {
        reply::clear_handoff(&spec.handoff_path)?;

        let cmd = command::worker_command(&self.pi_bin, spec);
        let out = self.spawn_and_parse(cmd, "worker")?;

        Ok(WorkerResult {
            summary: out.stream.summary,
            proposal: reply::read_handoff(&spec.handoff_path),
            usage: out.stream.usage,
            exit_ok: out.exit_ok,
            stderr_tail: out.stderr_tail,
        })
    }

    /// `--no-session --no-builtin-tools --no-extensions --no-skills`, and no
    /// tool of its own — so the verdict is the spawn's final message, read
    /// against the `PASS`/`FAIL` first-line contract in
    /// [`command::judge_prompt`].
    ///
    /// A judge that returns nothing usable — a reply that ignores the
    /// contract, or a process that exited non-zero — is a **fail**, not a
    /// pass: an unavailable grader must never wave work through
    /// (docs/05-design-notes.md).
    fn run_judge(&self, spec: &JudgeSpec) -> Result<Verdict> {
        let cmd = command::judge_command(&self.pi_bin, spec);
        let out = self.spawn_and_parse(cmd, "judge")?;

        // The fail-closed rationale is what a human reads when a guard fails
        // for a reason that has nothing to do with the work.
        let (pass, rationale) = out
            .on_contract(reply::parse_verdict)
            .unwrap_or_else(|| (false, out.off_contract("judge returned no usable verdict")));

        Ok(Verdict {
            pass,
            rationale,
            usage: out.stream.usage,
        })
    }

    /// Same isolation as the Judge, and the same shape of contract: the choice
    /// is the first line of the final message, matched against the states it
    /// was offered (see [`command::navigator_prompt`]).
    ///
    /// A navigator that returns nothing usable — an unrecognized first line or
    /// a non-zero exit — escalates rather than stalling the run.
    fn run_navigator(&self, spec: &NavigatorSpec) -> Result<Choice> {
        let cmd = command::navigator_command(&self.pi_bin, spec);
        let out = self.spawn_and_parse(cmd, "navigator")?;
        let choices = command::navigator_choices(spec);

        let (to, entry_prompt) = out
            .on_contract(|reply| reply::parse_choice(reply, &choices))
            .unwrap_or_else(|| {
                (
                    command::ESCALATE.to_string(),
                    Some(out.off_contract(
                        "the navigator spawn produced no usable choice, so the harness escalated",
                    )),
                )
            });

        Ok(Choice {
            to,
            entry_prompt,
            usage: out.stream.usage,
        })
    }
}
