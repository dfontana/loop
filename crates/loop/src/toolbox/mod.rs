//! Name resolution, template rendering, and everything that has to be on disk
//! before a `pi` spawn can work.
//!
//! See docs/03-customizing.md. Two kinds of authored thing, and the difference
//! is what reaches the model, not what the file looks like — both are markdown
//! with YAML frontmatter:
//!
//! - A **stage prompt** is bound to one state and is *always* in that stage's
//!   context: loop reads it, substitutes `$VAR`s into it, and hands the result
//!   to `--append-system-prompt`. It is the only channel the task, the plan,
//!   and the ledger digest have into a stage.
//! - A **skill** is *offered*: loop passes its path to `--skill` and never
//!   opens it, so pi shows the model its name and description and the model
//!   decides whether to load the body. It can carry scripts; a stage prompt
//!   is one file of prose.
//!
//! That asymmetry is why they are not one thing. Anything a stage must be told
//! cannot be a skill, because "offered" is not "told"; anything carrying run
//! state cannot be a skill, because loop never renders one.
//!
//! MCP servers are deliberately not a third kind: loop names servers out of the
//! user's own config and never ships one.
//!
//! Everything resolves inside the ticket directory. There is no second root to
//! fall back to and no precedence order to remember — a name either names a
//! file in `.loop/` or it is an error that says which path it looked at.

use std::path::{Path, PathBuf};

use crate::core::{Config, Context, CoreError, IoContext, ModelChoice, Result, StagePromptRef};

pub mod render;
pub mod skill;
pub mod stage_prompt;

pub use stage_prompt::ResolvedStagePrompt;

/// How a name is looked up — the only thing the two kinds ever disagreed on.
pub enum Lookup<'a> {
    /// An exact path, because the authored value contained a `/`. Absolute
    /// as-is, otherwise relative to the machine file's own directory.
    Exact(&'a Path),
    /// A bare name: these candidates, in order, first usable one wins.
    Named(&'a [PathBuf]),
}

/// The one resolver, for both kinds of authored name.
///
/// There were two, one per kind, and they agreed on everything that matters:
/// the same `/`-means-exact-path escape hatch relative to the same directory,
/// the same first-usable-candidate-wins loop, and the same
/// [`CoreError::Unresolved`] listing every path that was tried — which is the
/// message that makes `loop validate` worth running. Only the candidate list
/// and what counts as a usable hit ever differed, so those are the parameters
/// and the rest is shared by construction rather than by two people
/// remembering to keep it that way.
///
/// `usable` applies to [`Lookup::Named`] only. An exact path is judged by
/// `exists` alone, as both copies did: an author who wrote out a path has said
/// what they meant, and second-guessing the shape of it there would make the
/// escape hatch less of one.
pub fn resolve_name(
    kind: &'static str,
    name: &str,
    machine_dir: &Path,
    lookup: Lookup<'_>,
    usable: impl Fn(&Path) -> bool,
) -> Result<PathBuf> {
    match lookup {
        Lookup::Exact(p) => {
            let full = if p.is_absolute() {
                p.to_path_buf()
            } else {
                machine_dir.join(p)
            };
            if full.exists() {
                Ok(full)
            } else {
                Err(CoreError::Unresolved {
                    kind,
                    name: name.to_string(),
                    searched: vec![full],
                })
            }
        }
        Lookup::Named(candidates) => {
            for candidate in candidates {
                if usable(candidate) {
                    return Ok(candidate.clone());
                }
            }
            Err(CoreError::Unresolved {
                kind,
                name: name.to_string(),
                searched: candidates.to_vec(),
            })
        }
    }
}

pub struct Toolbox<'a> {
    config: &'a Config,
}

impl<'a> Toolbox<'a> {
    pub fn new(config: &'a Config) -> Self {
        Self { config }
    }

    /// Resolve a stage prompt reference against `<machine_dir>/stage-prompts/`
    /// and read it.
    ///
    /// An inline prompt short-circuits — it has no path to resolve. Everything
    /// else goes through [`resolve_name`], so the escape hatch and the error
    /// are the skill resolver's, and then the file is read and parsed, which is
    /// the half only a stage prompt has: loop never opens a skill.
    pub fn resolve_stage_prompt(
        &self,
        r: &StagePromptRef,
        machine_dir: &Path,
    ) -> Result<ResolvedStagePrompt> {
        // Hoisted so the `Lookup::Named` below can borrow it. Rooted at the
        // machine file's own directory, which the caller passes in rather than
        // reading off `config.paths`: resolution follows the file that named
        // the stage prompt. The subdirectory name is shared with `Paths` so
        // `init` and resolution cannot disagree about the layout.
        let candidates = match r {
            StagePromptRef::Named(n) => stage_prompt::candidates(
                n,
                &machine_dir.join(crate::core::config::STAGE_PROMPTS_DIR),
            ),
            _ => Vec::new(),
        };

        // `name` is the display name the parsed prompt carries; `reported` is
        // what an error echoes back, and has to be the authored value rather
        // than the file stem — an unresolved `:stage-prompt "vendor/qa.md"`
        // reported as `qa` names a file the machine does not mention.
        let (name, reported, lookup) = match r {
            StagePromptRef::Inline(prompt) => return stage_prompt::parse("inline", prompt, None),
            StagePromptRef::Path(p) => (
                p.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("stage-prompt")
                    .to_string(),
                p.display().to_string(),
                Lookup::Exact(p),
            ),
            StagePromptRef::Named(n) => (n.clone(), n.clone(), Lookup::Named(&candidates)),
        };

        let path = resolve_name("stage prompt", &reported, machine_dir, lookup, |p| {
            p.is_file()
        })?;

        // Separate from resolution on purpose. The old code folded the two
        // together with `read_to_string`, so a file that exists but cannot be
        // read — a bad mode, a dangling symlink — reported as "could not
        // resolve", sending the author to look for a missing file that was
        // right there.
        let src = std::fs::read_to_string(&path)
            .io_ctx(format!("reading stage prompt {}", path.display()))?;
        stage_prompt::parse(&name, &src, Some(path))
    }

    /// Resolve one skill name to the path pi's `--skill` should load, in
    /// `<machine_dir>/skills/`.
    pub fn resolve_skill(&self, name: &str, machine_dir: &Path) -> Result<PathBuf> {
        skill::resolve(name, machine_dir)
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

        let path = self
            .config
            .paths
            .render_file(&ctx.state, ctx.cycle, ctx.attempt, suffix);
        std::fs::write(&path, rendered).io_ctx(format!("writing {}", path.display()))?;

        Ok(path)
    }
}

/// The model/thinking a stage prompt's frontmatter declares — the layer between a
/// state's overrides and the machine defaults.
pub fn frontmatter_model(pb: &ResolvedStagePrompt) -> ModelChoice {
    ModelChoice {
        provider: None,
        model: pb.model.clone(),
        thinking: pb.thinking,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Paths;
    use tempfile::tempdir;

    fn test_config(project_dir: &Path) -> Config {
        Config::defaults(Paths {
            project_dir: project_dir.to_path_buf(),
        })
    }

    #[test]
    fn resolve_stage_prompt_by_name() {
        let project_dir = tempdir().unwrap();
        let config = test_config(project_dir.path());
        let tb = Toolbox::new(&config);

        let machine_dir = project_dir.path().join(".loop");
        let stage_prompts = machine_dir.join("stage-prompts");
        std::fs::create_dir_all(&stage_prompts).unwrap();
        std::fs::write(
            stage_prompts.join("qa.md"),
            "---\nname: qa\n---\nthe qa prompt\n",
        )
        .unwrap();

        let resolved = tb
            .resolve_stage_prompt(&StagePromptRef::Named("qa".into()), &machine_dir)
            .unwrap();
        assert_eq!(resolved.body, "the qa prompt\n");
        assert_eq!(resolved.path, Some(stage_prompts.join("qa.md")));
    }

    #[test]
    fn resolve_stage_prompt_exact_path() {
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
            .resolve_stage_prompt(
                &StagePromptRef::Path(PathBuf::from("validate-contract.md")),
                &machine_dir,
            )
            .unwrap();
        assert_eq!(resolved.body, "bespoke prompt\n");
    }

    #[test]
    fn resolve_stage_prompt_inline() {
        let project_dir = tempdir().unwrap();
        let config = test_config(project_dir.path());
        let tb = Toolbox::new(&config);

        let resolved = tb
            .resolve_stage_prompt(
                &StagePromptRef::Inline("do the one-off thing".into()),
                &project_dir.path().join(".loop"),
            )
            .unwrap();
        assert_eq!(resolved.body, "do the one-off thing");
        assert!(resolved.path.is_none());
    }

    #[test]
    fn resolve_stage_prompt_miss_lists_every_searched_path() {
        let project_dir = tempdir().unwrap();
        let config = test_config(project_dir.path());
        let tb = Toolbox::new(&config);

        let machine_dir = project_dir.path().join(".loop");
        std::fs::create_dir_all(&machine_dir).unwrap();

        let err = tb
            .resolve_stage_prompt(&StagePromptRef::Named("missing".into()), &machine_dir)
            .unwrap_err();
        match err {
            CoreError::Unresolved {
                kind,
                name,
                searched,
            } => {
                assert_eq!(kind, "stage prompt");
                assert_eq!(name, "missing");
                assert_eq!(searched, vec![machine_dir.join("stage-prompts/missing.md")]);
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

    /// Smoke test against the real worked example: every shipped stage prompt
    /// must parse and every shipped skill must resolve. This is what actually
    /// exercises the format the docs promise, on top of the synthetic per-case
    /// unit tests above.
    ///
    /// It asserts the tree is there rather than skipping when it isn't. It used
    /// to point at `examples/toolbox` and return early if that was missing —
    /// and `examples/toolbox` had been `examples/proj-1487` for some time, so
    /// the early return fired every run and the whole test passed without
    /// opening a file. A smoke test that silently tests nothing is worse than
    /// no smoke test, because the green tick is load-bearing in review.
    #[test]
    fn real_example_stage_prompts_and_skills_all_load_cleanly() {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let examples = manifest_dir.join("../../examples/proj-1487");
        assert!(
            examples.is_dir(),
            "the worked example moved: {} does not exist. Point this test at \
             the new path rather than letting it skip.",
            examples.display()
        );

        let stage_prompts_dir = examples.join("stage-prompts");
        let mut parsed_any = false;
        for entry in std::fs::read_dir(&stage_prompts_dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let src = std::fs::read_to_string(&path).unwrap();
            let name = path.file_stem().unwrap().to_str().unwrap();
            let pb = stage_prompt::parse(name, &src, Some(path.clone()))
                .unwrap_or_else(|e| panic!("{} failed to parse: {e}", path.display()));
            assert!(!pb.name.is_empty(), "{} has no name", path.display());
            parsed_any = true;
        }
        assert!(parsed_any, "expected at least one example stage prompt");

        // Resolved against the example's own root, the way a run would: a
        // machine's `:skills` names are looked up under `<machine_dir>/skills/`.
        let mut resolved_any = false;
        for entry in std::fs::read_dir(examples.join("skills")).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_str().unwrap().to_string();
            let name = name.strip_suffix(".md").unwrap_or(&name).to_string();
            skill::resolve(&name, &examples)
                .unwrap_or_else(|e| panic!("skill `{name}` did not resolve: {e}"));
            resolved_any = true;
        }
        assert!(resolved_any, "expected at least one example skill");
    }
}
