//! Artifact capture: temp-file + atomic rename, so a crash never leaves a
//! half-written file a later stage might read.
//!
//! What capture buys is a *snapshot*. The worker names a file in the working
//! tree; the store copies it under a `<state>-<cycle>-<name>` key, and that
//! copy is what `$ARTIFACT_DIFF` and the digest's artifact table point at, so
//! cycle two rewriting `diff.patch` does not retroactively change what cycle
//! one handed off. Nothing here hashes: no consumer ever checked a hash, and
//! recording one would assert an integrity guarantee the system does not make.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::core::{Artifact, ArtifactSink, CoreError, IoContext, Result, sanitize_component};

pub struct ArtifactStore {
    root: PathBuf,
    project_root: PathBuf,
}

impl ArtifactStore {
    /// `root` is `./.loop/artifacts/`; `project_root` bounds what a worker may
    /// claim.
    pub fn new(root: impl AsRef<Path>, project_root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
            project_root: project_root.as_ref().to_path_buf(),
        }
    }

    /// Resolve a worker's claimed source path against the project root and
    /// reject anything that escapes it — a worker naming `/etc/passwd`, or a
    /// relative path walking out via `..`, or a symlink whose target lands
    /// outside the root. Canonicalizing both sides and comparing resolves all
    /// three at once: `fs::canonicalize` follows every symlink component, so
    /// the comparison is against where the path *actually* points, not its
    /// spelling (docs/design-notes.md).
    fn resolve_claimed_source(&self, claimed: &str) -> Result<PathBuf> {
        let candidate = Path::new(claimed);
        let joined = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            self.project_root.join(candidate)
        };
        let canonical_root = fs::canonicalize(&self.project_root).io_ctx(format!(
            "resolving project root {}",
            self.project_root.display()
        ))?;
        let canonical_source = fs::canonicalize(&joined)
            .io_ctx(format!("resolving claimed artifact path {claimed}"))?;
        if !canonical_source.starts_with(&canonical_root) {
            return Err(CoreError::other(format!(
                "artifact claim `{claimed}` escapes the project root ({})",
                canonical_source.display()
            )));
        }
        Ok(canonical_source)
    }
}

impl ArtifactSink for ArtifactStore {
    /// Copy a worker-claimed file into the store as `<state>-<cycle>-<name>`.
    fn capture(&self, state: &str, cycle: u32, claim: &Artifact) -> Result<Artifact> {
        let source = self.resolve_claimed_source(&claim.path)?;
        let bytes =
            fs::read(&source).io_ctx(format!("reading artifact source {}", source.display()))?;

        // `state` and `claim.name` both come from the worker's `transition`
        // call (untrusted) — sanitize before they become path components, so
        // a crafted name like `../../etc/passwd` can't escape the artifacts
        // directory the way a crafted *source* path could (same risk,
        // different vector: the destination this time, not the source).
        let dest_name = format!(
            "{}-{cycle}-{}",
            sanitize_component(state, "state"),
            sanitize_component(&claim.name, "artifact")
        );
        let dest_path = self.root.join(&dest_name);
        write_atomic(&self.root, &dest_path, &bytes)?;

        Ok(Artifact {
            name: claim.name.clone(),
            path: relativize(&self.project_root, &dest_path),
        })
    }
}

/// Best-effort project-relative rendering of a captured path, for
/// `Artifact.path` (skills/loop-authoring/references/runtime.md: "Project-relative, e.g.
/// `.loop/artifacts/implement-1-diff.patch`"). Falls back to the path as-is
/// when it isn't actually under `project_root` (e.g. an absolute artifacts
/// root configured outside the project in a test).
fn relativize(project_root: &Path, path: &Path) -> String {
    path.strip_prefix(project_root)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string_lossy().into_owned())
}

/// Write `bytes` to `dest` atomically: a temp file in `dir` (so the rename is
/// same-filesystem), `fsync`ed, then renamed over `dest`. A crash at any point
/// before the rename leaves only a stray temp file — `dest` itself is either
/// absent or the previous complete version, never a partial write.
fn write_atomic(dir: &Path, dest: &Path, bytes: &[u8]) -> Result<()> {
    fs::create_dir_all(dir).io_ctx(format!("creating artifacts directory {}", dir.display()))?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)
        .io_ctx(format!("creating temp file in {}", dir.display()))?;
    tmp.write_all(bytes)
        .io_ctx(format!("writing {}", dest.display()))?;
    tmp.as_file()
        .sync_all()
        .io_ctx(format!("fsyncing {}", dest.display()))?;
    tmp.persist(dest)
        .map_err(|e| CoreError::io(format!("renaming into {}", dest.display()), e.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn store_in(project: &Path) -> ArtifactStore {
        let root = project.join(".loop").join("artifacts");
        ArtifactStore::new(root, project)
    }

    #[test]
    fn capture_copies_the_claimed_file_into_the_store() {
        let project = tempfile::tempdir().unwrap();
        fs::create_dir_all(project.path().join("work")).unwrap();
        let src = project.path().join("work").join("diff.patch");
        fs::write(&src, b"some diff content").unwrap();

        let store = store_in(project.path());
        let claim = Artifact {
            name: "diff".into(),
            path: "work/diff.patch".into(),
        };
        let art = store.capture("implement", 1, &claim).unwrap();

        assert_eq!(art.name, "diff");
        assert_eq!(art.path, ".loop/artifacts/implement-1-diff");

        let dest = project.path().join(&art.path);
        assert_eq!(fs::read(&dest).unwrap(), b"some diff content");
    }

    /// The point of copying rather than recording the claimed path: the
    /// snapshot keeps meaning what it meant when the stage handed it off, even
    /// after a later cycle rewrites the file it came from.
    #[test]
    fn a_captured_snapshot_survives_the_source_being_rewritten() {
        let project = tempfile::tempdir().unwrap();
        let src = project.path().join("diff.patch");
        fs::write(&src, b"cycle one").unwrap();

        let store = store_in(project.path());
        let claim = Artifact {
            name: "diff".into(),
            path: "diff.patch".into(),
        };
        let first = store.capture("implement", 1, &claim).unwrap();

        fs::write(&src, b"cycle two").unwrap();
        let second = store.capture("implement", 2, &claim).unwrap();

        assert_ne!(first.path, second.path);
        assert_eq!(
            fs::read(project.path().join(&first.path)).unwrap(),
            b"cycle one"
        );
        assert_eq!(
            fs::read(project.path().join(&second.path)).unwrap(),
            b"cycle two"
        );
    }

    /// A claim naming a file that isn't there is the common worker mistake, so
    /// it has to come back as an ordinary `Err` the engine can record — not a
    /// panic and not something that escapes to kill the run.
    #[test]
    fn a_claim_naming_a_missing_file_is_an_error_not_a_panic() {
        let project = tempfile::tempdir().unwrap();
        let store = store_in(project.path());
        let err = store
            .capture(
                "implement",
                1,
                &Artifact {
                    name: "diff".into(),
                    path: "nope/never-written.patch".into(),
                },
            )
            .unwrap_err();
        assert!(
            err.to_string().contains("never-written.patch"),
            "the error must name the claim: {err}"
        );
    }

    #[test]
    fn rejects_absolute_path_escaping_root() {
        let project = tempfile::tempdir().unwrap();
        let store = store_in(project.path());
        let claim = Artifact {
            name: "passwd".into(),
            path: "/etc/passwd".into(),
        };
        let err = store.capture("implement", 1, &claim).unwrap_err();
        assert!(err.to_string().contains("escapes") || err.to_string().contains("passwd"));
    }

    #[test]
    fn rejects_dotdot_escape() {
        let project = tempfile::tempdir().unwrap();
        fs::create_dir(project.path().join("sub")).unwrap();
        // Something that exists just outside the project root.
        let parent = project.path().parent().unwrap();
        let outside = parent.join(format!("ledger-escape-test-{}", std::process::id()));
        fs::write(&outside, b"secret").unwrap();

        let claim = Artifact {
            name: "secret".into(),
            path: format!(
                "sub/../../{}",
                outside.file_name().unwrap().to_str().unwrap()
            ),
        };
        let store = store_in(project.path());
        let result = store.capture("implement", 1, &claim);
        let _ = fs::remove_file(&outside);
        assert!(result.is_err(), "escaping via .. must be rejected");
    }

    #[test]
    fn rejects_symlink_pointing_outside_root() {
        let project = tempfile::tempdir().unwrap();
        let outside_dir = tempfile::tempdir().unwrap();
        let secret = outside_dir.path().join("secret.txt");
        fs::write(&secret, b"outside content").unwrap();

        let link = project.path().join("innocuous.txt");
        symlink(&secret, &link).unwrap();

        let store = store_in(project.path());
        let claim = Artifact {
            name: "innocuous".into(),
            path: "innocuous.txt".into(),
        };
        let err = store.capture("implement", 1, &claim).unwrap_err();
        assert!(err.to_string().contains("escapes"));
    }

    #[test]
    fn capture_is_atomic_no_partial_file_observable() {
        let project = tempfile::tempdir().unwrap();
        let src = project.path().join("big.bin");
        let content = vec![0xABu8; 1024 * 64];
        fs::write(&src, &content).unwrap();

        let store = store_in(project.path());
        let art = store
            .capture(
                "implement",
                1,
                &Artifact {
                    name: "big".into(),
                    path: "big.bin".into(),
                },
            )
            .unwrap();

        let dest = project.path().join(&art.path);
        // The file that exists is either fully present or absent — never a
        // truncated fragment: since we only observe *after* `capture`
        // returns, assert it's the complete content, and that no stray temp
        // file (the atomic-write intermediate) was left behind.
        assert_eq!(fs::read(&dest).unwrap(), content);
        let leftover_tmp = fs::read_dir(&store.root)
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().starts_with(".tmp"));
        assert!(
            !leftover_tmp,
            "atomic write must not leave temp files behind"
        );
    }
}
