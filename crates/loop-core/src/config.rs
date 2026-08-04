//! The paths the harness works from, and the built-in defaults a machine
//! overlays.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::machine::{Budgets, ModelSpec};

/// Everything loop reads or writes, all of it under `<project>/.loop/`.
///
/// There used to be three roots: a toolbox at `~/.config/loop`, the ticket at
/// `<project>/.loop`, and generated renders at `~/.local/state/loop`. Playbooks
/// and skills resolved local-first across the first two, which meant "where
/// does this come from" had two answers, `loop preview` had to report which
/// one won, and editing a toolbox playbook silently changed the next stage of
/// every in-flight ticket.
///
/// One root instead. A ticket directory is now self-contained: committable,
/// reviewable, and `rm -rf`-able in the same breath as the branch it belongs
/// to. Reuse is `loop init --from <dir>`, which copies — so what you started
/// from is recorded in the ticket rather than resolved out from under it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Paths {
    /// The project root the run drives — where `.loop/` lives and pi is spawned.
    pub project_dir: PathBuf,
}

impl Paths {
    pub fn discover(project_dir: impl Into<PathBuf>) -> Self {
        Self {
            project_dir: project_dir.into(),
        }
    }

    /// The ticket directory. Everything below is inside it.
    pub fn loop_dir(&self) -> PathBuf {
        self.project_dir.join(".loop")
    }

    // ── authored ──────────────────────────────────────────────────────────
    pub fn machine_file(&self) -> PathBuf {
        self.loop_dir().join("machine.fnl")
    }
    pub fn playbooks(&self) -> PathBuf {
        self.loop_dir().join("playbooks")
    }
    pub fn skills(&self) -> PathBuf {
        self.loop_dir().join("skills")
    }

    // ── recorded ──────────────────────────────────────────────────────────
    pub fn ledger_file(&self) -> PathBuf {
        self.loop_dir().join("ledger.jsonl")
    }
    pub fn artifacts_dir(&self) -> PathBuf {
        self.loop_dir().join("artifacts")
    }

    // ── generated ─────────────────────────────────────────────────────────
    /// Rendered prompts and handoff files. Derived from the machine and the
    /// ledger, so deleting it costs nothing — which is why `loop init` writes
    /// it into `.gitignore`.
    pub fn run_dir(&self) -> PathBuf {
        self.loop_dir().join("run")
    }

    /// Where a Worker spawn writes its handoff JSON. One file per attempt, so
    /// a retry can never read the previous attempt's proposal — and the
    /// harness deletes it before spawning anyway, belt and braces.
    pub fn handoff_file(&self, state: &str, cycle: u32, attempt: u32) -> PathBuf {
        self.run_dir().join(format!(
            "{}-{cycle}-{attempt}-handoff.json",
            sanitize(state)
        ))
    }
}

/// Replace anything that isn't safe in a path component.
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

/// Expand a leading `~/` against `$HOME`.
pub fn expand_tilde(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    match s.strip_prefix("~/") {
        Some(rest) => home()
            .map(|h| h.join(rest))
            .unwrap_or_else(|| p.to_path_buf()),
        None => p.to_path_buf(),
    }
}

/// The settings a run uses, and where it works.
///
/// This is no longer read from a file. `config.fnl` was a second authored
/// artifact in a second directory whose only job was to hold values a machine
/// could already override — so the values moved into the machine, the file
/// went, and what remains here is the built-in floor a machine overlays plus
/// the two things a machine has no business setting (`paths`, `pi_bin`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    /// The provider every role falls back to. Each role spec below carries its
    /// own resolved `provider`; this is the value they take when their table
    /// does not name one, which is what makes setting it alone switch the
    /// whole toolbox.
    pub provider: String,
    /// Default Worker model when a state doesn't specify one.
    pub worker: ModelSpec,
    pub judge: ModelSpec,
    pub navigator: ModelSpec,
    pub navigator_max_invocations: u32,

    /// Skills loaded into every stage, before the machine's and the state's.
    pub default_skills: Vec<String>,
    /// MCP servers connected in every stage, before the machine's and the
    /// state's. Names out of the user's own `mcp.json` — loop never reads or
    /// writes that file.
    pub default_mcp: Vec<String>,
    /// Installed pi-extension package names activated per spawn
    /// (`mcp`, `review-model-selector`).
    pub pi_extensions: Vec<String>,

    pub budgets: Budgets,
    /// How many recent transitions the digest includes verbatim.
    pub digest_last_n: usize,

    /// The pi executable. `LOOP_PI_BIN` overrides it — that is how the
    /// integration tests point the whole harness at `mock-pi`.
    pub pi_bin: String,

    pub paths: Paths,
}

impl Config {
    /// The defaults a fresh install runs with, before `config.fnl` is read.
    pub fn defaults(paths: Paths) -> Self {
        Self {
            provider: "anthropic".into(),
            worker: ModelSpec {
                provider: "anthropic".into(),
                model: "claude-sonnet-5".into(),
                thinking: crate::machine::Thinking::Medium,
            },
            judge: ModelSpec {
                provider: "anthropic".into(),
                model: "claude-haiku-4-5".into(),
                thinking: crate::machine::Thinking::Low,
            },
            navigator: ModelSpec {
                provider: "anthropic".into(),
                model: "claude-haiku-4-5".into(),
                thinking: crate::machine::Thinking::Low,
            },
            navigator_max_invocations: 5,
            default_skills: Vec::new(),
            default_mcp: Vec::new(),
            pi_extensions: vec!["mcp".into(), "review-model-selector".into()],
            budgets: Budgets {
                usd: Some(15.0),
                wallclock_s: Some(7200),
                max_transitions: Some(60),
            },
            digest_last_n: 8,
            pi_bin: std::env::var("LOOP_PI_BIN").unwrap_or_else(|_| "pi".into()),
            paths,
        }
    }
}
