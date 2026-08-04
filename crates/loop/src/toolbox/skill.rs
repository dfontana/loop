//! Resolving a stage's skills to the paths pi's `--skill` takes.
//!
//! A skill is a `SKILL.md` plus whatever scripts sit beside it. loop does not
//! parse or rewrite either — it resolves a name to a path and hands that path
//! to pi, which owns the format. That is the whole difference from a stage
//! prompt, which loop reads, renders, and puts in the system prompt itself.
//!
//! Resolution lives entirely in the ticket directory:
//!
//! 1. `./.loop/skills/<name>/`  (a directory containing `SKILL.md`)
//! 2. `./.loop/skills/<name>.md`
//!
//! The mechanics of getting from a name to one of those — the `/` escape
//! hatch, the first-hit-wins loop, the error listing every path tried — are
//! [`super::resolve_name`], shared with stage prompts. What lives here is the
//! part that is actually about skills: those two candidates, and the rule that
//! a directory has to hold a `SKILL.md` to count.

use std::path::{Path, PathBuf};

use crate::core::Result;
use crate::toolbox::{Lookup, resolve_name};

/// Both places a bare skill name is looked for, in order. Exposed so a miss
/// can report both — that message is what makes `loop validate` useful.
pub fn candidates(name: &str, skills_dir: &Path) -> Vec<PathBuf> {
    vec![skills_dir.join(name), skills_dir.join(format!("{name}.md"))]
}

/// Resolve one skill name to the path pi should load, under
/// `<machine_dir>/skills/`.
///
/// A name containing `/` is an exact path relative to `machine_dir` — the same
/// escape hatch `:stage-prompt` has, because it is now literally the same code.
pub fn resolve(name: &str, machine_dir: &Path) -> Result<PathBuf> {
    let skills_dir = machine_dir.join(crate::core::config::SKILLS_DIR);
    let candidates = candidates(name, &skills_dir);
    // The same predicate `StagePromptRef::parse` uses, so the two kinds agree
    // on what an author meant by a `/`.
    let lookup = if crate::core::names_a_path(name) {
        Lookup::Exact(Path::new(name))
    } else {
        Lookup::Named(&candidates)
    };
    resolve_name("skill", name, machine_dir, lookup, |candidate| {
        // A directory only counts when it actually holds a SKILL.md; an empty
        // `skills/foo/` would otherwise resolve and then load nothing, which
        // looks like the skill silently doing nothing at run time.
        if candidate.is_dir() {
            candidate.join("SKILL.md").is_file()
        } else {
            candidate.is_file()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::CoreError;
    use tempfile::tempdir;

    /// A machine directory with an empty `skills/` in it — what `resolve` is
    /// given, since a stage's `:skills` are looked up relative to the machine
    /// file, not to the skills directory itself.
    fn machine_dir(tmp: &Path) -> PathBuf {
        let dir = tmp.join(".loop");
        std::fs::create_dir_all(dir.join("skills")).unwrap();
        dir
    }

    fn skill_dir(machine: &Path, name: &str, body: &str) -> PathBuf {
        let dir = machine.join("skills").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), body).unwrap();
        dir
    }

    #[test]
    fn a_directory_skill_resolves_to_its_directory() {
        let tmp = tempdir().unwrap();
        let machine = machine_dir(tmp.path());
        let expected = skill_dir(&machine, "deploy", "the deploy skill");

        assert_eq!(resolve("deploy", &machine).unwrap(), expected);
    }

    /// The single-file form: a bare `.md` with no scripts beside it.
    #[test]
    fn a_bare_md_file_resolves_too() {
        let tmp = tempdir().unwrap();
        let machine = machine_dir(tmp.path());
        let flat = machine.join("skills/qa.md");
        std::fs::write(&flat, "---\nname: qa\n---\n").unwrap();

        assert_eq!(resolve("qa", &machine).unwrap(), flat);
    }

    /// The directory form wins when both spellings exist — a `SKILL.md` with
    /// scripts beside it is the richer thing, and silently preferring the bare
    /// file would drop the scripts.
    #[test]
    fn the_directory_form_wins_over_the_bare_file() {
        let tmp = tempdir().unwrap();
        let machine = machine_dir(tmp.path());
        let expected = skill_dir(&machine, "qa", "directory version");
        std::fs::write(machine.join("skills/qa.md"), "bare version").unwrap();

        assert_eq!(resolve("qa", &machine).unwrap(), expected);
    }

    /// An empty `skills/foo/` would resolve and then load nothing — at run
    /// time that is indistinguishable from a skill that does nothing, so it
    /// has to be a miss here instead.
    #[test]
    fn a_directory_without_a_skill_md_does_not_resolve() {
        let tmp = tempdir().unwrap();
        let machine = machine_dir(tmp.path());
        std::fs::create_dir_all(machine.join("skills/hollow")).unwrap();

        assert!(resolve("hollow", &machine).is_err());
    }

    #[test]
    fn a_miss_lists_every_path_it_searched() {
        let tmp = tempdir().unwrap();
        let machine = machine_dir(tmp.path());

        let err = resolve("missing", &machine).unwrap_err();
        match err {
            CoreError::Unresolved {
                kind,
                name,
                searched,
            } => {
                assert_eq!(kind, "skill");
                assert_eq!(name, "missing");
                assert_eq!(searched.len(), 2);
            }
            other => panic!("expected Unresolved, got {other:?}"),
        }
    }

    #[test]
    fn a_name_with_a_slash_is_an_exact_path() {
        let tmp = tempdir().unwrap();
        let machine = machine_dir(tmp.path());
        // Relative to the *machine* dir, mirroring how `:stage-prompt` paths
        // resolve — the shared `resolve_name` is what makes that true.
        let vendored = machine.join("vendor/thing.md");
        std::fs::create_dir_all(vendored.parent().unwrap()).unwrap();
        std::fs::write(&vendored, "x").unwrap();

        assert_eq!(resolve("vendor/thing.md", &machine).unwrap(), vendored);
    }

    /// The escape hatch reaches a stage prompt, which is the one place the two
    /// kinds legitimately overlap: a procedure worth loading into a second
    /// stage as reference does not need to be copied into `skills/`.
    #[test]
    fn a_slash_path_can_name_a_stage_prompt() {
        let tmp = tempdir().unwrap();
        let machine = machine_dir(tmp.path());
        let prompts = machine.join("stage-prompts");
        std::fs::create_dir_all(&prompts).unwrap();
        std::fs::write(prompts.join("review.md"), "the review procedure").unwrap();

        assert_eq!(
            resolve("stage-prompts/review.md", &machine).unwrap(),
            prompts.join("review.md")
        );
    }
}
