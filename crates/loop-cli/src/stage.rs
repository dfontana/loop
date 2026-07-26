//! The `StageBuilder` the engine drives.
//!
//! This is where the engine's abstract "run state X, cycle N" becomes concrete
//! files and flags: a playbook resolved and rendered, a context pack assembled
//! from the ledger, a `pi` invocation described. The engine deliberately knows
//! none of it.

use std::path::PathBuf;

use loop_core::{
    ArtifactRef, Config, Context, JudgeSpec, Machine, NavigatorSpec, Proposal, Result, StateId,
    WorkerSpec,
};
use loop_engine::{StageBuilder, StagePlan};
use loop_ledger::{Ledger, digest};
use loop_toolbox::{ExtPaths, Toolbox, frontmatter_model, render};

pub struct CliStage<'a> {
    pub machine: &'a Machine,
    pub config: &'a Config,
    pub toolbox: Toolbox<'a>,
    pub agent_dir: PathBuf,
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
            ledger_digest: digest::render(&events, self.config.digest_last_n),
            entry_addendum: entry_addendum.map(str::to_string),
            qa_cases: self.machine.qa_cases.clone(),
            artifacts: folded.artifacts.clone(),
        })
    }

    /// A deterministic session id, so a crashed stage's transcript stays
    /// findable in pi's session store.
    fn session_id(&self, state: &str, cycle: u32, attempt: u32) -> String {
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
    ) -> Result<StagePlan> {
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
        let tools = self
            .machine
            .resolve_tools(state, &self.config.default_tools);

        let context = self.context(state_id, cycle, attempt, entry_addendum)?;
        let vars = context.to_map();
        let body = render::substitute(&playbook.body, &vars);
        let system_prompt_path = self.toolbox.write_rendered(&context, &body, "system")?;

        // Small, scalar context values only: these land in the spawn's
        // environment, where a stage's tooling reads them to key its
        // idempotency on the cycle (docs/04).
        let env = [
            ("TICKET_ID", context.ticket_id.clone()),
            ("STATE", context.state.clone()),
            ("CYCLE", context.cycle.to_string()),
            ("ATTEMPT", context.attempt.to_string()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();

        let spec = WorkerSpec {
            ticket: self.machine.ticket.clone(),
            state: state_id.clone(),
            cycle,
            attempt,
            model,
            tools,
            exclude_tools: state.exclude_tools.clone(),
            system_prompt_path,
            entry_message: render::entry_message(&context),
            reachable: self.machine.neighbors(state_id),
            transition_mode: self.machine.transition_mode,
            agent_dir: self.agent_dir.clone(),
            ext_paths: vec![self.ext.transition.clone()],
            pi_extensions: self.config.pi_extensions.clone(),
            cwd: self.config.paths.project_dir.clone(),
            session_id: Some(self.session_id(state_id, cycle, attempt)),
            env,
        };
        Ok(StagePlan { spec, context })
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
        let context = self.context(from, cycle, attempt, None)?;
        let cmd = render::substitute(&check.cmd, &context.to_map());
        let env: Vec<(String, String)> = [
            ("TICKET_ID", context.ticket_id.clone()),
            ("STATE", context.state.clone()),
            ("CYCLE", context.cycle.to_string()),
            ("ATTEMPT", context.attempt.to_string()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();

        loop_runner::exec_check(&cmd, &self.config.paths.project_dir, &env, check.timeout_s)
    }
}
