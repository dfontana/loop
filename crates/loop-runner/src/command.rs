//! Assembling the `pi` command line for each of the three roles.
//!
//! Confirmed against the installed pi (`pi --help`) and its resource loader
//! source (`dist/core/resource-loader.js`):
//! `--print`, `--mode json`, `--session-id`, `--no-session`, `--provider`,
//! `--model <m>:<thinking>`, `--tools`, `--exclude-tools`,
//! `--no-builtin-tools`, `--no-extensions`, `-e <path>`,
//! `--append-system-prompt <text-or-path>`, then the positional message.
//!
//! One correction against docs/05-orchestration.md: `--append-system-prompt`
//! does **not** use an `@path` convention. pi's `resolvePromptInput` calls
//! `existsSync` on the raw argument and reads it as a file if it exists,
//! otherwise treats it as literal text — so we pass the rendered playbook's
//! path bare, no `@` prefix.

use std::process::Command;

use loop_core::{JudgeSpec, NavigatorSpec, TransitionMode, WorkerSpec};

/// Build the Worker command.
///
/// Required environment:
/// - `PI_AGENT_DIR` → the staged agent dir, so `scoped-tools` finds the
///   merged YAML and `mcp` finds `mcp.json`.
/// - `LOOP_REACHABLE` → comma-separated neighbors; the transition tool builds
///   its enum from this.
/// - `LOOP_TRANSITION_MODE` → `constrained` | `open`.
/// - every `spec.env` pair, so a scoped-tool's `valueFromCmd` can read
///   `$TICKET_ID` / `$CYCLE` and key its idempotency on them.
///
/// Extension discovery is left on (no `--no-extensions`): the Worker is the
/// one role that needs the installed `scoped-tools`/`mcp`/
/// `review-model-selector` pi-extensions to load alongside the harness's own
/// injected `transition` tool.
pub fn worker_command(pi_bin: &str, spec: &WorkerSpec) -> Command {
    let mut cmd = Command::new(pi_bin);
    cmd.arg("--print").arg("--mode").arg("json");

    if let Some(id) = &spec.session_id {
        cmd.arg("--session-id").arg(id);
    }

    cmd.arg("--provider").arg(&spec.model.provider);
    cmd.arg("--model").arg(spec.model.pi_model_arg());

    if !spec.tools.is_empty() {
        cmd.arg("--tools").arg(spec.tools.join(","));
    }
    if !spec.exclude_tools.is_empty() {
        cmd.arg("--exclude-tools").arg(spec.exclude_tools.join(","));
    }

    for ext in &spec.ext_paths {
        cmd.arg("-e").arg(ext);
    }

    cmd.arg("--append-system-prompt")
        .arg(&spec.system_prompt_path);
    cmd.arg(&spec.entry_message);

    cmd.current_dir(&spec.cwd);

    cmd.env("PI_AGENT_DIR", &spec.agent_dir);
    cmd.env("LOOP_REACHABLE", spec.reachable.join(","));
    cmd.env(
        "LOOP_TRANSITION_MODE",
        match spec.transition_mode {
            TransitionMode::Constrained => "constrained",
            TransitionMode::Open => "open",
        },
    );
    for (k, v) in &spec.env {
        cmd.env(k, v);
    }

    // Diagnostic-only environment, read by `mock-pi` to walk its script. The
    // real pi ignores environment variables it doesn't recognize, so setting
    // these unconditionally is safe against a real spawn.
    cmd.env("LOOP_MOCK_ROLE", "worker");
    cmd.env("LOOP_MOCK_STATE", &spec.state);
    cmd.env("LOOP_MOCK_CYCLE", spec.cycle.to_string());
    cmd.env("LOOP_MOCK_ATTEMPT", spec.attempt.to_string());

    cmd
}

/// The Judge gets no code tools at all: `--no-builtin-tools` disables the
/// built-ins, and `--no-extensions` stops any *installed* pi-extension
/// (`scoped-tools`, `mcp`, …) from being auto-discovered and handing it a
/// side door. The only tool it has is the explicitly `-e`'d `verdict-tool.ts`.
/// That independence is what makes its verdict trustworthy (docs/07-risks.md
/// #1) — do not add `read` "for convenience".
pub fn judge_command(pi_bin: &str, spec: &JudgeSpec) -> Command {
    let mut cmd = Command::new(pi_bin);
    cmd.arg("--print")
        .arg("--mode")
        .arg("json")
        .arg("--no-session");

    cmd.arg("--provider").arg(&spec.model.provider);
    cmd.arg("--model").arg(spec.model.pi_model_arg());

    cmd.arg("--no-builtin-tools").arg("--no-extensions");
    cmd.arg("-e").arg(&spec.ext_path);

    cmd.arg("--append-system-prompt").arg(&spec.criteria);
    cmd.arg(judge_message(spec));

    cmd.current_dir(&spec.cwd);
    cmd.env("LOOP_MOCK_ROLE", "judge");

    cmd
}

fn judge_message(spec: &JudgeSpec) -> String {
    let mut message = spec.worker_digest.clone();
    if !spec.artifact_paths.is_empty() {
        message.push_str("\n\nArtifacts:\n");
        for p in &spec.artifact_paths {
            message.push_str(&format!("- {}\n", p.display()));
        }
    }
    message
}

/// Same isolation as the Judge — `--no-builtin-tools --no-extensions`, only
/// the explicit `choose-tool.ts`. `LOOP_REACHABLE` is exported because
/// `choose-tool.ts` reads it to build its `to` enum (reachable neighbors plus
/// the always-available `escalate`), exactly like the Worker's transition
/// tool does.
pub fn navigator_command(pi_bin: &str, spec: &NavigatorSpec) -> Command {
    let mut cmd = Command::new(pi_bin);
    cmd.arg("--print")
        .arg("--mode")
        .arg("json")
        .arg("--no-session");

    cmd.arg("--provider").arg(&spec.model.provider);
    cmd.arg("--model").arg(spec.model.pi_model_arg());

    cmd.arg("--no-builtin-tools").arg("--no-extensions");
    cmd.arg("-e").arg(&spec.ext_path);

    cmd.arg("--append-system-prompt").arg(&spec.graph_summary);
    cmd.arg(navigator_message(spec));

    cmd.current_dir(&spec.cwd);

    cmd.env("LOOP_REACHABLE", spec.reachable.join(","));
    cmd.env("LOOP_MOCK_ROLE", "navigator");
    cmd.env("LOOP_MOCK_STATE", &spec.from);

    cmd
}

fn navigator_message(spec: &NavigatorSpec) -> String {
    let mut message = spec.ledger_digest.clone();
    if let Some(p) = &spec.proposal {
        message.push_str("\n\nWorker rationale: ");
        message.push_str(&p.rationale);
        if p.blocked {
            message.push_str(" (worker reported blocked)");
        }
        if let Some(to) = &p.to {
            message.push_str(&format!(" (worker proposed: {to})"));
        }
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use loop_core::{ArtifactClaim, ModelSpec, Proposal, Thinking, Vars};
    use std::path::PathBuf;

    fn args_of(cmd: &Command) -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn env_of<'a>(cmd: &'a Command, key: &str) -> Option<&'a str> {
        cmd.get_envs()
            .find(|(k, _)| *k == key)
            .and_then(|(_, v)| v)
            .and_then(|v| v.to_str())
    }

    fn worker_spec() -> WorkerSpec {
        WorkerSpec {
            ticket: "PROJ-1487".into(),
            state: "implement".into(),
            cycle: 1,
            attempt: 1,
            model: ModelSpec {
                provider: "anthropic".into(),
                model: "claude-sonnet-5".into(),
                thinking: Thinking::High,
            },
            tools: vec!["read".into(), "edit".into(), "transition".into()],
            exclude_tools: vec!["write".into()],
            system_prompt_path: PathBuf::from("/tmp/playbook.md"),
            entry_message: "Entering implement, cycle 1".into(),
            reachable: vec!["review".into(), "debug".into()],
            transition_mode: TransitionMode::Constrained,
            agent_dir: PathBuf::from("/tmp/agent-dir"),
            ext_paths: vec![PathBuf::from("/tmp/ext/transition-tool.ts")],
            pi_extensions: vec!["scoped-tools".into(), "mcp".into()],
            cwd: PathBuf::from("/tmp/project"),
            session_id: Some("PROJ-1487-implement-1".into()),
            env: vec![("TICKET_ID".into(), "PROJ-1487".into())],
        }
    }

    #[test]
    fn worker_command_has_the_exact_flags() {
        let cmd = worker_command("pi", &worker_spec());
        let args = args_of(&cmd);

        assert_eq!(
            args,
            vec![
                "--print",
                "--mode",
                "json",
                "--session-id",
                "PROJ-1487-implement-1",
                "--provider",
                "anthropic",
                "--model",
                "claude-sonnet-5:high",
                "--tools",
                "read,edit,transition",
                "--exclude-tools",
                "write",
                "-e",
                "/tmp/ext/transition-tool.ts",
                "--append-system-prompt",
                "/tmp/playbook.md",
                "Entering implement, cycle 1",
            ]
        );
        assert_eq!(
            cmd.get_current_dir(),
            Some(std::path::Path::new("/tmp/project"))
        );
    }

    #[test]
    fn worker_command_env_matches_spec() {
        let cmd = worker_command("pi", &worker_spec());
        assert_eq!(env_of(&cmd, "PI_AGENT_DIR"), Some("/tmp/agent-dir"));
        assert_eq!(env_of(&cmd, "LOOP_REACHABLE"), Some("review,debug"));
        assert_eq!(env_of(&cmd, "LOOP_TRANSITION_MODE"), Some("constrained"));
        assert_eq!(env_of(&cmd, "TICKET_ID"), Some("PROJ-1487"));
        assert_eq!(env_of(&cmd, "LOOP_MOCK_ROLE"), Some("worker"));
        assert_eq!(env_of(&cmd, "LOOP_MOCK_STATE"), Some("implement"));
        assert_eq!(env_of(&cmd, "LOOP_MOCK_CYCLE"), Some("1"));
        assert_eq!(env_of(&cmd, "LOOP_MOCK_ATTEMPT"), Some("1"));
    }

    #[test]
    fn worker_command_open_mode_env() {
        let mut spec = worker_spec();
        spec.transition_mode = TransitionMode::Open;
        let cmd = worker_command("pi", &spec);
        assert_eq!(env_of(&cmd, "LOOP_TRANSITION_MODE"), Some("open"));
    }

    #[test]
    fn worker_command_without_session_id_omits_the_flag() {
        let mut spec = worker_spec();
        spec.session_id = None;
        let cmd = worker_command("pi", &spec);
        let args = args_of(&cmd);
        assert!(!args.contains(&"--session-id".to_string()));
    }

    fn judge_spec() -> JudgeSpec {
        JudgeSpec {
            criteria: "All three checklist items must be present.".into(),
            worker_digest: "Added churn_score column; build green.".into(),
            artifact_paths: vec![PathBuf::from(".loop/artifacts/implement-1-diff.patch")],
            model: ModelSpec {
                provider: "anthropic".into(),
                model: "claude-haiku-4-5".into(),
                thinking: Thinking::Low,
            },
            ext_path: PathBuf::from("/tmp/ext/verdict-tool.ts"),
            cwd: PathBuf::from("/tmp/project"),
        }
    }

    #[test]
    fn judge_command_has_no_code_tools() {
        let cmd = judge_command("pi", &judge_spec());
        let args = args_of(&cmd);

        assert!(args.contains(&"--no-builtin-tools".to_string()));
        assert!(args.contains(&"--no-extensions".to_string()));
        // Exactly one `-e`, pointing at verdict-tool.ts — no `read`, no
        // scoped-tools, nothing else.
        let e_positions: Vec<usize> = args
            .iter()
            .enumerate()
            .filter(|(_, a)| a.as_str() == "-e")
            .map(|(i, _)| i)
            .collect();
        assert_eq!(e_positions.len(), 1);
        assert_eq!(args[e_positions[0] + 1], "/tmp/ext/verdict-tool.ts");
        assert!(!args.contains(&"--tools".to_string()));
        assert!(args.contains(&"--no-session".to_string()));
    }

    #[test]
    fn judge_command_message_includes_digest_and_artifacts() {
        let cmd = judge_command("pi", &judge_spec());
        let args = args_of(&cmd);
        let message = args.last().unwrap();
        assert!(message.contains("Added churn_score column"));
        assert!(message.contains(".loop/artifacts/implement-1-diff.patch"));
    }

    fn navigator_spec() -> NavigatorSpec {
        NavigatorSpec {
            graph_summary: "implement -> review -> done".into(),
            ledger_digest: "worker stuck at review".into(),
            from: "review".into(),
            proposal: Some(Proposal {
                to: None,
                blocked: true,
                rationale: "no reachable state fits".into(),
                artifacts: vec![ArtifactClaim {
                    name: "diff".into(),
                    path: "diff.patch".into(),
                }],
                vars: Vars::default(),
            }),
            reachable: vec!["debug".into(), "escalate".into()],
            model: ModelSpec {
                provider: "anthropic".into(),
                model: "claude-haiku-4-5".into(),
                thinking: Thinking::Low,
            },
            ext_path: PathBuf::from("/tmp/ext/choose-tool.ts"),
            cwd: PathBuf::from("/tmp/project"),
        }
    }

    #[test]
    fn navigator_command_is_isolated_and_exports_reachable() {
        let cmd = navigator_command("pi", &navigator_spec());
        let args = args_of(&cmd);
        assert!(args.contains(&"--no-builtin-tools".to_string()));
        assert!(args.contains(&"--no-extensions".to_string()));
        assert!(args.contains(&"/tmp/ext/choose-tool.ts".to_string()));
        assert_eq!(env_of(&cmd, "LOOP_REACHABLE"), Some("debug,escalate"));

        let message = args.last().unwrap();
        assert!(message.contains("worker stuck at review"));
        assert!(message.contains("no reachable state fits"));
        assert!(message.contains("blocked"));
    }
}
