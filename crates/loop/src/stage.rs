//! The `StageBuilder` the engine drives.
//!
//! This is where the engine's abstract "run state X, cycle N" becomes concrete
//! files and flags: a stage prompt resolved and rendered, a context pack assembled
//! from the ledger, a `pi` invocation described. The engine deliberately knows
//! none of it.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::core::text::first_line;
use crate::core::{
    Artifact, Context, JudgeSpec, Machine, ModelSpec, NavigatorSpec, Paths, Proposal, Result,
    State, StateId, WorkerSpec, sanitize_component,
};
use crate::engine::{StageBuilder, StagePlan};
use crate::ledger::{self, digest};
use crate::toolbox::{self, ResolvedStagePrompt, frontmatter_model, render};

/// What a state resolves to before any ledger is read or any file is written:
/// the four-layer model, the local-first stage prompt, and the effective skill and
/// MCP sets.
pub struct Resolved<'a> {
    pub state: &'a State,
    pub stage_prompt: ResolvedStagePrompt,
    pub model: ModelSpec,
    /// Skill *names*, as the ledger records them.
    pub skills: Vec<String>,
    /// The same skills as the paths pi is handed, in the same order.
    pub skill_paths: Vec<PathBuf>,
    pub mcp: Vec<String>,
}

/// The read-only half of stage building.
///
/// Everything here is a pure function of the machine, the paths, and the
/// toolbox on disk — it reads files, and writes and creates none. Both
/// [`CliStage::build_stage`] and `loop preview` go through it, so the layered
/// model rules and local-first resolution have exactly one implementation.
pub struct Resolver<'a> {
    pub machine: &'a Machine,
    pub paths: &'a Paths,
}

impl<'a> Resolver<'a> {
    pub fn new(machine: &'a Machine, paths: &'a Paths) -> Self {
        Self { machine, paths }
    }

    pub fn resolve(&self, state_id: &StateId) -> Result<Resolved<'a>> {
        let state = self
            .machine
            .state(state_id)
            .ok_or_else(|| crate::core::CoreError::machine(format!("no such state: {state_id}")))?;
        let stage_prompt = toolbox::resolve_stage_prompt(&state.stage_prompt, &self.machine.dir)?;
        let model = self
            .machine
            .resolve_model(state, &frontmatter_model(&stage_prompt));
        let skills = self.machine.resolve_skills(state);
        let skill_paths = toolbox::resolve_skills(&skills, &self.machine.dir)?;
        let mcp = self.machine.resolve_mcp(state);

        Ok(Resolved {
            state,
            stage_prompt,
            model,
            skills,
            skill_paths,
            mcp,
        })
    }

    /// The deterministic session id, so a crashed stage's transcript stays
    /// findable in pi's session store.
    pub fn session_id(&self, state: &str, cycle: u32, attempt: u32) -> String {
        format!(
            "{}-{}-{}-{}",
            sanitize_component(&self.machine.ticket, "ticket"),
            sanitize_component(state, "state"),
            cycle,
            attempt
        )
    }
}

/// The context variables that also travel as environment, where a stage's
/// tooling reads them to key its idempotency on the cycle
/// (docs/03-customizing.md).
///
/// Named once, here. This list used to exist three times — built by hand in
/// `Resolver::env`, again (as a superset) by [`Context::to_map`], and a third
/// time as the literal string `loop preview` printed — so adding a fifth
/// variable made preview quietly lie about what a spawn receives.
pub const ENV_VARS: [&str; 4] = ["TICKET_ID", "STATE", "CYCLE", "ATTEMPT"];

/// [`ENV_VARS`], picked out of a substitution map.
///
/// Takes the map rather than the [`Context`] so a spawn cannot receive a
/// different value than the stage prompt interpolated: both callers have
/// already built one for `render::substitute`, and reading the environment out
/// of that exact map makes the two agree by construction rather than because
/// [`Context::to_map`] is deterministic. Building a second map here also meant
/// cloning `TASK`, `PLAN`, the whole rendered `LEDGER_DIGEST` and `QA_CASES` —
/// every large string in the process — to keep four scalars, twice per check.
pub fn env(vars: &BTreeMap<String, String>) -> Vec<(String, String)> {
    ENV_VARS
        .iter()
        .filter_map(|k| vars.get(*k).map(|v| (k.to_string(), v.clone())))
        .collect()
}

pub struct CliStage<'a> {
    /// The read-only half, held rather than rebuilt. It *is* the machine and
    /// the paths — `CliStage` used to carry those two fields and construct a
    /// `Resolver` out of them on every `build_stage`.
    pub resolver: Resolver<'a>,
    /// Read-only handle. The engine holds the writable one, so this opens the
    /// same file independently rather than aliasing it — every append is
    /// fsynced, so these reads are always current.
    pub ledger_path: PathBuf,
}

impl<'a> CliStage<'a> {
    pub fn new(machine: &'a Machine, paths: &'a Paths, ledger_path: PathBuf) -> Self {
        Self {
            resolver: Resolver::new(machine, paths),
            ledger_path,
        }
    }

    fn machine(&self) -> &'a Machine {
        self.resolver.machine
    }

    fn paths(&self) -> &'a Paths {
        self.resolver.paths
    }

    fn events(&self) -> Result<Vec<crate::core::Event>> {
        ledger::events(&self.ledger_path)
    }

    /// The context pack: task, plan, digest, artifacts — the engineered
    /// continuity that replaces shared chat memory between stages.
    fn context(
        &self,
        state: &StateId,
        cycle: u32,
        attempt: u32,
        entry_addendum: Option<&str>,
        crashed: bool,
    ) -> Result<Context> {
        let events = self.events()?;
        let folded = self.machine().fold(&events);
        Ok(Context {
            ticket_id: self.machine().ticket.clone(),
            task: self.machine().task.clone(),
            plan: self.machine().plan.clone(),
            state: state.clone(),
            prev_state: folded.prev_state.clone(),
            cycle,
            attempt,
            crashed,
            ledger_digest: digest::render(&events, self.machine().digest_last_n),
            entry_addendum: entry_addendum.map(str::to_string),
            qa_cases: self.machine().qa_cases.clone(),
            artifacts: folded.artifacts.clone(),
        })
    }

    /// The graph, rendered for the Navigator: every state's purpose plus the
    /// edges out of the stuck one, so it routes within declared structure
    /// instead of inventing it.
    fn graph_summary(&self, from: &str) -> String {
        let mut out = String::from("## States\n\n");
        for (id, st) in &self.machine().states {
            let desc = st.description.as_deref().unwrap_or("(no description)");
            out.push_str(&format!("- `{id}` — {desc}\n"));
        }
        for t in &self.machine().terminals {
            out.push_str(&format!("- `{t}` — terminal\n"));
        }
        out.push_str(&format!("\n## Edges out of `{from}`\n\n"));
        for e in self.machine().edges_from(from) {
            // Both tiers, so the Navigator can see that an edge is gated on a
            // command it cannot talk its way past, not just on a criterion.
            let mut guards = Vec::new();
            if let Some(c) = &e.check {
                guards.push(format!("check: `{}`", first_line(&c.cmd)));
            }
            if let Some(c) = &e.criteria {
                guards.push(format!("criteria: {}", first_line(c)));
            }
            let guard = if guards.is_empty() {
                String::new()
            } else {
                format!(" ({})", guards.join("; "))
            };
            out.push_str(&format!("- `{from}` → `{}`{guard}\n", e.to));
        }
        out
    }
}

impl StageBuilder for CliStage<'_> {
    fn build_stage(
        &self,
        state_id: &StateId,
        cycle: u32,
        attempt: u32,
        entry_addendum: Option<&str>,
        crashed: bool,
    ) -> Result<StagePlan> {
        let resolved = self.resolver.resolve(state_id)?;

        let context = self.context(state_id, cycle, attempt, entry_addendum, crashed)?;
        let vars = context.to_map();
        let reachable = self.machine().neighbors(state_id);
        let handoff_path = self.paths().handoff_file(state_id, cycle, attempt);

        // The protocol block is appended *after* substitution, and is not
        // itself a template — a stage prompt must not be able to interpolate its
        // way into changing how the stage ends, and the handoff path is the
        // harness's own value rather than one from the context namespace.
        let mut body = render::substitute(&resolved.stage_prompt.body, &vars);
        body.push_str(&crate::runner::reply::handoff_protocol(
            &handoff_path,
            &reachable,
        ));
        let system_prompt_path = toolbox::write_rendered(self.paths(), &context, &body, "system")?;

        let spec = WorkerSpec {
            state: state_id.clone(),
            cycle,
            attempt,
            model: resolved.model,
            skill_paths: resolved.skill_paths,
            system_prompt_path,
            entry_message: render::entry_message(&context, &resolved.mcp),
            handoff_path,
            cwd: self.paths().project_dir.clone(),
            session_id: Some(self.resolver.session_id(state_id, cycle, attempt)),
            env: env(&vars),
        };
        Ok(StagePlan {
            spec,
            skills: resolved.skills,
            mcp: resolved.mcp,
        })
    }

    fn build_judge(
        &self,
        criteria: &str,
        worker_summary: &str,
        artifacts: &[Artifact],
        check_output: Option<&str>,
    ) -> Result<JudgeSpec> {
        Ok(JudgeSpec {
            criteria: criteria.to_string(),
            worker_digest: crate::core::worker_digest_for_judge(worker_summary, artifacts),
            check_output: check_output.map(str::to_string),
            model: self.machine().judge.clone(),
            cwd: self.paths().project_dir.clone(),
        })
    }

    fn build_navigator(
        &self,
        from: &StateId,
        proposal: Option<&Proposal>,
    ) -> Result<NavigatorSpec> {
        let events = self.events()?;
        Ok(NavigatorSpec {
            graph_summary: self.graph_summary(from),
            ledger_digest: digest::render(&events, self.machine().digest_last_n),
            from: from.clone(),
            proposal: proposal.cloned(),
            reachable: self.machine().neighbors(from),
            model: self.machine().navigator.clone(),
            cwd: self.paths().project_dir.clone(),
        })
    }
}

impl crate::core::CheckRunner for CliStage<'_> {
    /// Substitute the context namespace into the check's command, then shell
    /// out in the project root.
    ///
    /// The environment is the same small scalar pack a worker spawn gets, so a
    /// check can key on the cycle the way a stage's tooling does — an
    /// idempotent re-check of `loop-$TICKET_ID-$CYCLE` looks at exactly the
    /// namespace the stage just deployed to.
    fn run_check(
        &self,
        check: &crate::core::Check,
        from: &StateId,
        cycle: u32,
        attempt: u32,
    ) -> Result<crate::core::CheckOutcome> {
        let context = self.context(from, cycle, attempt, None, false)?;
        let vars = context.to_map();
        let cmd = render::substitute(&check.cmd, &vars);
        let env = env(&vars);

        crate::runner::exec_check(&cmd, &self.paths().project_dir, &env, check.timeout_s)
    }
}
