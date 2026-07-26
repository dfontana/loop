//! Global configuration — `~/.config/loop/config.fnl` plus the resolved paths
//! the harness works from.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::machine::{Budgets, ModelSpec, TransitionMode};

/// How much prior context a stage's prompt carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ContextMode {
    /// A rolling summary the harness assembles (default).
    #[default]
    Digest,
    /// Every `worker_output` verbatim. Expensive.
    Full,
}

/// The XDG paths loop reads and writes. Split so nothing generated is ever
/// written into the directory a human hand-edits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Paths {
    /// `~/.config/loop` — authored: config.fnl, playbooks/, skills/, machines/, ext/.
    pub config_dir: PathBuf,
    /// `~/.local/state/loop` — generated and disposable.
    pub state_dir: PathBuf,
    /// The project root the run drives (where `.loop/` lives and pi is spawned).
    pub project_dir: PathBuf,
}

impl Paths {
    /// XDG defaults, honoring `LOOP_CONFIG_DIR` / `LOOP_STATE_DIR` overrides
    /// (which is how the integration tests get a hermetic environment).
    pub fn discover(project_dir: impl Into<PathBuf>) -> Self {
        let config_dir = std::env::var_os("LOOP_CONFIG_DIR")
            .map(PathBuf::from)
            .or_else(|| dirs_config().map(|d| d.join("loop")))
            .unwrap_or_else(|| PathBuf::from(".config/loop"));
        let state_dir = std::env::var_os("LOOP_STATE_DIR")
            .map(PathBuf::from)
            .or_else(|| dirs_state().map(|d| d.join("loop")))
            .unwrap_or_else(|| PathBuf::from(".local/state/loop"));
        Self {
            config_dir,
            state_dir,
            project_dir: project_dir.into(),
        }
    }

    // ── authored ──────────────────────────────────────────────────────────
    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.fnl")
    }
    pub fn toolbox_playbooks(&self) -> PathBuf {
        self.config_dir.join("playbooks")
    }
    pub fn toolbox_skills(&self) -> PathBuf {
        self.config_dir.join("skills")
    }
    /// `~/.config/loop/mcp.json`, staged into the agent dir for the `mcp`
    /// pi-extension.
    pub fn toolbox_mcp(&self) -> PathBuf {
        self.config_dir.join("mcp.json")
    }
    pub fn toolbox_machines(&self) -> PathBuf {
        self.config_dir.join("machines")
    }
    pub fn ext_dir(&self) -> PathBuf {
        self.config_dir.join("ext")
    }

    // ── generated ─────────────────────────────────────────────────────────
    /// Exported as `PI_AGENT_DIR` for every spawn.
    pub fn agent_dir(&self) -> PathBuf {
        self.state_dir.join("agent-dir")
    }
    /// Rendered playbooks and entry messages for one ticket's spawns.
    pub fn render_dir(&self, ticket: &str) -> PathBuf {
        self.state_dir.join("render").join(sanitize(ticket))
    }

    // ── per-ticket, in the project ────────────────────────────────────────
    pub fn loop_dir(&self) -> PathBuf {
        self.project_dir.join(".loop")
    }
    pub fn machine_file(&self) -> PathBuf {
        self.loop_dir().join("machine.fnl")
    }
    pub fn local_playbooks(&self) -> PathBuf {
        self.loop_dir().join("playbooks")
    }
    pub fn local_skills(&self) -> PathBuf {
        self.loop_dir().join("skills")
    }
    pub fn ledger_file(&self) -> PathBuf {
        self.loop_dir().join("ledger.jsonl")
    }
    pub fn artifacts_dir(&self) -> PathBuf {
        self.loop_dir().join("artifacts")
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

fn dirs_config() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| home().map(|h| h.join(".config")))
}

fn dirs_state() -> Option<PathBuf> {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| home().map(|h| h.join(".local/state")))
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

/// The contents of `config.fnl`, with defaults applied.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    pub provider: String,
    /// Default Worker model when a state doesn't specify one.
    pub worker: ModelSpec,
    pub judge: ModelSpec,
    pub navigator: ModelSpec,
    pub navigator_max_invocations: u32,

    /// Skills loaded into every stage, before the machine's and the state's.
    pub default_skills: Vec<String>,
    /// Installed pi-extension package names activated per spawn
    /// (`mcp`, `review-model-selector`).
    pub pi_extensions: Vec<String>,

    pub budgets: Budgets,
    pub context: ContextMode,
    /// How many recent transitions the digest includes verbatim.
    pub digest_last_n: usize,
    pub transition_mode: TransitionMode,

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
            pi_extensions: vec!["mcp".into(), "review-model-selector".into()],
            budgets: Budgets {
                usd: Some(15.0),
                wallclock_s: Some(7200),
                max_transitions: Some(60),
            },
            context: ContextMode::Digest,
            digest_last_n: 8,
            transition_mode: TransitionMode::Constrained,
            pi_bin: std::env::var("LOOP_PI_BIN").unwrap_or_else(|_| "pi".into()),
            paths,
        }
    }
}
