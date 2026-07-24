//! The toolbox: playbook resolution, template rendering, and everything that
//! has to be on disk before a `pi` spawn can work.
//!
//! See docs/04-toolbox.md. Two kinds of reusable thing — **playbooks** (a
//! stage's prompt) and **tools** (scoped-tools YAML / MCP servers) — plus the
//! staging step that turns `~/.config/loop/tools/*.yaml` into the single
//! `scoped-tools.yaml` the installed extension reads.
//!
//! TASK T3 implements this crate.

use std::path::{Path, PathBuf};

use loop_core::{Config, Context, ModelChoice, PlaybookRef, Result};

pub mod ext;
pub mod playbook;
pub mod render;
pub mod scoped;

pub use ext::ExtPaths;
pub use playbook::ResolvedPlaybook;

pub struct Toolbox<'a> {
    config: &'a Config,
}

impl<'a> Toolbox<'a> {
    pub fn new(config: &'a Config) -> Self {
        Self { config }
    }

    /// Resolve a playbook reference **local-first**:
    /// 1. `./.loop/playbooks/<name>.md`
    /// 2. `~/.config/loop/playbooks/<name>.md`
    ///
    /// A value containing `/` is an exact path (relative to `machine_dir`); an
    /// inline prompt short-circuits. A miss is
    /// [`loop_core::CoreError::Unresolved`] listing every path searched — that
    /// message is what makes `loop validate` useful.
    pub fn resolve_playbook(
        &self,
        r: &PlaybookRef,
        machine_dir: &Path,
    ) -> Result<ResolvedPlaybook> {
        let _ = (r, machine_dir);
        todo!("T3")
    }

    /// Merge `~/.config/loop/tools/*.yaml` into
    /// `~/.local/state/loop/agent-dir/scoped-tools.yaml` and copy `mcp.json`
    /// alongside it. Returns the directory to export as `PI_AGENT_DIR`.
    ///
    /// On a same-named tool in two files, the alphabetically later file wins
    /// and a warning is collected — silently dropping a tool is how you get a
    /// stage that mysteriously can't build.
    pub fn stage_agent_dir(&self) -> Result<(PathBuf, Vec<String>)> {
        todo!("T3")
    }

    /// Write loop's three vendored pi extensions into `~/.config/loop/ext/`
    /// if absent or stale, and return their paths. They are `include_str!`ed
    /// into the binary, so a fresh install needs no separate fetch.
    pub fn materialize_ext(&self) -> Result<ExtPaths> {
        todo!("T3")
    }

    /// Render a playbook body with the context namespace and write it to
    /// `~/.local/state/loop/render/<ticket>/<state>-<cycle>-<attempt>.md`,
    /// returning the path for `--append-system-prompt @path`.
    pub fn write_rendered(&self, ctx: &Context, body: &str, suffix: &str) -> Result<PathBuf> {
        let _ = (ctx, body, suffix);
        todo!("T3")
    }

    pub fn config(&self) -> &Config {
        self.config
    }
}

/// The model/thinking a playbook's frontmatter declares — the layer between a
/// state's overrides and the machine defaults.
pub fn frontmatter_model(pb: &ResolvedPlaybook) -> ModelChoice {
    ModelChoice {
        provider: None,
        model: pb.model.clone(),
        thinking: pb.thinking,
    }
}
