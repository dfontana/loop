//! Resolving a stage's skills to the paths pi's `--skill` takes.
//!
//! A skill is a `SKILL.md` plus whatever scripts sit beside it. loop does not
//! parse or rewrite either — it resolves a name to a path and hands that path
//! to pi, which owns the format.
//!
//! Resolution mirrors playbooks, and lives entirely in the ticket directory:
//!
//! 1. `./.loop/skills/<name>/`  (a directory containing `SKILL.md`)
//! 2. `./.loop/skills/<name>.md`

use std::path::{Path, PathBuf};

use loop_core::{CoreError, Result};

/// Both places a bare skill name is looked for, in order. Exposed so a miss
/// can report both — that message is what makes `loop validate` useful.
pub fn candidates(name: &str, skills_dir: &Path) -> Vec<PathBuf> {
    vec![skills_dir.join(name), skills_dir.join(format!("{name}.md"))]
}

/// Resolve one skill name to the path pi should load.
///
/// A name containing `/` is an exact path, taken relative to `skills_dir`'s
/// parent (the machine's directory) when it isn't absolute — the same escape
/// hatch `:playbook` has.
pub fn resolve(name: &str, skills_dir: &Path) -> Result<PathBuf> {
    if name.contains('/') {
        let p = PathBuf::from(name);
        let full = if p.is_absolute() {
            p
        } else {
            skills_dir.parent().unwrap_or(skills_dir).join(&p)
        };
        return if full.exists() {
            Ok(full)
        } else {
            Err(CoreError::Unresolved {
                kind: "skill",
                name: name.to_string(),
                searched: vec![full],
            })
        };
    }

    let searched = candidates(name, skills_dir);
    for candidate in &searched {
        // A directory only counts when it actually holds a SKILL.md; an empty
        // `skills/foo/` would otherwise resolve and then load nothing, which
        // looks like the skill silently doing nothing at run time.
        let usable = if candidate.is_dir() {
            candidate.join("SKILL.md").is_file()
        } else {
            candidate.is_file()
        };
        if usable {
            return Ok(candidate.clone());
        }
    }
    Err(CoreError::Unresolved {
        kind: "skill",
        name: name.to_string(),
        searched,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn skill_dir(root: &Path, name: &str, body: &str) -> PathBuf {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), body).unwrap();
        dir
    }

    #[test]
    fn a_directory_skill_resolves_to_its_directory() {
        let tmp = tempdir().unwrap();
        let skills = tmp.path().join(".loop/skills");
        std::fs::create_dir_all(&skills).unwrap();
        let expected = skill_dir(&skills, "deploy", "the deploy skill");

        assert_eq!(resolve("deploy", &skills).unwrap(), expected);
    }

    /// The single-file form: a bare `.md` with no scripts beside it.
    #[test]
    fn a_bare_md_file_resolves_too() {
        let tmp = tempdir().unwrap();
        let skills = tmp.path().join(".loop/skills");
        std::fs::create_dir_all(&skills).unwrap();
        let flat = skills.join("qa.md");
        std::fs::write(&flat, "---\nname: qa\n---\n").unwrap();

        assert_eq!(resolve("qa", &skills).unwrap(), flat);
    }

    /// The directory form wins when both spellings exist — a `SKILL.md` with
    /// scripts beside it is the richer thing, and silently preferring the bare
    /// file would drop the scripts.
    #[test]
    fn the_directory_form_wins_over_the_bare_file() {
        let tmp = tempdir().unwrap();
        let skills = tmp.path().join(".loop/skills");
        std::fs::create_dir_all(&skills).unwrap();
        let expected = skill_dir(&skills, "qa", "directory version");
        std::fs::write(skills.join("qa.md"), "bare version").unwrap();

        assert_eq!(resolve("qa", &skills).unwrap(), expected);
    }

    /// An empty `skills/foo/` would resolve and then load nothing — at run
    /// time that is indistinguishable from a skill that does nothing, so it
    /// has to be a miss here instead.
    #[test]
    fn a_directory_without_a_skill_md_does_not_resolve() {
        let tmp = tempdir().unwrap();
        let skills = tmp.path().join(".loop/skills");
        std::fs::create_dir_all(skills.join("hollow")).unwrap();

        assert!(resolve("hollow", &skills).is_err());
    }

    #[test]
    fn a_miss_lists_every_path_it_searched() {
        let tmp = tempdir().unwrap();
        let skills = tmp.path().join(".loop/skills");

        let err = resolve("missing", &skills).unwrap_err();
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
        let machine_dir = tmp.path().join(".loop");
        let skills = machine_dir.join("skills");
        std::fs::create_dir_all(&skills).unwrap();
        // Relative to the *machine* dir, mirroring how `:playbook` paths resolve.
        let vendored = machine_dir.join("vendor/thing.md");
        std::fs::create_dir_all(vendored.parent().unwrap()).unwrap();
        std::fs::write(&vendored, "x").unwrap();

        assert_eq!(resolve("vendor/thing.md", &skills).unwrap(), vendored);
    }
}
