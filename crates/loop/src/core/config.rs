//! The paths the harness works from, and the built-in defaults a machine
//! overlays.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::core::machine::{Budgets, ModelSpec};

/// Everything loop reads or writes, all of it under `<project>/.loop/`.
///
/// There used to be three roots: a toolbox at `~/.config/loop`, the ticket at
/// `<project>/.loop`, and generated renders at `~/.local/state/loop`. Stage prompts
/// and skills resolved local-first across the first two, which meant "where
/// does this come from" had two answers, `loop preview` had to report which
/// one won, and editing a toolbox stage prompt silently changed the next stage of
/// every in-flight ticket.
///
/// One root instead. A ticket directory is now self-contained: committable,
/// reviewable, and `rm -rf`-able in the same breath as the branch it belongs
/// to. Reuse is `loop init --from <dir>`, which copies — so what you started
/// from is recorded in the ticket rather than resolved out from under it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
    pub fn discover(project_dir: impl Into<PathBuf>) -> Self {
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

    /// The one place that knows how a `run/` filename is spelled.
    ///
    /// It was three: this, the rendered-prompt name in `toolbox`, and a third
    /// literal in `report` so `loop preview` could *guess* what the second one
    /// would produce. Only this one sanitized, so a state id containing a `/`
    /// wrote its handoff correctly and its prompt to a path that did not exist.
    fn run_file(&self, state: &str, cycle: u32, attempt: u32, tail: &str) -> PathBuf {
        self.run_dir().join(format!(
            "{}-{cycle}-{attempt}-{tail}",
            sanitize_component(state, "state")
        ))
    }

    /// The rendered-prompt path with the cycle and attempt left as placeholders
    /// — what `loop preview` shows for a stage that has not run. Built from the
    /// same sanitizer as the real thing, so the two cannot drift.
    pub fn render_file_pattern(&self, state: &str, suffix: &str) -> PathBuf {
        self.run_dir().join(format!(
            "{}-<cycle>-<attempt>-{suffix}.md",
            sanitize_component(state, "state")
        ))
    }
}

/// Collapse anything that isn't safe as a **single** path component down to
/// `-`, falling back to `fallback` when nothing usable survives.
///
/// One function for every name the harness interpolates into a path: run-file
/// names, artifact filenames, and session ids. There used to be three, and they
/// disagreed — `.loop/run/` kept `_`, session ids did not, and artifact names
/// kept `.` as well — so a state named `open_pr` wrote `open_pr-1-1-handoff.json`
/// beside a session id of `PROJ-open-pr-1-1`: two spellings of one state, ten
/// files apart.
///
/// Two properties are load-bearing rather than cosmetic:
/// - **No `/` survives**, so a name the harness did not choose can never
///   introduce a path separator into a filename built by interpolation. `.` is
///   kept, because it is harmless in a lone component and keeps an artifact
///   called `report.md` readable.
/// - **The result is never empty and never all dots**, so it can never be a
///   bare `..`. That guard used to protect artifact names alone, on the grounds
///   that those are worker-supplied — but a state id reaches a path too, and
///   there was no reason for the weaker rule to hold anywhere.
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

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
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

/// The settings a run uses, and where it works.
///
/// This is no longer read from a file. `config.fnl` was a second authored
/// artifact in a second directory whose only job was to hold values a machine
/// could already override — so the values moved into the machine, the file
/// went, and what remains here is the built-in floor a machine overlays plus
/// the two things a machine has no business setting (`paths`, `pi_bin`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    // No bare `provider` here. Each role spec below carries its own, and the
    // machine's `:provider` key is what rewrites all three at once — a
    // separate fallback field read by nothing was a fourth tier waiting to be
    // wired to something that never merged it.
    /// Default Worker model when a state doesn't specify one.
    pub worker: ModelSpec,
    pub judge: ModelSpec,
    pub navigator: ModelSpec,
    pub navigator_max_invocations: u32,

    // No `default_skills` / `default_mcp` here. They were the config file's
    // spelling of a baseline tier the machine already had as `:defaults`, and
    // with one authored file there is one baseline: `Machine::resolve_skills`
    // unions the machine defaults with the state's, and that is the whole
    // chain. Keeping empty fields around invited a third tier that nothing
    // merged.
    /// Installed pi-extension package names activated per spawn
    /// (`mcp`, `review-model-selector`).
    pub pi_extensions: Vec<String>,

    pub budgets: Budgets,
    /// How many recent transitions the digest includes verbatim.
    pub digest_last_n: usize,

    /// The pi executable. `LOOP_PI_BIN` overrides it — that is how the
    /// integration tests point the whole harness at `mock-pi`.
    pub pi_bin: String,

    pub paths: Paths,
}

impl Config {
    /// The built-in floor, before the machine overlays its own keys.
    pub fn defaults(paths: Paths) -> Self {
        Self {
            worker: ModelSpec {
                provider: "anthropic".into(),
                model: "claude-sonnet-5".into(),
                thinking: crate::core::machine::Thinking::Medium,
            },
            judge: ModelSpec {
                provider: "anthropic".into(),
                model: "claude-haiku-4-5".into(),
                thinking: crate::core::machine::Thinking::Low,
            },
            navigator: ModelSpec {
                provider: "anthropic".into(),
                model: "claude-haiku-4-5".into(),
                thinking: crate::core::machine::Thinking::Low,
            },
            navigator_max_invocations: 5,
            pi_extensions: vec!["mcp".into(), "review-model-selector".into()],
            budgets: Budgets {
                usd: Some(15.0),
                wallclock_s: Some(7200),
                max_transitions: Some(60),
            },
            digest_last_n: 8,
            pi_bin: std::env::var("LOOP_PI_BIN").unwrap_or_else(|_| "pi".into()),
            paths,
        }
    }
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

        // Never empty, never a bare `..` — the guard that used to apply to
        // artifact names alone.
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
        let paths = Paths::discover("/proj");
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
