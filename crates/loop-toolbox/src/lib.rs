//! The toolbox: playbook resolution, template rendering, and everything that
//! has to be on disk before a `pi` spawn can work.
//!
//! See docs/04-toolbox.md. Two kinds of reusable thing — **playbooks** (a
//! stage's prompt) and **tools** (scoped-tools YAML / MCP servers) — plus the
//! staging step that turns `~/.config/loop/tools/*.yaml` into the single
//! `scoped-tools.yaml` the installed extension reads.

use std::path::{Path, PathBuf};

use loop_core::{Config, Context, CoreError, IoContext, ModelChoice, PlaybookRef, Result};

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
        match r {
            PlaybookRef::Inline(prompt) => playbook::parse("inline", prompt, None),

            PlaybookRef::Path(p) => {
                let full = if p.is_absolute() {
                    p.clone()
                } else {
                    machine_dir.join(p)
                };
                match std::fs::read_to_string(&full) {
                    Ok(src) => {
                        let name = p.file_stem().and_then(|s| s.to_str()).unwrap_or("playbook");
                        playbook::parse(name, &src, Some(full))
                    }
                    Err(_) => Err(CoreError::Unresolved {
                        kind: "playbook",
                        name: p.display().to_string(),
                        searched: vec![full],
                    }),
                }
            }

            PlaybookRef::Named(name) => {
                // Mirrors `Paths::local_playbooks()` shape, but rooted at the
                // machine's own directory (which the caller passes in) rather
                // than `self.config.paths.project_dir` — the two may differ,
                // e.g. under test.
                let local = machine_dir.join("playbooks").join(format!("{name}.md"));
                let toolbox = self
                    .config
                    .paths
                    .toolbox_playbooks()
                    .join(format!("{name}.md"));

                for candidate in [&local, &toolbox] {
                    if let Ok(src) = std::fs::read_to_string(candidate) {
                        return playbook::parse(name, &src, Some(candidate.clone()));
                    }
                }

                Err(CoreError::Unresolved {
                    kind: "playbook",
                    name: name.clone(),
                    searched: vec![local, toolbox],
                })
            }
        }
    }

    /// Merge `~/.config/loop/tools/*.yaml` into
    /// `~/.local/state/loop/agent-dir/scoped-tools.yaml`, copy `mcp.json`, and
    /// stage optional `tools/bin/` helpers alongside them. Returns the directory
    /// to export as `PI_AGENT_DIR`.
    ///
    /// On a same-named tool in two files, the alphabetically later file wins
    /// and a warning is collected — silently dropping a tool is how you get a
    /// stage that mysteriously can't build.
    pub fn stage_agent_dir(&self) -> Result<(PathBuf, Vec<String>)> {
        let agent_dir = self.config.paths.agent_dir();
        std::fs::create_dir_all(&agent_dir)
            .io_ctx(format!("creating agent dir {}", agent_dir.display()))?;

        let tools_dir = self.config.paths.toolbox_tools();
        let dest = agent_dir.join("scoped-tools.yaml");
        let (_names, warnings) = scoped::merge_tools(&tools_dir, &dest)?;
        scoped::stage_mcp(&tools_dir, &agent_dir)?;
        stage_tool_helpers(&tools_dir, &agent_dir)?;

        Ok((agent_dir, warnings))
    }

    /// Write loop's three vendored pi extensions into `~/.config/loop/ext/`
    /// if absent or stale, and return their paths. They are `include_str!`ed
    /// into the binary, so a fresh install needs no separate fetch.
    pub fn materialize_ext(&self) -> Result<ExtPaths> {
        let ext_dir = self.config.paths.ext_dir();
        std::fs::create_dir_all(&ext_dir)
            .io_ctx(format!("creating ext dir {}", ext_dir.display()))?;

        let transition =
            write_if_stale(&ext_dir.join("transition-tool.ts"), ext::TRANSITION_TOOL_TS)?;
        let verdict = write_if_stale(&ext_dir.join("verdict-tool.ts"), ext::VERDICT_TOOL_TS)?;
        let choose = write_if_stale(&ext_dir.join("choose-tool.ts"), ext::CHOOSE_TOOL_TS)?;

        Ok(ExtPaths {
            transition,
            verdict,
            choose,
        })
    }

    /// Render a playbook body with the context namespace and write it to
    /// `~/.local/state/loop/render/<ticket>/<state>-<cycle>-<attempt>.md`,
    /// returning the path for `--append-system-prompt @path`.
    pub fn write_rendered(&self, ctx: &Context, body: &str, suffix: &str) -> Result<PathBuf> {
        let vars = ctx.to_map();
        let rendered = render::substitute(body, &vars);

        let dir = self.config.paths.render_dir(&ctx.ticket_id);
        std::fs::create_dir_all(&dir).io_ctx(format!("creating render dir {}", dir.display()))?;

        let filename = format!("{}-{}-{}-{}.md", ctx.state, ctx.cycle, ctx.attempt, suffix);
        let path = dir.join(filename);
        std::fs::write(&path, rendered).io_ctx(format!("writing {}", path.display()))?;

        Ok(path)
    }

    pub fn config(&self) -> &Config {
        self.config
    }
}

/// Copy optional executable helpers from `tools/bin/` into the staged agent
/// directory. Scoped-tool commands can then invoke them through `$PI_AGENT_DIR`
/// without assuming a particular XDG configuration path.
fn stage_tool_helpers(tools_dir: &Path, agent_dir: &Path) -> Result<()> {
    let source = tools_dir.join("bin");
    if !source.exists() {
        return Ok(());
    }

    let destination = agent_dir.join("bin");
    if destination.exists() {
        std::fs::remove_dir_all(&destination)
            .io_ctx(format!("clearing staged helpers {}", destination.display()))?;
    }
    copy_tree(&source, &destination)
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    std::fs::create_dir_all(destination).io_ctx(format!(
        "creating staged helper directory {}",
        destination.display()
    ))?;
    for entry in std::fs::read_dir(source)
        .io_ctx(format!("reading helper directory {}", source.display()))?
    {
        let entry = entry.io_ctx(format!("reading helper entry in {}", source.display()))?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if entry
            .file_type()
            .io_ctx(format!("reading helper type {}", source_path.display()))?
            .is_dir()
        {
            copy_tree(&source_path, &destination_path)?;
        } else {
            std::fs::copy(&source_path, &destination_path).io_ctx(format!(
                "staging helper {} to {}",
                source_path.display(),
                destination_path.display()
            ))?;
        }
    }
    Ok(())
}

/// Write `content` to `path` only if the file is absent or its content hash
/// differs from `content`'s — never touching a file that's already current.
fn write_if_stale(path: &Path, content: &str) -> Result<PathBuf> {
    let write_needed = match std::fs::read(path) {
        Ok(existing) => sha256_hex(&existing) != sha256_hex(content.as_bytes()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
        Err(e) => return Err(CoreError::io(format!("reading {}", path.display()), e)),
    };
    if write_needed {
        std::fs::write(path, content).io_ctx(format!("writing {}", path.display()))?;
    }
    Ok(path.to_path_buf())
}

/// Hex sha256 of a byte slice, used by `materialize_ext`'s staleness check.
pub(crate) fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
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

#[cfg(test)]
mod tests {
    use super::*;
    use loop_core::Paths;
    use tempfile::tempdir;

    fn test_config(config_dir: &Path, state_dir: &Path, project_dir: &Path) -> Config {
        let paths = Paths {
            config_dir: config_dir.to_path_buf(),
            state_dir: state_dir.to_path_buf(),
            project_dir: project_dir.to_path_buf(),
        };
        Config::defaults(paths)
    }

    #[test]
    fn resolve_playbook_local_wins_over_toolbox() {
        let config_dir = tempdir().unwrap();
        let state_dir = tempdir().unwrap();
        let project_dir = tempdir().unwrap();
        let config = test_config(config_dir.path(), state_dir.path(), project_dir.path());
        let tb = Toolbox::new(&config);

        let toolbox_playbooks = config.paths.toolbox_playbooks();
        std::fs::create_dir_all(&toolbox_playbooks).unwrap();
        std::fs::write(
            toolbox_playbooks.join("qa.md"),
            "---\nname: qa\n---\ntoolbox version\n",
        )
        .unwrap();

        let machine_dir = project_dir.path().join(".loop");
        let local_playbooks = machine_dir.join("playbooks");
        std::fs::create_dir_all(&local_playbooks).unwrap();
        std::fs::write(
            local_playbooks.join("qa.md"),
            "---\nname: qa\n---\nlocal version\n",
        )
        .unwrap();

        let resolved = tb
            .resolve_playbook(&PlaybookRef::Named("qa".into()), &machine_dir)
            .unwrap();
        assert_eq!(resolved.body, "local version\n");
        assert_eq!(resolved.path, Some(local_playbooks.join("qa.md")));
    }

    #[test]
    fn resolve_playbook_falls_back_to_toolbox_when_no_local() {
        let config_dir = tempdir().unwrap();
        let state_dir = tempdir().unwrap();
        let project_dir = tempdir().unwrap();
        let config = test_config(config_dir.path(), state_dir.path(), project_dir.path());
        let tb = Toolbox::new(&config);

        let toolbox_playbooks = config.paths.toolbox_playbooks();
        std::fs::create_dir_all(&toolbox_playbooks).unwrap();
        std::fs::write(
            toolbox_playbooks.join("review.md"),
            "---\nname: review\n---\ntoolbox version\n",
        )
        .unwrap();

        let machine_dir = project_dir.path().join(".loop");
        std::fs::create_dir_all(&machine_dir).unwrap();

        let resolved = tb
            .resolve_playbook(&PlaybookRef::Named("review".into()), &machine_dir)
            .unwrap();
        assert_eq!(resolved.body, "toolbox version\n");
        assert_eq!(resolved.path, Some(toolbox_playbooks.join("review.md")));
    }

    #[test]
    fn resolve_playbook_exact_path() {
        let config_dir = tempdir().unwrap();
        let state_dir = tempdir().unwrap();
        let project_dir = tempdir().unwrap();
        let config = test_config(config_dir.path(), state_dir.path(), project_dir.path());
        let tb = Toolbox::new(&config);

        let machine_dir = project_dir.path().join(".loop");
        std::fs::create_dir_all(&machine_dir).unwrap();
        std::fs::write(
            machine_dir.join("validate-contract.md"),
            "---\nname: validate-contract\n---\nbespoke prompt\n",
        )
        .unwrap();

        let resolved = tb
            .resolve_playbook(
                &PlaybookRef::Path(PathBuf::from("validate-contract.md")),
                &machine_dir,
            )
            .unwrap();
        assert_eq!(resolved.body, "bespoke prompt\n");
    }

    #[test]
    fn resolve_playbook_inline() {
        let config_dir = tempdir().unwrap();
        let state_dir = tempdir().unwrap();
        let project_dir = tempdir().unwrap();
        let config = test_config(config_dir.path(), state_dir.path(), project_dir.path());
        let tb = Toolbox::new(&config);

        let resolved = tb
            .resolve_playbook(
                &PlaybookRef::Inline("do the one-off thing".into()),
                &project_dir.path().join(".loop"),
            )
            .unwrap();
        assert_eq!(resolved.body, "do the one-off thing");
        assert!(resolved.path.is_none());
    }

    #[test]
    fn resolve_playbook_miss_lists_every_searched_path() {
        let config_dir = tempdir().unwrap();
        let state_dir = tempdir().unwrap();
        let project_dir = tempdir().unwrap();
        let config = test_config(config_dir.path(), state_dir.path(), project_dir.path());
        let tb = Toolbox::new(&config);

        let machine_dir = project_dir.path().join(".loop");
        std::fs::create_dir_all(&machine_dir).unwrap();

        let err = tb
            .resolve_playbook(&PlaybookRef::Named("missing".into()), &machine_dir)
            .unwrap_err();
        match err {
            CoreError::Unresolved {
                kind,
                name,
                searched,
            } => {
                assert_eq!(kind, "playbook");
                assert_eq!(name, "missing");
                assert_eq!(searched.len(), 2);
                assert_eq!(searched[0], machine_dir.join("playbooks/missing.md"));
                assert_eq!(
                    searched[1],
                    config.paths.toolbox_playbooks().join("missing.md")
                );
            }
            other => panic!("expected Unresolved, got {other:?}"),
        }
    }

    #[test]
    fn materialize_ext_writes_rewrites_and_noops() {
        let config_dir = tempdir().unwrap();
        let state_dir = tempdir().unwrap();
        let project_dir = tempdir().unwrap();
        let config = test_config(config_dir.path(), state_dir.path(), project_dir.path());
        let tb = Toolbox::new(&config);

        // Absent -> written.
        let paths = tb.materialize_ext().unwrap();
        let written = std::fs::read_to_string(&paths.transition).unwrap();
        assert_eq!(written, ext::TRANSITION_TOOL_TS);
        let mtime_after_write = std::fs::metadata(&paths.transition)
            .unwrap()
            .modified()
            .unwrap();

        // Current -> no-op: re-running with unchanged content must not touch
        // the file at all, verified by mtime staying bit-identical (content
        // equality alone wouldn't prove a write didn't happen).
        tb.materialize_ext().unwrap();
        let mtime_after_noop = std::fs::metadata(&paths.transition)
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(mtime_after_write, mtime_after_noop);
        let unchanged = std::fs::read_to_string(&paths.transition).unwrap();
        assert_eq!(unchanged, ext::TRANSITION_TOOL_TS);

        // Stale -> rewritten back to the vendored content.
        std::fs::write(&paths.transition, "stale-content").unwrap();
        tb.materialize_ext().unwrap();
        let rewritten = std::fs::read_to_string(&paths.transition).unwrap();
        assert_eq!(rewritten, ext::TRANSITION_TOOL_TS);
    }

    #[test]
    fn write_rendered_puts_files_under_state_dir_not_config_dir() {
        let config_dir = tempdir().unwrap();
        let state_dir = tempdir().unwrap();
        let project_dir = tempdir().unwrap();
        let config = test_config(config_dir.path(), state_dir.path(), project_dir.path());
        let tb = Toolbox::new(&config);

        let ctx = Context {
            ticket_id: "PROJ-1".into(),
            state: "implement".into(),
            cycle: 2,
            attempt: 1,
            ..Default::default()
        };
        let path = tb
            .write_rendered(&ctx, "hello $TICKET_ID", "system")
            .unwrap();

        assert!(path.starts_with(state_dir.path()));
        assert!(!path.starts_with(config_dir.path()));
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "hello PROJ-1");
    }

    #[test]
    fn stage_agent_dir_reports_collision_warnings() {
        let config_dir = tempdir().unwrap();
        let state_dir = tempdir().unwrap();
        let project_dir = tempdir().unwrap();
        let config = test_config(config_dir.path(), state_dir.path(), project_dir.path());
        let tb = Toolbox::new(&config);

        let tools_dir = config.paths.toolbox_tools();
        std::fs::create_dir_all(&tools_dir).unwrap();
        std::fs::write(
            tools_dir.join("a.yaml"),
            "shared:\n  description: from a\n  commandTemplate: echo a\n",
        )
        .unwrap();
        std::fs::write(
            tools_dir.join("b.yaml"),
            "shared:\n  description: from b\n  commandTemplate: echo b\n",
        )
        .unwrap();
        let helpers = tools_dir.join("bin");
        std::fs::create_dir_all(&helpers).unwrap();
        std::fs::write(helpers.join("classify.sh"), "#!/bin/sh\necho helper\n").unwrap();

        let (agent_dir, warnings) = tb.stage_agent_dir().unwrap();
        assert_eq!(agent_dir, config.paths.agent_dir());
        assert_eq!(warnings.len(), 1);
        assert!(agent_dir.join("scoped-tools.yaml").is_file());
        assert_eq!(
            std::fs::read_to_string(agent_dir.join("bin/classify.sh")).unwrap(),
            "#!/bin/sh\necho helper\n"
        );
    }

    /// Smoke test against the real `examples/toolbox` fixtures: every shipped
    /// playbook must parse, and merging every shipped `tools/*.yaml` must not
    /// warn or fail. This is what actually exercises the format the docs
    /// promise, on top of the synthetic per-case unit tests above.
    #[test]
    fn real_example_toolbox_playbooks_and_tools_all_load_cleanly() {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let examples = manifest_dir.join("../../examples/toolbox");
        if !examples.is_dir() {
            // Keep this test from becoming a spurious hermetic-test failure
            // if the examples tree is ever relocated.
            return;
        }

        let playbooks_dir = examples.join("playbooks");
        let mut parsed_any = false;
        for entry in std::fs::read_dir(&playbooks_dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let src = std::fs::read_to_string(&path).unwrap();
            let name = path.file_stem().unwrap().to_str().unwrap();
            let pb = playbook::parse(name, &src, Some(path.clone()))
                .unwrap_or_else(|e| panic!("{} failed to parse: {e}", path.display()));
            assert!(!pb.name.is_empty(), "{} has no name", path.display());
            parsed_any = true;
        }
        assert!(parsed_any, "expected at least one example playbook");

        let config_dir = tempdir().unwrap();
        let dest = config_dir.path().join("scoped-tools.yaml");
        let (names, warnings) = scoped::merge_tools(&examples.join("tools"), &dest).unwrap();
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
        assert!(names.contains(&"spark_build".to_string()));
        assert!(names.contains(&"staging_deploy".to_string()));
        assert!(names.contains(&"ci_status".to_string()));

        let servers = scoped::stage_mcp(&examples.join("tools"), config_dir.path()).unwrap();
        assert!(servers.contains(&"linear".to_string()));
        assert!(servers.contains(&"warehouse".to_string()));
    }
}
