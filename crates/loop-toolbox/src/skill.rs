//! Resolving a stage's skills to the paths pi's `--skill` takes.
//!
//! A skill is a `SKILL.md` plus whatever scripts sit beside it. loop does not
//! parse or rewrite either — it resolves a name to a path and hands that path
//! to pi, which owns the format.
//!
//! Resolution mirrors playbooks, **local-first**, so a ticket can override a
//! toolbox skill without touching the toolbox:
//!
//! 1. `./.loop/skills/<name>/`  (a directory containing `SKILL.md`)
//! 2. `./.loop/skills/<name>.md`
//! 3. `~/.config/loop/skills/<name>/`
//! 4. `~/.config/loop/skills/<name>.md`

use std::path::{Path, PathBuf};

use loop_core::{CoreError, Result};

/// Every place a bare skill name is looked for, in order. Exposed so a miss
/// can report all of them — that message is what makes `loop validate` useful.
pub fn candidates(name: &str, local_dir: &Path, toolbox_dir: &Path) -> Vec<PathBuf> {
    vec![
        local_dir.join(name),
        local_dir.join(format!("{name}.md")),
        toolbox_dir.join(name),
        toolbox_dir.join(format!("{name}.md")),
    ]
}

/// Resolve one skill name to the path pi should load.
///
/// A name containing `/` is an exact path, taken relative to `local_dir`'s
/// parent (the machine's directory) when it isn't absolute — the same escape
/// hatch `:playbook` has.
pub fn resolve(name: &str, local_dir: &Path, toolbox_dir: &Path) -> Result<PathBuf> {
    if name.contains('/') {
        let p = PathBuf::from(name);
        let full = if p.is_absolute() {
            p
        } else {
            local_dir.parent().unwrap_or(local_dir).join(&p)
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

    let searched = candidates(name, local_dir, toolbox_dir);
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
    fn a_local_skill_wins_over_the_toolbox() {
        let tmp = tempdir().unwrap();
        let local = tmp.path().join(".loop/skills");
        let toolbox = tmp.path().join("config/skills");
        std::fs::create_dir_all(&local).unwrap();
        std::fs::create_dir_all(&toolbox).unwrap();
        skill_dir(&toolbox, "deploy", "toolbox version");
        let expected = skill_dir(&local, "deploy", "local version");

        assert_eq!(resolve("deploy", &local, &toolbox).unwrap(), expected);
    }

    #[test]
    fn falls_back_to_the_toolbox_and_accepts_a_bare_md_file() {
        let tmp = tempdir().unwrap();
        let local = tmp.path().join(".loop/skills");
        let toolbox = tmp.path().join("config/skills");
        std::fs::create_dir_all(&local).unwrap();
        std::fs::create_dir_all(&toolbox).unwrap();
        let flat = toolbox.join("qa.md");
        std::fs::write(&flat, "---\nname: qa\n---\n").unwrap();

        assert_eq!(resolve("qa", &local, &toolbox).unwrap(), flat);
    }

    /// An empty `skills/foo/` would resolve and then load nothing — at run
    /// time that is indistinguishable from a skill that does nothing, so it
    /// has to be a miss here instead.
    #[test]
    fn a_directory_without_a_skill_md_does_not_resolve() {
        let tmp = tempdir().unwrap();
        let local = tmp.path().join(".loop/skills");
        let toolbox = tmp.path().join("config/skills");
        std::fs::create_dir_all(toolbox.join("hollow")).unwrap();
        std::fs::create_dir_all(&local).unwrap();

        assert!(resolve("hollow", &local, &toolbox).is_err());
    }

    #[test]
    fn a_miss_lists_every_path_it_searched() {
        let tmp = tempdir().unwrap();
        let local = tmp.path().join(".loop/skills");
        let toolbox = tmp.path().join("config/skills");

        let err = resolve("missing", &local, &toolbox).unwrap_err();
        match err {
            CoreError::Unresolved {
                kind,
                name,
                searched,
            } => {
                assert_eq!(kind, "skill");
                assert_eq!(name, "missing");
                assert_eq!(searched.len(), 4);
            }
            other => panic!("expected Unresolved, got {other:?}"),
        }
    }

    #[test]
    fn a_name_with_a_slash_is_an_exact_path() {
        let tmp = tempdir().unwrap();
        let machine_dir = tmp.path().join(".loop");
        let local = machine_dir.join("skills");
        let toolbox = tmp.path().join("config/skills");
        std::fs::create_dir_all(&local).unwrap();
        // Relative to the *machine* dir, mirroring how `:playbook` paths resolve.
        let vendored = machine_dir.join("vendor/thing.md");
        std::fs::create_dir_all(vendored.parent().unwrap()).unwrap();
        std::fs::write(&vendored, "x").unwrap();

        assert_eq!(
            resolve("vendor/thing.md", &local, &toolbox).unwrap(),
            vendored
        );
    }
}
