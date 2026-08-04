//! Assembling the `pi` command line for each of the three roles.
//!
//! Confirmed against the installed pi (`pi --help`) and its resource loader
//! source (`dist/core/resource-loader.js`):
//! `--print`, `--mode json`, `--session-id`, `--no-session`, `--provider`,
//! `--model <m>:<thinking>`, `--no-skills`, `--skill <path>`,
//! `--no-builtin-tools`, `--no-extensions`,
//! `--append-system-prompt <text-or-path>`, then the positional message.
//!
//! One correction against docs/02-how-it-works.md: `--append-system-prompt`
//! does **not** use an `@path` convention. pi's `resolvePromptInput` calls
//! `existsSync` on the raw argument and reads it as a file if it exists,
//! otherwise treats it as literal text — so we pass the rendered stage prompt's
//! path bare, no `@` prefix. The Judge and Navigator exploit the other half of
//! that: their system prompts are short enough to pass as literal text.
//!
//! No `-e` anywhere. loop used to inject three vendored TypeScript tools whose
//! only job was to echo their arguments back as a marker line; the Worker now
//! writes a handoff file instead, and the two tool-less roles answer against a
//! first-line contract. See [`crate::runner::reply::handoff_protocol`].

use std::process::Command;

use crate::core::{HANDOFF_ENV, JudgeSpec, NavigatorSpec, WorkerSpec};
use crate::runner::reply::{VERDICT_FAIL, VERDICT_PASS};

/// The sentinel a Navigator names when no reachable state fits. Not a state:
/// it takes the same "unknown target" path through the engine that any
/// unroutable choice does, and lands on the machine's escalation state.
pub const ESCALATE: &str = "escalate";

/// Build the Worker command.
///
/// Required environment:
/// - [`HANDOFF_ENV`] → the absolute path this spawn writes its proposal to.
///   The rendered system prompt names the same path, so the agent can read it
///   either way; the variable is there so a skill's script can write the
///   handoff on the agent's behalf.
/// - every `spec.env` pair, so a stage's tooling can read `$TICKET_ID` /
///   `$CYCLE` and key its idempotency on them.
///
/// Extension discovery is left on (no `--no-extensions`): the Worker is the
/// one role that needs the installed `mcp`/`review-model-selector`
/// pi-extensions to load. Note `PI_AGENT_DIR` is deliberately **not** set — `mcp` reads the
/// user's own `~/.pi/agent/mcp.json`, and pointing it at a loop-owned
/// directory would hide every server the user actually configured.
///
/// Skills, by contrast, are pinned shut. `--no-skills` turns off ambient
/// discovery and each `--skill <path>` adds one back, so a stage loads exactly
/// what its machine named and nothing a stray `~/.pi/skills/` happens to
/// contain. Note this bounds *instructions*, not capability: a skill is a
/// prompt plus a script the agent runs through bash, so withholding one hides
/// know-how rather than revoking access.
pub fn worker_command(pi_bin: &str, spec: &WorkerSpec) -> Command {
    let mut cmd = Command::new(pi_bin);
    cmd.arg("--print").arg("--mode").arg("json");

    if let Some(id) = &spec.session_id {
        cmd.arg("--session-id").arg(id);
    }

    cmd.arg("--provider").arg(&spec.model.provider);
    cmd.arg("--model").arg(spec.model.pi_model_arg());

    cmd.arg("--no-skills");
    for skill in &spec.skill_paths {
        cmd.arg("--skill").arg(skill);
    }

    cmd.arg("--append-system-prompt")
        .arg(&spec.system_prompt_path);
    cmd.arg(&spec.entry_message);

    cmd.current_dir(&spec.cwd);

    cmd.env(HANDOFF_ENV, &spec.handoff_path);
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

/// A spawn for one of the two tool-less roles.
///
/// The Judge and the Navigator get no tools whatsoever: `--no-builtin-tools`
/// disables the built-ins, `--no-extensions` stops any *installed*
/// pi-extension (`mcp`, …) from being auto-discovered and handing them a side
/// door, `--no-skills` stops ambient skill discovery, and there is nothing
/// injected to replace any of it. That independence is what makes the Judge's
/// verdict trustworthy (docs/05-design-notes.md) — do not add `read` "for
/// convenience".
///
/// It is one builder rather than two because that isolation is a property of
/// the *role class*, not of either role. With a copy each, a flag added to one
/// and missed on the other silently gives that role ambient tools back, and no
/// test that checks for the presence of flags would notice an absent one.
///
/// Having no tools is also why both answer in prose against a fixed first-line
/// contract rather than by writing a file: neither could write one if it
/// wanted to.
fn isolated_command(
    pi_bin: &str,
    model: &crate::core::ModelSpec,
    system_prompt: String,
    message: String,
    cwd: &std::path::Path,
    role: &str,
) -> Command {
    let mut cmd = Command::new(pi_bin);
    cmd.arg("--print")
        .arg("--mode")
        .arg("json")
        .arg("--no-session");

    cmd.arg("--provider").arg(&model.provider);
    cmd.arg("--model").arg(model.pi_model_arg());

    cmd.arg("--no-builtin-tools")
        .arg("--no-extensions")
        .arg("--no-skills");

    cmd.arg("--append-system-prompt").arg(system_prompt);
    cmd.arg(message);

    cmd.current_dir(cwd);
    cmd.env("LOOP_MOCK_ROLE", role);

    cmd
}

pub fn judge_command(pi_bin: &str, spec: &JudgeSpec) -> Command {
    isolated_command(
        pi_bin,
        &spec.model,
        judge_prompt(spec),
        judge_message(spec),
        &spec.cwd,
        "judge",
    )
}

/// The Judge's system prompt: the reply contract, then the criteria.
///
/// The contract leads because it is the part the harness parses. The two
/// tokens come from [`VERDICT_PASS`]/[`VERDICT_FAIL`] rather than being typed
/// out here, so the prompt cannot drift away from what
/// [`crate::runner::parse_verdict`] accepts. One bare token on its own line is
/// deliberately the narrowest thing a model can be asked for — no punctuation,
/// no JSON to get subtly wrong — and the parser fails closed on anything else,
/// so a model that ignores the format cannot accidentally pass work through.
pub fn judge_prompt(spec: &JudgeSpec) -> String {
    format!(
        "You are an independent reviewer. You have no tools: you cannot read files, \
         run commands, or gather any evidence beyond what is in the message below. \
         Judge only what you were given, against the criteria.\n\n\
         Reply in exactly this shape:\n\n\
         ```\n\
         {pass}\n\
         <one or two sentences citing the specific evidence behind the verdict>\n\
         ```\n\n\
         The first line must be the single word `{pass}` or `{fail}` and nothing \
         else. A reply that does not start that way is read as a failure, so do \
         not preface it with anything.\n\n\
         Pass only if every part of the criteria is satisfied by the evidence. \
         Absence of evidence is not satisfaction: if the criteria says something \
         was run and you cannot see that it was run, that is a `{fail}`.\n\n\
         ## Criteria\n\n{criteria}\n",
        pass = VERDICT_PASS,
        fail = VERDICT_FAIL,
        criteria = spec.criteria
    )
}

fn judge_message(spec: &JudgeSpec) -> String {
    let mut message = spec.worker_digest.clone();
    if !spec.artifact_paths.is_empty() {
        message.push_str("\n\nArtifacts:\n");
        for p in &spec.artifact_paths {
            message.push_str(&format!("- {}\n", p.display()));
        }
    }
    // Labelled as the harness's own, because that is exactly what makes it
    // worth more than everything above it: the digest and the artifacts came
    // from the worker, this did not.
    if let Some(output) = &spec.check_output {
        message.push_str(
            "\n\nOutput of the harness's own check for this transition (the worker did not \
             produce this, and it exited zero):\n```\n",
        );
        message.push_str(output);
        message.push_str("\n```\n");
    }
    message
}

/// Same isolation as the Judge, by construction rather than by repetition. It
/// routes within the declared graph or escalates; it never invents structure,
/// because the harness only accepts a reply that exactly names one of the
/// choices it was given.
pub fn navigator_command(pi_bin: &str, spec: &NavigatorSpec) -> Command {
    let mut cmd = isolated_command(
        pi_bin,
        &spec.model,
        navigator_prompt(spec),
        navigator_message(spec),
        &spec.cwd,
        "navigator",
    );
    cmd.env("LOOP_MOCK_STATE", &spec.from);
    cmd
}

/// The choices a Navigator may name: the stuck state's neighbors, plus the
/// always-available [`ESCALATE`] sentinel.
pub fn navigator_choices(spec: &NavigatorSpec) -> Vec<String> {
    let mut choices: Vec<String> = spec.reachable.clone();
    if !choices.iter().any(|c| c == ESCALATE) {
        choices.push(ESCALATE.to_string());
    }
    choices
}

/// The Navigator's system prompt: the reply contract, then the graph.
///
/// Same shape as the Judge's — one bare token on the first line — for the same
/// reason. Here the token set is the reachable states, so
/// [`crate::runner::parse_choice`] can validate by exact lookup rather than by
/// parsing anything.
pub fn navigator_prompt(spec: &NavigatorSpec) -> String {
    let choices = navigator_choices(spec);
    let mut out = String::from(
        "A worker could not route itself out of its stage. Pick where the run goes \
         next, from the list below, and write a short note telling that stage how to \
         get back on track.\n\n\
         Reply in exactly this shape:\n\n\
         ```\n\
         <state>\n\
         <a few sentences: what went wrong, and what to do differently>\n\
         ```\n\n\
         The first line must be one of these names, alone, with nothing else on it:\n\n",
    );
    for choice in &choices {
        if choice == ESCALATE {
            out.push_str(&format!(
                "- `{choice}` — no reachable state fits; hand this to a human\n"
            ));
        } else {
            out.push_str(&format!("- `{choice}`\n"));
        }
    }
    out.push_str(
        "\nA first line naming anything else escalates the run, so pick from the list \
         or pick `escalate` deliberately. Everything after the first line becomes the \
         next stage's note; keep it concrete.\n\n",
    );
    out.push_str(&spec.graph_summary);
    out
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
    use crate::core::{ArtifactClaim, ModelSpec, Proposal, Thinking};
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
            state: "implement".into(),
            cycle: 1,
            attempt: 1,
            model: ModelSpec {
                provider: "anthropic".into(),
                model: "claude-sonnet-5".into(),
                thinking: Thinking::High,
            },
            skill_paths: vec![
                PathBuf::from("/tb/skills/spark-build"),
                PathBuf::from("/tb/skills/jj"),
            ],
            system_prompt_path: PathBuf::from("/tmp/stage-prompt.md"),
            entry_message: "Entering implement, cycle 1".into(),
            mcp: vec!["linear".into()],
            handoff_path: PathBuf::from("/tmp/render/implement-1-1-handoff.json"),
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
                "--no-skills",
                "--skill",
                "/tb/skills/spark-build",
                "--skill",
                "/tb/skills/jj",
                "--append-system-prompt",
                "/tmp/stage-prompt.md",
                "Entering implement, cycle 1",
            ]
        );
        assert_eq!(
            cmd.get_current_dir(),
            Some(std::path::Path::new("/tmp/project"))
        );
    }

    /// Setting `PI_AGENT_DIR` would repoint the `mcp` extension at a
    /// loop-owned directory, hiding every server in the user's own
    /// `~/.pi/agent/mcp.json` — the exact set a stage's `:mcp` names.
    #[test]
    fn worker_command_does_not_repoint_the_agent_dir() {
        let cmd = worker_command("pi", &worker_spec());
        assert_eq!(env_of(&cmd, "PI_AGENT_DIR"), None);
    }

    #[test]
    fn worker_command_env_matches_spec() {
        let cmd = worker_command("pi", &worker_spec());
        assert_eq!(
            env_of(&cmd, HANDOFF_ENV),
            Some("/tmp/render/implement-1-1-handoff.json")
        );
        assert_eq!(env_of(&cmd, "TICKET_ID"), Some("PROJ-1487"));
        assert_eq!(env_of(&cmd, "LOOP_MOCK_ROLE"), Some("worker"));
        assert_eq!(env_of(&cmd, "LOOP_MOCK_STATE"), Some("implement"));
        assert_eq!(env_of(&cmd, "LOOP_MOCK_CYCLE"), Some("1"));
        assert_eq!(env_of(&cmd, "LOOP_MOCK_ATTEMPT"), Some("1"));
    }

    /// No `-e` on any role. The three vendored TypeScript tools are gone, and
    /// with them loop's only dependency on pi's extension ABI — this asserts
    /// that nothing quietly reintroduces one.
    #[test]
    fn no_role_injects_an_extension() {
        for args in [
            args_of(&worker_command("pi", &worker_spec())),
            args_of(&judge_command("pi", &judge_spec())),
            args_of(&navigator_command("pi", &navigator_spec())),
        ] {
            assert!(!args.contains(&"-e".to_string()), "got: {args:?}");
        }
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
            check_output: None,
            model: ModelSpec {
                provider: "anthropic".into(),
                model: "claude-haiku-4-5".into(),
                thinking: Thinking::Low,
            },
            cwd: PathBuf::from("/tmp/project"),
        }
    }

    /// `--no-skills` must be present even when the stage names none: without
    /// it a spawn silently inherits whatever `~/.pi/skills/` happens to hold,
    /// and a stage's instruction set stops being a property of its machine.
    #[test]
    fn worker_with_no_skills_still_pins_discovery_shut() {
        let spec = WorkerSpec {
            skill_paths: vec![],
            ..worker_spec()
        };
        let args = args_of(&worker_command("pi", &spec));
        assert!(args.contains(&"--no-skills".to_string()));
        assert!(!args.contains(&"--skill".to_string()));
    }

    /// Skills are a separate switch from extensions, so the isolated roles have
    /// to turn both off — `--no-extensions` alone leaves skill discovery on.
    #[test]
    fn judge_and_navigator_also_disable_skill_discovery() {
        let judge = args_of(&judge_command("pi", &judge_spec()));
        assert!(judge.contains(&"--no-skills".to_string()));
        let nav = args_of(&navigator_command("pi", &navigator_spec()));
        assert!(nav.contains(&"--no-skills".to_string()));
    }

    /// The Judge has no tools whatsoever — not even one of loop's own. That
    /// is what makes its verdict evidence rather than a second opinion, so
    /// this asserts the absence directly.
    #[test]
    fn judge_command_has_no_tools_at_all() {
        let cmd = judge_command("pi", &judge_spec());
        let args = args_of(&cmd);

        assert!(args.contains(&"--no-builtin-tools".to_string()));
        assert!(args.contains(&"--no-extensions".to_string()));
        assert!(!args.contains(&"-e".to_string()));
        assert!(!args.contains(&"--tools".to_string()));
        assert!(args.contains(&"--no-session".to_string()));
    }

    /// The contract is what the harness parses, so it has to reach the model
    /// ahead of the criteria rather than being appended as an afterthought.
    #[test]
    fn judge_prompt_states_the_contract_and_carries_the_criteria() {
        let prompt = judge_prompt(&judge_spec());
        assert!(prompt.contains("PASS"));
        assert!(prompt.contains("FAIL"));
        assert!(prompt.contains("All three checklist items must be present."));
        let contract_at = prompt.find("first line").expect("states the contract");
        let criteria_at = prompt.find("## Criteria").expect("carries the criteria");
        assert!(contract_at < criteria_at, "contract must lead");
    }

    /// The check's output is the only evidence in the Judge's message the
    /// worker did not author, so it has to arrive labelled as such rather than
    /// blended into the digest.
    #[test]
    fn judge_message_labels_check_output_as_the_harnesss_own() {
        let spec = JudgeSpec {
            check_output: Some("test result: ok. 41 passed".into()),
            ..judge_spec()
        };
        let message = judge_message(&spec);
        assert!(message.contains("test result: ok. 41 passed"));
        assert!(
            message.contains("the worker did not produce this"),
            "got: {message}"
        );
    }

    #[test]
    fn judge_message_omits_the_check_block_when_there_is_no_check() {
        let message = judge_message(&judge_spec());
        assert!(!message.contains("harness's own check"), "got: {message}");
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
            }),
            reachable: vec!["debug".into(), "escalate".into()],
            model: ModelSpec {
                provider: "anthropic".into(),
                model: "claude-haiku-4-5".into(),
                thinking: Thinking::Low,
            },
            cwd: PathBuf::from("/tmp/project"),
        }
    }

    #[test]
    fn navigator_command_is_isolated_and_passes_the_digest() {
        let cmd = navigator_command("pi", &navigator_spec());
        let args = args_of(&cmd);
        assert!(args.contains(&"--no-builtin-tools".to_string()));
        assert!(args.contains(&"--no-extensions".to_string()));

        let message = args.last().unwrap();
        assert!(message.contains("worker stuck at review"));
        assert!(message.contains("no reachable state fits"));
        assert!(message.contains("blocked"));
    }

    /// The choices are what `parse_choice` validates against, so the prompt
    /// and the parser have to be built from the same list — this is the seam
    /// where they could drift apart.
    #[test]
    fn navigator_prompt_lists_every_choice_and_the_graph() {
        let spec = navigator_spec();
        let prompt = navigator_prompt(&spec);
        for choice in navigator_choices(&spec) {
            assert!(prompt.contains(&format!("`{choice}`")), "missing {choice}");
        }
        assert!(prompt.contains("implement -> review -> done"));
    }

    /// `escalate` is always available, even when the machine's own reachable
    /// set never mentions it — it is the Navigator's way out.
    #[test]
    fn navigator_choices_always_include_escalate_exactly_once() {
        let mut spec = navigator_spec();
        spec.reachable = vec!["debug".into()];
        assert_eq!(navigator_choices(&spec), vec!["debug", "escalate"]);

        spec.reachable = vec!["debug".into(), "escalate".into()];
        assert_eq!(navigator_choices(&spec), vec!["debug", "escalate"]);
    }
}
