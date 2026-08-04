//! The paths the harness works from, and the built-in defaults a machine
//! overlays.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::core::machine::{Budgets, ModelSpec};

/// Everything loop reads or writes, all of it under `<project>/.loop/`.
///
/// One root, so "where does this come from" has one answer. A ticket directory
/// is self-contained: committable, reviewable, and `rm -rf`-able in the same
/// breath as the branch it belongs to. Reuse is `loop init --from <dir>`, which
/// copies — so what you started from is recorded in the ticket rather than
/// resolved out from under it, and editing the source cannot change a run
/// already in flight.
///
/// A newtype over one `PathBuf` because what it earns is the methods below,
/// not the field: nothing serializes or compares a `Paths`, so it derives
/// neither.
#[derive(Clone, Debug)]
pub struct Paths {
    /// The project root the run drives — where `.loop/` lives and pi is spawned.
    pub project_dir: PathBuf,
}

/// The two subdirectories a machine resolves names against. Named here because
/// [`Paths`] builds them for `loop init` while `toolbox` builds them again
/// against the machine file's own directory — the two have to agree, and a
/// string literal in each is how they quietly stop agreeing.
pub const STAGE_PROMPTS_DIR: &str = "stage-prompts";
pub const SKILLS_DIR: &str = "skills";

impl Paths {
    /// Root everything at `project_dir`. Not `discover`: nothing is
    /// discovered, and the old name promised an upward search for a `.loop/`
    /// that has never happened — `loop` works in the directory it is run in,
    /// or in `--project-dir`.
    pub fn new(project_dir: impl Into<PathBuf>) -> Self {
        Self {
            project_dir: project_dir.into(),
        }
    }

    /// The ticket directory. Everything below is inside it.
    pub fn loop_dir(&self) -> PathBuf {
        self.project_dir.join(".loop")
    }

    // ── authored ──────────────────────────────────────────────────────────
    pub fn machine_file(&self) -> PathBuf {
        self.loop_dir().join("machine.fnl")
    }
    pub fn stage_prompts(&self) -> PathBuf {
        self.loop_dir().join(STAGE_PROMPTS_DIR)
    }
    pub fn skills(&self) -> PathBuf {
        self.loop_dir().join(SKILLS_DIR)
    }

    // ── recorded ──────────────────────────────────────────────────────────
    pub fn ledger_file(&self) -> PathBuf {
        self.loop_dir().join("ledger.jsonl")
    }
    pub fn artifacts_dir(&self) -> PathBuf {
        self.loop_dir().join("artifacts")
    }

    // ── generated ─────────────────────────────────────────────────────────
    /// Rendered prompts and handoff files. Derived from the machine and the
    /// ledger, so deleting it costs nothing — which is why `loop init` writes
    /// it into `.gitignore`.
    pub fn run_dir(&self) -> PathBuf {
        self.loop_dir().join("run")
    }

    /// Where a Worker spawn writes its handoff JSON. One file per attempt, so
    /// a retry can never read the previous attempt's proposal — and the
    /// harness deletes it before spawning anyway, belt and braces.
    pub fn handoff_file(&self, state: &str, cycle: u32, attempt: u32) -> PathBuf {
        self.run_file(state, cycle, attempt, "handoff.json")
    }

    /// Where a rendered prompt for one attempt lands.
    pub fn render_file(&self, state: &str, cycle: u32, attempt: u32, suffix: &str) -> PathBuf {
        self.run_file(state, cycle, attempt, &format!("{suffix}.md"))
    }

    /// The rendered-prompt path with the cycle and attempt left as placeholders
    /// — what `loop preview` shows for a stage that has not run.
    ///
    /// The *same* builder as the real thing, handed `<cycle>` and `<attempt>`
    /// instead of numbers, rather than a second format string that has to be
    /// kept in step with it. There is a test pinning the two together, which
    /// was the tell that they were two.
    #[must_use]
    pub fn render_file_pattern(&self, state: &str, suffix: &str) -> PathBuf {
        self.run_file(state, "<cycle>", "<attempt>", &format!("{suffix}.md"))
    }

    /// The one place that knows how a `run/` filename is spelled.
    fn run_file(
        &self,
        state: &str,
        cycle: impl std::fmt::Display,
        attempt: impl std::fmt::Display,
        tail: &str,
    ) -> PathBuf {
        self.run_dir().join(format!(
            "{}-{cycle}-{attempt}-{tail}",
            sanitize_component(state, "state")
        ))
    }
}

/// Collapse anything that isn't safe as a **single** path component down to
/// `-`, falling back to `fallback` when nothing usable survives.
///
/// One function for every name the harness interpolates into a path: run-file
/// names, artifact filenames, and session ids. One rather than three, so a
/// state named `open_pr` cannot be spelled two ways ten files apart.
///
/// Two properties are load-bearing rather than cosmetic:
/// - **No `/` survives**, so a name the harness did not choose can never
///   introduce a path separator into a filename built by interpolation. `.` is
///   kept, because it is harmless in a lone component and keeps an artifact
///   called `report.md` readable.
/// - **The result is never empty and never all dots**, so it can never be a
///   bare `..`. Worker-supplied artifact names make that necessary; state ids
///   reach a path too, so it holds for every caller.
pub fn sanitize_component(s: &str, fallback: &str) -> String {
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
        fallback.to_string()
    } else {
        cleaned
    }
}

/// The hash recorded in `run_started` and re-checked by `loop recap`.
///
/// One function rather than the expression written out at each call site: the
/// whole of `recap`'s provenance decision is these bytes matching the ones
/// `fennel` recorded at load time, and two spellings of one algorithm is how
/// they quietly stop matching. The test fixtures build their `machine_hash`
/// through it too, so a fixture ledger carries a hash the real reader would
/// accept rather than a `"sha256:test"` shaped like nothing the harness writes.
pub fn machine_hash(source: &str) -> String {
    hex::encode(<sha2::Sha256 as sha2::Digest>::digest(source.as_bytes()))
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

/// Whether an authored name is meant as a path rather than a bare name.
///
/// The escape hatch both `:stage-prompt` and `:skills` offer: a value with a
/// `/` in it is taken as an exact path relative to the machine file, instead of
/// being looked up under `stage-prompts/` or `skills/`.
///
/// One predicate, because it was two — decided at *load* time for a stage
/// prompt (`convert` picking a [`crate::core::StagePromptRef`] variant) and at
/// *resolve* time for a skill (`toolbox::skill` picking a `Lookup`) — so
/// widening the rule, say to treat a leading `./` as a path too, had to be done
/// in two modules at two different layers or the two kinds would disagree about
/// what an author had written.
pub fn names_a_path(value: &str) -> bool {
    value.contains('/')
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

/// The built-in defaults a machine overlays, and nothing else.
///
/// Not read from a file: a machine that wants a different model or budget says
/// so itself, and `loop init --from <dir>` is how that stops being retyped per
/// ticket.
///
/// Every field is consumed exactly once, by [`crate::fennel::convert`], and
/// copied into the `Machine`. Nothing reads them again — so runtime callers
/// take [`Paths`] or [`pi_bin`], not this.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Floor {
    // No bare `provider`: each role spec carries its own, and the machine's
    // `:provider` key rewrites all three at once.
    /// Default Worker model when a state doesn't specify one.
    pub worker: ModelSpec,
    pub judge: ModelSpec,
    pub navigator: ModelSpec,
    pub navigator_max_invocations: u32,

    // No `default_skills` / `default_mcp`: there is one baseline, the
    // machine's `:defaults`, which `Machine::resolve_skills` unions with the
    // state's. A second would be a tier nothing merges.
    /// Installed pi-extension package names activated per spawn
    /// (`mcp`, `review-model-selector`).
    pub pi_extensions: Vec<String>,

    pub budgets: Budgets,
    /// How many recent transitions the digest includes verbatim.
    pub digest_last_n: usize,
}

impl Default for Floor {
    fn default() -> Self {
        let anthropic = |model: &str, thinking| ModelSpec {
            provider: "anthropic".into(),
            model: model.into(),
            thinking,
        };
        use crate::core::machine::Thinking;
        Self {
            worker: anthropic("claude-sonnet-5", Thinking::Medium),
            judge: anthropic("claude-haiku-4-5", Thinking::Low),
            navigator: anthropic("claude-haiku-4-5", Thinking::Low),
            navigator_max_invocations: 5,
            pi_extensions: vec!["mcp".into(), "review-model-selector".into()],
            budgets: Budgets {
                usd: Some(15.0),
                wallclock_s: Some(7200),
                max_transitions: Some(60),
            },
            digest_last_n: 8,
        }
    }
}

/// The pi executable. `LOOP_PI_BIN` overrides it — that is how the integration
/// tests point the whole harness at `mock-pi`.
pub fn pi_bin() -> String {
    std::env::var("LOOP_PI_BIN").unwrap_or_else(|_| "pi".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One sanitizer now spells run-file names, artifact filenames, and session
    /// ids, so the properties every one of them relies on are pinned here
    /// rather than three times over.
    #[test]
    fn sanitize_component_holds_the_properties_all_three_callers_need() {
        // Nothing that could split a path survives — this is the one that
        // matters, because artifact names come from the worker.
        assert_eq!(
            sanitize_component("../../etc/passwd", "artifact"),
            "..-..-etc-passwd"
        );
        assert!(!sanitize_component("a/b", "x").contains('/'));

        // `.`, `-` and `_` are kept, so ordinary names stay readable.
        assert_eq!(
            sanitize_component("normal-name_1.2", "artifact"),
            "normal-name_1.2"
        );
        // Previously this was the disagreement: the run-file sanitizer kept
        // `_` and the session-id one did not, so one state had two spellings.
        assert_eq!(sanitize_component("open_pr", "state"), "open_pr");

        // Never empty, never a bare `..`.
        assert_eq!(sanitize_component("", "state"), "state");
        assert_eq!(sanitize_component("..", "artifact"), "artifact");
        assert_eq!(sanitize_component("...", "state"), "state");
        // A name that is only separators collapses to dashes, which is not
        // all-dots, so it keeps its own (harmless) spelling.
        assert_eq!(sanitize_component("///", "state"), "---");
    }

    /// The two run-file builders and the pattern `loop preview` prints must
    /// agree, since preview's whole job is to name the file a run would write.
    #[test]
    fn preview_pattern_matches_the_file_a_run_would_write() {
        let paths = Paths::new("/proj");
        let real = paths.render_file("open_pr", 2, 3, "system");
        let pattern = paths.render_file_pattern("open_pr", "system");

        assert_eq!(
            real.file_name().unwrap(),
            "open_pr-2-3-system.md",
            "the `_` survives, as it does in the session id"
        );
        assert_eq!(
            pattern.file_name().unwrap(),
            "open_pr-<cycle>-<attempt>-system.md"
        );
        assert_eq!(real.parent(), pattern.parent());
    }
}
