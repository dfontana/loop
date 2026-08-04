//! Playbook resolution, template rendering, and everything that has to be on
//! disk before a `pi` spawn can work.
//!
//! See docs/03-customizing.md. Two kinds of authored thing — **playbooks** (a
//! stage's prompt) and **skills** (situational know-how plus the scripts that
//! carry it out). MCP servers are deliberately not a third: loop names servers
//! out of the user's own config and never ships one.
//!
//! Everything resolves inside the ticket directory. There is no second root to
//! fall back to and no precedence order to remember — a name either names a
//! file in `.loop/` or it is an error that says which path it looked at.

use std::path::{Path, PathBuf};

use loop_core::{Config, Context, CoreError, IoContext, ModelChoice, PlaybookRef, Result};

pub mod playbook;
pub mod render;
pub mod skill;

pub use playbook::ResolvedPlaybook;

pub struct Toolbox<'a> {
    config: &'a Config,
}

impl<'a> Toolbox<'a> {
    pub fn new(config: &'a Config) -> Self {
        Self { config }
    }

    /// Resolve a playbook reference against `<machine_dir>/playbooks/`.
    ///
    /// A value containing `/` is an exact path (relative to `machine_dir`); an
    /// inline prompt short-circuits. A miss is
    /// [`loop_core::CoreError::Unresolved`] naming the path it looked at —
    /// that message is what makes `loop validate` useful.
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
                // Rooted at the machine's own directory, which the caller
                // passes in rather than reading off `config.paths` — the two
                // differ under `-C` and under test.
                let path = machine_dir.join("playbooks").join(format!("{name}.md"));
                match std::fs::read_to_string(&path) {
                    Ok(src) => playbook::parse(name, &src, Some(path)),
                    Err(_) => Err(CoreError::Unresolved {
                        kind: "playbook",
                        name: name.clone(),
                        searched: vec![path],
                    }),
                }
            }
        }
    }

    /// Resolve one skill name to the path pi's `--skill` should load, in
    /// `<machine_dir>/skills/`.
    pub fn resolve_skill(&self, name: &str, machine_dir: &Path) -> Result<PathBuf> {
        skill::resolve(name, &machine_dir.join("skills"))
    }

    /// Resolve every skill a stage names, in order.
    pub fn resolve_skills(&self, names: &[String], machine_dir: &Path) -> Result<Vec<PathBuf>> {
        names
            .iter()
            .map(|n| self.resolve_skill(n, machine_dir))
            .collect()
    }

    /// Write an already-rendered prompt to
    /// `.loop/run/<state>-<cycle>-<attempt>-<suffix>.md`, returning the path
    /// for `--append-system-prompt <path>`.
    ///
    /// Substitution is the caller's job, and deliberately so: this used to run
    /// [`render::substitute`] itself on a body the caller had *already*
    /// substituted, so any `$NAME` that appeared in a substituted value got
    /// expanded a second time. Nothing depended on the second pass, and a
    /// prompt assembled from harness-owned text (the handoff protocol) must be
    /// able to reach the file without passing through the template engine at
    /// all.
    pub fn write_rendered(&self, ctx: &Context, rendered: &str, suffix: &str) -> Result<PathBuf> {
        let dir = self.config.paths.run_dir();
        std::fs::create_dir_all(&dir).io_ctx(format!("creating run dir {}", dir.display()))?;

        let filename = format!("{}-{}-{}-{}.md", ctx.state, ctx.cycle, ctx.attempt, suffix);
        let path = dir.join(filename);
        std::fs::write(&path, rendered).io_ctx(format!("writing {}", path.display()))?;

        Ok(path)
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

#[cfg(test)]
mod tests {
    use super::*;
    use loop_core::Paths;
    use tempfile::tempdir;

    fn test_config(project_dir: &Path) -> Config {
        Config::defaults(Paths {
            project_dir: project_dir.to_path_buf(),
        })
    }

    #[test]
    fn resolve_playbook_by_name() {
        let project_dir = tempdir().unwrap();
        let config = test_config(project_dir.path());
        let tb = Toolbox::new(&config);

        let machine_dir = project_dir.path().join(".loop");
        let playbooks = machine_dir.join("playbooks");
        std::fs::create_dir_all(&playbooks).unwrap();
        std::fs::write(
            playbooks.join("qa.md"),
            "---\nname: qa\n---\nthe qa prompt\n",
        )
        .unwrap();

        let resolved = tb
            .resolve_playbook(&PlaybookRef::Named("qa".into()), &machine_dir)
            .unwrap();
        assert_eq!(resolved.body, "the qa prompt\n");
        assert_eq!(resolved.path, Some(playbooks.join("qa.md")));
    }

    #[test]
    fn resolve_playbook_exact_path() {
        let project_dir = tempdir().unwrap();
        let config = test_config(project_dir.path());
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
        let project_dir = tempdir().unwrap();
        let config = test_config(project_dir.path());
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
        let project_dir = tempdir().unwrap();
        let config = test_config(project_dir.path());
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
                assert_eq!(searched, vec![machine_dir.join("playbooks/missing.md")]);
            }
            other => panic!("expected Unresolved, got {other:?}"),
        }
    }

    #[test]
    fn write_rendered_puts_files_in_the_run_dir() {
        let project_dir = tempdir().unwrap();
        let config = test_config(project_dir.path());
        let tb = Toolbox::new(&config);

        let ctx = Context {
            ticket_id: "PROJ-1".into(),
            state: "implement".into(),
            cycle: 2,
            attempt: 1,
            ..Default::default()
        };
        // Written verbatim: the `$TICKET_ID` here survives, because rendering
        // happened before this call and a second pass would re-expand values
        // that merely *contain* a `$NAME`.
        let path = tb
            .write_rendered(&ctx, "hello PROJ-1 and $TICKET_ID", "system")
            .unwrap();

        assert!(path.starts_with(config.paths.run_dir()), "got {path:?}");
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "hello PROJ-1 and $TICKET_ID");
    }

    /// Smoke test against the real `examples/toolbox` fixtures: every shipped
    /// playbook must parse and every shipped skill must resolve. This is what
    /// actually exercises the format the docs promise, on top of the synthetic
    /// per-case unit tests above.
    #[test]
    fn real_example_toolbox_playbooks_and_skills_all_load_cleanly() {
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

        let skills_dir = examples.join("skills");
        let mut resolved_any = false;
        for entry in std::fs::read_dir(&skills_dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_str().unwrap().to_string();
            let name = name.strip_suffix(".md").unwrap_or(&name).to_string();
            skill::resolve(&name, &skills_dir)
                .unwrap_or_else(|e| panic!("skill `{name}` did not resolve: {e}"));
            resolved_any = true;
        }
        assert!(resolved_any, "expected at least one example skill");
    }
}
