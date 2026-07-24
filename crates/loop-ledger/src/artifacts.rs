//! Artifact capture: temp-file + atomic rename + sha256, so a crash never
//! leaves a half-written file a later stage might read.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use loop_core::{ArtifactClaim, ArtifactRef, ArtifactSink, CoreError, IoContext, Result};

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

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Verify a previously captured artifact still matches its recorded hash.
    pub fn verify(&self, art: &ArtifactRef) -> Result<bool> {
        let path = self.resolve_recorded_path(&art.path);
        let bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => {
                return Err(CoreError::io(
                    format!("reading artifact {}", path.display()),
                    e,
                ));
            }
        };
        Ok(sha256_hex(&bytes) == art.sha256)
    }

    /// `art.path` is stored project-relative where possible (see `capture`);
    /// tolerate an absolute path too, since nothing stops a caller from
    /// building an `ArtifactRef` by hand for a test or a `verify` from another
    /// project layout.
    fn resolve_recorded_path(&self, recorded: &str) -> PathBuf {
        let p = Path::new(recorded);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.project_root.join(p)
        }
    }

    /// Resolve a worker's claimed source path against the project root and
    /// reject anything that escapes it — a worker naming `/etc/passwd`, or a
    /// relative path walking out via `..`, or a symlink whose target lands
    /// outside the root. Canonicalizing both sides and comparing resolves all
    /// three at once: `fs::canonicalize` follows every symlink component, so
    /// the comparison is against where the path *actually* points, not its
    /// spelling (docs/07-risks.md #10).
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
    /// Copy a worker-claimed file into the store as `<state>-<cycle>-<name>`,
    /// writing a sibling `.sha256`.
    fn capture(&self, state: &str, cycle: u32, claim: &ArtifactClaim) -> Result<ArtifactRef> {
        let source = self.resolve_claimed_source(&claim.path)?;
        let bytes =
            fs::read(&source).io_ctx(format!("reading artifact source {}", source.display()))?;
        let hash = sha256_hex(&bytes);

        // `state` and `claim.name` both come from the worker's `transition`
        // call (untrusted) — sanitize before they become path components, so
        // a crafted name like `../../etc/passwd` can't escape the artifacts
        // directory the way a crafted *source* path could (same risk,
        // different vector: the destination this time, not the source).
        let dest_name = format!(
            "{}-{cycle}-{}",
            sanitize_component(state),
            sanitize_component(&claim.name)
        );
        let dest_path = self.root.join(&dest_name);
        write_atomic(&self.root, &dest_path, &bytes)?;

        let sidecar_name = format!("{dest_name}.sha256");
        let sidecar_path = self.root.join(&sidecar_name);
        write_atomic(&self.root, &sidecar_path, hash.as_bytes())?;

        Ok(ArtifactRef {
            name: claim.name.clone(),
            path: relativize(&self.project_root, &dest_path),
            sha256: hash,
        })
    }
}

/// Best-effort project-relative rendering of a captured path, for
/// `ArtifactRef.path` (docs/03-ledger.md: "Project-relative, e.g.
/// `.loop/artifacts/implement-1-diff.patch`"). Falls back to the path as-is
/// when it isn't actually under `project_root` (e.g. an absolute artifacts
/// root configured outside the project in a test).
fn relativize(project_root: &Path, path: &Path) -> String {
    path.strip_prefix(project_root)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string_lossy().into_owned())
}

/// Collapse anything that isn't safe as a single path component down to `-`,
/// so a worker-supplied string can never introduce a `/` (or a bare `..`)
/// into a filename we build by string interpolation.
fn sanitize_component(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if cleaned.is_empty() || cleaned.chars().all(|c| c == '.') {
        "artifact".to_string()
    } else {
        cleaned
    }
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

/// Hex sha256 of a byte slice.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
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
    fn sha256_hex_matches_known_vectors() {
        // Standard NIST-style test vectors for SHA-256.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"hello world"),
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn capture_hashes_content_and_writes_sidecar() {
        let project = tempfile::tempdir().unwrap();
        fs::create_dir_all(project.path().join("work")).unwrap();
        let src = project.path().join("work").join("diff.patch");
        fs::write(&src, b"some diff content").unwrap();

        let store = store_in(project.path());
        let claim = ArtifactClaim {
            name: "diff".into(),
            path: "work/diff.patch".into(),
        };
        let art = store.capture("implement", 1, &claim).unwrap();

        assert_eq!(art.name, "diff");
        assert_eq!(art.sha256, sha256_hex(b"some diff content"));
        assert_eq!(art.path, ".loop/artifacts/implement-1-diff");

        let dest = project.path().join(&art.path);
        assert_eq!(fs::read(&dest).unwrap(), b"some diff content");

        let sidecar = project
            .path()
            .join(".loop/artifacts/implement-1-diff.sha256");
        assert_eq!(fs::read_to_string(&sidecar).unwrap(), art.sha256);

        assert!(store.verify(&art).unwrap());
    }

    #[test]
    fn verify_detects_tampering() {
        let project = tempfile::tempdir().unwrap();
        let src = project.path().join("diff.patch");
        fs::write(&src, b"original").unwrap();
        let store = store_in(project.path());
        let art = store
            .capture(
                "implement",
                1,
                &ArtifactClaim {
                    name: "diff".into(),
                    path: "diff.patch".into(),
                },
            )
            .unwrap();

        let dest = project.path().join(&art.path);
        fs::write(&dest, b"tampered!").unwrap();
        assert!(!store.verify(&art).unwrap());
    }

    #[test]
    fn verify_missing_file_returns_false_not_error() {
        let project = tempfile::tempdir().unwrap();
        let store = store_in(project.path());
        let art = ArtifactRef {
            name: "diff".into(),
            path: ".loop/artifacts/implement-1-diff".into(),
            sha256: "whatever".into(),
        };
        assert!(!store.verify(&art).unwrap());
    }

    #[test]
    fn rejects_absolute_path_escaping_root() {
        let project = tempfile::tempdir().unwrap();
        let store = store_in(project.path());
        let claim = ArtifactClaim {
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
        let outside = parent.join(format!("loop-ledger-escape-test-{}", std::process::id()));
        fs::write(&outside, b"secret").unwrap();

        let claim = ArtifactClaim {
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
        let claim = ArtifactClaim {
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
                &ArtifactClaim {
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
        let leftover_tmp = fs::read_dir(store.root())
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().starts_with(".tmp"));
        assert!(
            !leftover_tmp,
            "atomic write must not leave temp files behind"
        );
    }

    #[test]
    fn sanitize_component_strips_path_traversal() {
        // `.` is kept (harmless in a single filename with no `/` beside it),
        // but every path separator becomes `-`, so the result can never be
        // interpreted as multiple components or as a bare `..`.
        assert_eq!(sanitize_component("../../etc/passwd"), "..-..-etc-passwd");
        assert_eq!(sanitize_component("normal-name_1.2"), "normal-name_1.2");
        assert_eq!(sanitize_component(""), "artifact");
        assert_eq!(sanitize_component(".."), "artifact");
    }
}
