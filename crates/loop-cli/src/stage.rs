//! The `StageBuilder` the engine drives.
//!
//! This is where the engine's abstract "run state X, cycle N" becomes concrete
//! files and flags: a playbook resolved and rendered, a context pack assembled
//! from the ledger, a `pi` invocation described. The engine deliberately knows
//! none of it.

use std::path::PathBuf;

use loop_core::{
    ArtifactRef, Config, Context, JudgeSpec, Machine, ModelSpec, NavigatorSpec, Proposal, Result,
    State, StateId, WorkerSpec,
};
use loop_engine::{StageBuilder, StagePlan};
use loop_ledger::{Ledger, digest};
use loop_toolbox::{ExtPaths, ResolvedPlaybook, Toolbox, frontmatter_model, render};

/// What a state resolves to before any ledger is read or any file is written:
/// the four-layer model, the local-first playbook, and the effective skill and
/// MCP sets.
pub struct Resolved<'a> {
    pub state: &'a State,
    pub playbook: ResolvedPlaybook,
    pub model: ModelSpec,
    /// Skill *names*, as the ledger records them.
    pub skills: Vec<String>,
    /// The same skills as the paths pi is handed, in the same order.
    pub skill_paths: Vec<PathBuf>,
    pub mcp: Vec<String>,
}

/// The read-only half of stage building.
///
/// Everything here is a pure function of the machine, the config, and the
/// toolbox on disk — it reads files, and writes and creates none. Both
/// [`CliStage::build_stage`] and `loop preview` go through it, so the layered
/// model rules and local-first resolution have exactly one implementation.
pub struct Resolver<'a> {
    pub machine: &'a Machine,
    pub config: &'a Config,
    pub toolbox: Toolbox<'a>,
}

impl<'a> Resolver<'a> {
    pub fn new(machine: &'a Machine, config: &'a Config) -> Self {
        Self {
            machine,
            config,
            toolbox: Toolbox::new(config),
        }
    }

    pub fn resolve(&self, state_id: &StateId) -> Result<Resolved<'a>> {
        let state = self
            .machine
            .state(state_id)
            .ok_or_else(|| loop_core::CoreError::machine(format!("no such state: {state_id}")))?;
        let playbook = self
            .toolbox
            .resolve_playbook(&state.playbook, &self.machine.dir)?;
        let model =
            self.machine
                .resolve_model(state, &frontmatter_model(&playbook), &self.config.worker);
        let skills = self
            .machine
            .resolve_skills(state, &self.config.default_skills);
        let skill_paths = self.toolbox.resolve_skills(&skills, &self.machine.dir)?;
        let mcp = self.machine.resolve_mcp(state, &self.config.default_mcp);

        Ok(Resolved {
            state,
            playbook,
            model,
            skills,
            skill_paths,
            mcp,
        })
    }

    /// The deterministic session id, so a crashed stage's transcript stays
    /// findable in pi's session store.
    pub fn session_id(&self, state: &str, cycle: u32, attempt: u32) -> String {
        let slug = |s: &str| {
            s.chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                .collect::<String>()
        };
        format!(
            "{}-{}-{}-{}",
            slug(&self.machine.ticket),
            slug(state),
            cycle,
            attempt
        )
    }

    /// The small, scalar context values that land in a spawn's environment,
    /// where a stage's tooling reads them to key its idempotency on the cycle
    /// (docs/03-customizing.md).
    pub fn env(ctx: &Context) -> Vec<(String, String)> {
        [
            ("TICKET_ID", ctx.ticket_id.clone()),
            ("STATE", ctx.state.clone()),
            ("CYCLE", ctx.cycle.to_string()),
            ("ATTEMPT", ctx.attempt.to_string()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect()
    }
}

pub struct CliStage<'a> {
    pub machine: &'a Machine,
    pub config: &'a Config,
    pub toolbox: Toolbox<'a>,
    pub ext: ExtPaths,
    /// Read-only handle. The engine holds the writable one, so this opens the
    /// same file independently rather than aliasing it — every append is
    /// fsynced, so these reads are always current.
    pub ledger_path: PathBuf,
}

impl CliStage<'_> {
    fn events(&self) -> Result<Vec<loop_core::Event>> {
        use loop_core::LedgerSink;
        Ledger::open(&self.ledger_path)?.read_all()
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
        let folded =
            loop_core::fold_with_loop_heads(&events, &|s| self.machine.loop_with_head(s).is_some());
        let prev_state = events.iter().rev().find_map(|e| match &e.payload {
            loop_core::EventPayload::TransitionCommitted { from, .. } => Some(from.clone()),
            _ => None,
        });
        Ok(Context {
            ticket_id: self.machine.ticket.clone(),
            task: self.machine.task.clone(),
            plan: self.machine.plan.clone(),
            state: state.clone(),
            prev_state,
            cycle,
            attempt,
            crashed,
            ledger_digest: digest::render(&events, self.config.digest_last_n),
            entry_addendum: entry_addendum.map(str::to_string),
            qa_cases: self.machine.qa_cases.clone(),
            artifacts: folded.artifacts.clone(),
        })
    }

    /// The graph, rendered for the Navigator: every state's purpose plus the
    /// edges out of the stuck one, so it routes within declared structure
    /// instead of inventing it.
    fn graph_summary(&self, from: &str) -> String {
        let mut out = String::from("## States\n\n");
        for (id, st) in &self.machine.states {
            let desc = st.description.as_deref().unwrap_or("(no description)");
            out.push_str(&format!("- `{id}` — {desc}\n"));
        }
        for t in &self.machine.terminals {
            out.push_str(&format!("- `{t}` — terminal\n"));
        }
        out.push_str(&format!("\n## Edges out of `{from}`\n\n"));
        for e in self.machine.edges_from(from) {
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

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("").trim()
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
        let resolver = Resolver::new(self.machine, self.config);
        let resolved = resolver.resolve(state_id)?;

        let context = self.context(state_id, cycle, attempt, entry_addendum, crashed)?;
        let vars = context.to_map();
        let body = render::substitute(&resolved.playbook.body, &vars);
        let system_prompt_path = self.toolbox.write_rendered(&context, &body, "system")?;

        let spec = WorkerSpec {
            ticket: self.machine.ticket.clone(),
            state: state_id.clone(),
            cycle,
            attempt,
            model: resolved.model,
            skill_paths: resolved.skill_paths,
            system_prompt_path,
            entry_message: render::entry_message(&context, &resolved.mcp),
            reachable: self.machine.neighbors(state_id),
            transition_mode: self.machine.transition_mode,
            mcp: resolved.mcp,
            ext_paths: vec![self.ext.transition.clone()],
            cwd: self.config.paths.project_dir.clone(),
            session_id: Some(resolver.session_id(state_id, cycle, attempt)),
            env: Resolver::env(&context),
        };
        Ok(StagePlan {
            spec,
            context,
            skills: resolved.skills,
        })
    }

    fn build_judge(
        &self,
        criteria: &str,
        worker_summary: &str,
        artifacts: &[ArtifactRef],
        check_output: Option<&str>,
    ) -> Result<JudgeSpec> {
        Ok(JudgeSpec {
            criteria: criteria.to_string(),
            worker_digest: digest::worker_digest_for_judge(worker_summary, artifacts),
            artifact_paths: artifacts
                .iter()
                .map(|a| self.config.paths.project_dir.join(&a.path))
                .collect(),
            check_output: check_output.map(str::to_string),
            model: self.config.judge.clone(),
            ext_path: self.ext.verdict.clone(),
            cwd: self.config.paths.project_dir.clone(),
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
            ledger_digest: digest::render(&events, self.config.digest_last_n),
            from: from.clone(),
            proposal: proposal.cloned(),
            reachable: self.machine.neighbors(from),
            model: self.config.navigator.clone(),
            ext_path: self.ext.choose.clone(),
            cwd: self.config.paths.project_dir.clone(),
        })
    }
}

impl loop_core::CheckRunner for CliStage<'_> {
    /// Substitute the context namespace into the check's command, then shell
    /// out in the project root.
    ///
    /// The environment is the same small scalar pack a worker spawn gets, so a
    /// check can key on the cycle the way a stage's tooling does — an
    /// idempotent re-check of `loop-$TICKET_ID-$CYCLE` looks at exactly the
    /// namespace the stage just deployed to.
    fn run_check(
        &self,
        check: &loop_core::Check,
        from: &StateId,
        cycle: u32,
        attempt: u32,
    ) -> Result<loop_core::CheckOutcome> {
        let context = self.context(from, cycle, attempt, None, false)?;
        let cmd = render::substitute(&check.cmd, &context.to_map());
        let env = Resolver::env(&context);

        loop_runner::exec_check(&cmd, &self.config.paths.project_dir, &env, check.timeout_s)
    }
}
