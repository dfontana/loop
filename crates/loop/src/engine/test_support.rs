//! In-process fakes for the engine's collaborator traits, plus small machine
//! fixture helpers. No Lua, no subprocess, no filesystem, no API key — every
//! trait the engine depends on is faked here.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::PathBuf;

use crate::core::{
    AgentRunner, Artifact, ArtifactSink, Budgets, Check, CheckOutcome, CheckRunner, Choice,
    Context, CoreError, Defaults, Event, EventPayload, JudgeSpec, LoopSpec, Machine, ModelChoice,
    ModelSpec, NavigatorSpec, OnExhausted, OnFail, Proposal, QaCase, Result, StagePromptRef, State,
    StateId, Thinking, Transition, Usage, Verdict, WorkerResult, WorkerSpec,
};

use crate::engine::prompts::{StageBuilder, StagePlan};

// ── machine fixtures ─────────────────────────────────────────────────────

pub fn model_spec() -> ModelSpec {
    ModelSpec {
        provider: "test".into(),
        model: "test-model".into(),
        thinking: Thinking::Low,
    }
}

/// A machine with no states/transitions and unlimited budgets. Tests fill in
/// what they need via direct field assignment.
pub fn base_machine() -> Machine {
    Machine {
        ticket: "T-1".into(),
        task: String::new(),
        plan: String::new(),
        qa_cases: Vec::<QaCase>::new(),
        entry: "start".into(),
        terminals: BTreeSet::new(),
        escalation_state: None,
        defaults: Defaults::default(),
        states: BTreeMap::new(),
        transitions: Vec::new(),
        loops: Vec::new(),
        budgets: Budgets::default(),
        judge: model_spec(),
        navigator: model_spec(),
        worker: model_spec(),
        navigator_max_invocations: 5,
        digest_last_n: 8,
        pi_extensions: vec!["mcp".into()],
        source_hash: crate::core::machine_hash("(test machine)"),
        source_path: PathBuf::from("machine.fnl"),
        dir: PathBuf::from("."),
    }
}

pub fn state(id: &str) -> State {
    State {
        id: id.into(),
        stage_prompt: StagePromptRef::Inline("test stage prompt".into()),
        model: ModelChoice::default(),
        skills: Vec::new(),
        mcp: Vec::new(),
        description: None,
    }
}

pub fn state_with_skills(id: &str, skills: &[&str]) -> State {
    State {
        skills: skills.iter().map(|s| s.to_string()).collect(),
        ..state(id)
    }
}

pub fn state_with_mcp(id: &str, mcp: &[&str]) -> State {
    State {
        mcp: mcp.iter().map(|s| s.to_string()).collect(),
        ..state(id)
    }
}

/// A plain, unconditional edge.
pub fn edge(from: &str, to: &str) -> Transition {
    Transition {
        from: from.into(),
        to: to.into(),
        check: None,
        criteria: None,
        on_fail: OnFail::default(),
        backoff_s: None,
        max_attempts: crate::core::Floor::default().transition_max_attempts,
    }
}

pub fn judged_edge(from: &str, to: &str, criteria: &str) -> Transition {
    Transition {
        criteria: Some(criteria.into()),
        ..edge(from, to)
    }
}

/// An edge gated by a deterministic check the harness runs.
pub fn checked_edge(from: &str, to: &str, cmd: &str) -> Transition {
    Transition {
        check: Some(Check::new(cmd)),
        ..edge(from, to)
    }
}

pub fn loop_spec(
    name: &str,
    states: &[&str],
    max_cycles: u32,
    on_exhausted: OnExhausted,
) -> LoopSpec {
    LoopSpec {
        name: name.into(),
        states: states.iter().map(|s| s.to_string()).collect(),
        max_cycles,
        on_exhausted,
    }
}

// ── machine builder ───────────────────────────────────────────────────────

/// Assemble a test machine from its shape rather than its field assignments.
///
/// [`Build::entry`] and [`Build::edge`] insert any state they name, so a test
/// cannot describe an unreachable state or a missing terminal by accident.
pub struct Build(Machine);

pub fn machine() -> Build {
    Build(base_machine())
}

impl Build {
    /// The entry state, inserted if it isn't already there.
    pub fn entry(mut self, id: &str) -> Self {
        self.0.entry = id.into();
        self.ensure(id);
        self
    }

    pub fn state(mut self, id: &str) -> Self {
        self.ensure(id);
        self
    }

    /// A state built by hand — [`state_with_skills`], [`state_with_mcp`].
    pub fn with(mut self, s: State) -> Self {
        self.0.states.insert(s.id.clone(), s);
        self
    }

    pub fn terminal(mut self, id: &str) -> Self {
        self.0.terminals.insert(id.into());
        self
    }

    /// A terminal that is also where escalation lands.
    pub fn escalate_to(mut self, id: &str) -> Self {
        self.0.terminals.insert(id.into());
        self.0.escalation_state = Some(id.into());
        self
    }

    /// An edge, plus any endpoint that is neither already a state nor a
    /// declared terminal. Keeps a test from silently describing a machine whose
    /// own linter would reject it.
    pub fn edge(mut self, t: Transition) -> Self {
        for id in [t.from.clone(), t.to.clone()] {
            if !self.0.terminals.contains(&id) {
                self.ensure(&id);
            }
        }
        self.0.transitions.push(t);
        self
    }

    pub fn loop_over(mut self, l: LoopSpec) -> Self {
        self.0.loops.push(l);
        self
    }

    pub fn budget_usd(mut self, usd: f64) -> Self {
        self.0.budgets.usd = Some(usd);
        self
    }

    pub fn budget_transitions(mut self, n: u32) -> Self {
        self.0.budgets.max_transitions = Some(n);
        self
    }

    pub fn budget_wallclock_s(mut self, s: u64) -> Self {
        self.0.budgets.wallclock_s = Some(s);
        self
    }

    pub fn navigator_cap(mut self, n: u32) -> Self {
        self.0.navigator_max_invocations = n;
        self
    }

    pub fn default_skills(mut self, names: &[&str]) -> Self {
        self.0.defaults.skills = names.iter().map(|s| s.to_string()).collect();
        self
    }

    pub fn default_mcp(mut self, names: &[&str]) -> Self {
        self.0.defaults.mcp = names.iter().map(|s| s.to_string()).collect();
        self
    }

    /// Drop every declared pi-extension, so a stage naming MCP servers has no
    /// tool to connect them with.
    pub fn without_extensions(mut self) -> Self {
        self.0.pi_extensions.clear();
        self
    }

    pub fn build(self) -> Machine {
        self.0
    }

    fn ensure(&mut self, id: &str) {
        self.0
            .states
            .entry(id.to_string())
            .or_insert_with(|| state(id));
    }
}

// ── CheckRunner fake ──────────────────────────────────────────────────────

/// Scriptable stand-in for the harness's own subprocess. Queued outcomes are
/// consumed in order; an empty queue passes, which keeps every test that
/// doesn't care about checks free of setup.
#[derive(Default)]
pub struct FakeChecks {
    queued: RefCell<VecDeque<CheckOutcome>>,
    /// Every command the engine asked for, in order — so a test can assert the
    /// check ran at all, and ran with the right substitutions.
    pub ran: RefCell<Vec<(String, StateId, u32, u32)>>,
}

impl FakeChecks {
    pub fn script(&self, outcome: CheckOutcome) {
        self.queued.borrow_mut().push_back(outcome);
    }

    pub fn script_pass(&self, output: &str) {
        self.script(CheckOutcome {
            passed: true,
            output: output.into(),
        });
    }

    pub fn script_fail(&self, output: &str) {
        self.script(CheckOutcome {
            passed: false,
            output: output.into(),
        });
    }

    pub fn commands(&self) -> Vec<String> {
        self.ran.borrow().iter().map(|(c, ..)| c.clone()).collect()
    }
}

impl CheckRunner for FakeChecks {
    fn run_check(
        &self,
        check: &Check,
        from: &StateId,
        cycle: u32,
        attempt: u32,
    ) -> Result<CheckOutcome> {
        self.ran
            .borrow_mut()
            .push((check.cmd.clone(), from.clone(), cycle, attempt));
        Ok(self
            .queued
            .borrow_mut()
            .pop_front()
            .unwrap_or(CheckOutcome {
                passed: true,
                output: String::new(),
            }))
    }
}

// ── LedgerSink fake ───────────────────────────────────────────────────────

#[derive(Default)]
pub struct FakeLedger {
    pub events: Vec<Event>,
    /// How many times the engine has read the whole log back.
    ///
    /// Against a real `Ledger` that is a syscall and a full re-parse of a file
    /// that grows all run, so it is worth being able to assert on.
    reads: std::cell::Cell<usize>,
}

impl crate::core::LedgerSink for FakeLedger {
    fn append(&mut self, payload: EventPayload) -> Result<Event> {
        let e = Event::stamped(payload, 0);
        self.events.push(e.clone());
        Ok(e)
    }

    fn read_all(&self) -> Result<Vec<Event>> {
        self.reads.set(self.reads.get() + 1);
        Ok(self.events.clone())
    }
}

/// One `guard_checked`, projected — what a test asserting on the guard tiers
/// actually reads.
#[derive(Clone, Debug)]
pub struct Guard {
    pub check: crate::core::GuardOutcome,
    pub criteria: crate::core::GuardOutcome,
    pub check_output: Option<String>,
    pub usage: Usage,
}

impl FakeLedger {
    /// A ledger that already holds these events — a run being resumed.
    pub fn holding(events: Vec<Event>) -> Self {
        Self {
            events,
            ..Self::default()
        }
    }

    pub fn reads(&self) -> usize {
        self.reads.get()
    }

    pub fn kinds(&self) -> Vec<&'static str> {
        self.events.iter().map(|e| e.kind()).collect()
    }

    pub fn count_of(&self, kind: &str) -> usize {
        self.events.iter().filter(|e| e.kind() == kind).count()
    }

    // ── projections ───────────────────────────────────────────────────────
    //
    // The variant is already known from the event kind, so the match belongs
    // here rather than in every test that wants one field off one event.

    /// `(state, cycle, attempt)` per `state_entered`, in order.
    pub fn entered(&self) -> Vec<(String, u32, u32)> {
        self.events
            .iter()
            .filter_map(|e| match &e.payload {
                EventPayload::StateEntered(h) => Some((h.state.clone(), h.cycle, h.attempt)),
                _ => None,
            })
            .collect()
    }

    pub fn attempts(&self) -> Vec<u32> {
        self.entered().into_iter().map(|(_, _, a)| a).collect()
    }

    pub fn state_cycles(&self) -> Vec<(String, u32)> {
        self.entered().into_iter().map(|(s, c, _)| (s, c)).collect()
    }

    pub fn guards(&self) -> Vec<Guard> {
        self.events
            .iter()
            .filter_map(|e| match &e.payload {
                EventPayload::GuardChecked {
                    check,
                    criteria,
                    check_output,
                    usage,
                    ..
                } => Some(Guard {
                    check: *check,
                    criteria: *criteria,
                    check_output: check_output.clone(),
                    usage: *usage,
                }),
                _ => None,
            })
            .collect()
    }

    /// `(from, to)` per `transition_committed`, in order.
    pub fn commits(&self) -> Vec<(String, String)> {
        self.events
            .iter()
            .filter_map(|e| match &e.payload {
                EventPayload::TransitionCommitted { from, to, .. } => {
                    Some((from.clone(), to.clone()))
                }
                _ => None,
            })
            .collect()
    }

    /// The detail of each `error`, in order.
    pub fn errors(&self) -> Vec<String> {
        self.events
            .iter()
            .filter_map(|e| match &e.payload {
                EventPayload::Error { detail, .. } => Some(detail.clone()),
                _ => None,
            })
            .collect()
    }

    /// The artifact *names* recorded on each `worker_output`, in order.
    pub fn artifact_names(&self) -> Vec<Vec<String>> {
        self.events
            .iter()
            .filter_map(|e| match &e.payload {
                EventPayload::WorkerOutput { artifacts, .. } => {
                    Some(artifacts.iter().map(|a| a.name.clone()).collect())
                }
                _ => None,
            })
            .collect()
    }
}

// ── ArtifactSink fake ─────────────────────────────────────────────────────

#[derive(Default)]
pub struct FakeArtifacts;

impl ArtifactSink for FakeArtifacts {
    fn capture(&self, state: &str, cycle: u32, claim: &Artifact) -> Result<Artifact> {
        Ok(Artifact {
            name: claim.name.clone(),
            path: format!(".loop/artifacts/{state}-{cycle}-{}", claim.name),
        })
    }
}

// ── AgentRunner fake ──────────────────────────────────────────────────────

#[derive(Default)]
pub struct FakeRunner {
    worker: RefCell<HashMap<StateId, VecDeque<WorkerResult>>>,
    judge: RefCell<VecDeque<Verdict>>,
    navigator: RefCell<VecDeque<Choice>>,
    pub judge_calls: RefCell<Vec<JudgeSpec>>,
    pub navigator_calls: RefCell<Vec<NavigatorSpec>>,
    pub worker_calls: RefCell<Vec<WorkerSpec>>,
}

impl FakeRunner {
    pub fn script_worker(&self, state: &str, result: WorkerResult) {
        self.worker
            .borrow_mut()
            .entry(state.to_string())
            .or_default()
            .push_back(result);
    }

    pub fn script_judge(&self, v: Verdict) {
        self.judge.borrow_mut().push_back(v);
    }

    pub fn script_navigator(&self, c: Choice) {
        self.navigator.borrow_mut().push_back(c);
    }
}

impl AgentRunner for FakeRunner {
    fn run_worker(&self, spec: &WorkerSpec) -> Result<WorkerResult> {
        self.worker_calls.borrow_mut().push(spec.clone());
        let mut m = self.worker.borrow_mut();
        m.entry(spec.state.clone())
            .or_default()
            .pop_front()
            .ok_or_else(|| {
                CoreError::other(format!(
                    "no scripted worker result for state `{}`",
                    spec.state
                ))
            })
    }

    fn run_judge(&self, spec: &JudgeSpec) -> Result<Verdict> {
        self.judge_calls.borrow_mut().push(spec.clone());
        self.judge
            .borrow_mut()
            .pop_front()
            .ok_or_else(|| CoreError::other("no scripted judge verdict"))
    }

    fn run_navigator(&self, spec: &NavigatorSpec) -> Result<Choice> {
        self.navigator_calls.borrow_mut().push(spec.clone());
        self.navigator
            .borrow_mut()
            .pop_front()
            .ok_or_else(|| CoreError::other("no scripted navigator choice"))
    }
}

// ── StageBuilder fake ─────────────────────────────────────────────────────

pub struct FakeStageBuilder<'m> {
    pub machine: &'m Machine,
    /// The context handed to every stage this builder assembled, in order —
    /// how a test asserts on what actually reached the stage prompt rather than on
    /// what the engine meant to send.
    pub contexts: RefCell<Vec<Context>>,
}

impl<'m> FakeStageBuilder<'m> {
    pub fn new(machine: &'m Machine) -> Self {
        Self {
            machine,
            contexts: RefCell::new(Vec::new()),
        }
    }
}

impl<'m> StageBuilder for FakeStageBuilder<'m> {
    fn build_stage(
        &self,
        state: &StateId,
        cycle: u32,
        attempt: u32,
        entry_addendum: Option<&str>,
        crashed: bool,
    ) -> Result<StagePlan> {
        let st = self
            .machine
            .state(state)
            .ok_or_else(|| CoreError::machine(format!("no such state `{state}`")))?;
        let model = self.machine.resolve_model(st, &ModelChoice::default());
        let skills = self.machine.resolve_skills(st);
        let mcp = self.machine.resolve_mcp(st);
        let spec = WorkerSpec {
            state: state.clone(),
            cycle,
            attempt,
            model,
            // The fake resolves a skill name to a stand-in path; nothing here
            // touches the filesystem.
            skill_paths: skills.iter().map(PathBuf::from).collect(),
            system_prompt_path: PathBuf::from("/dev/null"),
            entry_message: format!("enter {state} cycle {cycle} attempt {attempt}"),
            handoff_path: PathBuf::from("/tmp/handoff.json"),
            cwd: PathBuf::new(),
            session_id: None,
            env: Vec::new(),
        };
        let context = Context {
            ticket_id: self.machine.ticket.clone(),
            state: state.clone(),
            cycle,
            attempt,
            entry_addendum: entry_addendum.map(|s| s.to_string()),
            crashed,
            ..Context::default()
        };
        self.contexts.borrow_mut().push(context);
        Ok(StagePlan { spec, skills, mcp })
    }

    fn build_judge(
        &self,
        criteria: &str,
        worker_summary: &str,
        artifacts: &[Artifact],
        check_output: Option<&str>,
    ) -> Result<JudgeSpec> {
        Ok(JudgeSpec {
            check_output: check_output.map(str::to_string),
            criteria: criteria.into(),
            // The same function `CliStage` builds the real one with. It used to
            // be unreachable from here — it sat in `ledger`, which `engine` may
            // not import — so this fake carried its own version, with different
            // separators and no `Artifacts:` header. Every engine test that
            // asserted on a Judge's digest was asserting on the fake's.
            worker_digest: crate::core::worker_digest_for_judge(worker_summary, artifacts),
            model: self.machine.judge.clone(),
            cwd: PathBuf::new(),
        })
    }

    fn build_navigator(
        &self,
        from: &StateId,
        proposal: Option<&Proposal>,
    ) -> Result<NavigatorSpec> {
        // The declared neighbours, exactly as `CliStage` passes them. This
        // fake used to append `escalation_state` as well, so every engine test
        // ran against a choice set no real Navigator is ever offered — the
        // sentinel is added by `runner::command::navigator_choices`, one layer
        // out, and the escalation *state* is reached by naming it.
        Ok(NavigatorSpec {
            graph_summary: String::new(),
            ledger_digest: String::new(),
            from: from.clone(),
            proposal: proposal.cloned(),
            reachable: self.machine.neighbors(from),
            model: self.machine.navigator.clone(),
            cwd: PathBuf::new(),
        })
    }
}

// ── WorkerResult / Proposal builders ─────────────────────────────────────

pub fn proposal_to(to: &str, rationale: &str) -> Proposal {
    Proposal {
        to: Some(to.into()),
        blocked: false,
        rationale: rationale.into(),
        artifacts: Vec::new(),
    }
}

pub fn proposal_blocked(rationale: &str) -> Proposal {
    Proposal {
        to: None,
        blocked: true,
        rationale: rationale.into(),
        artifacts: Vec::new(),
    }
}

pub fn worker_result(proposal: Proposal) -> WorkerResult {
    WorkerResult {
        summary: "did the work".into(),
        proposal: Some(proposal),
        usage: Usage {
            tokens: 100,
            cost_usd: 0.1,
        },
        exit_ok: true,
        stderr_tail: String::new(),
    }
}

pub fn worker_result_costing(proposal: Proposal, cost_usd: f64) -> WorkerResult {
    WorkerResult {
        usage: Usage {
            tokens: 100,
            cost_usd,
        },
        ..worker_result(proposal)
    }
}

pub fn verdict(pass: bool, rationale: &str) -> Verdict {
    Verdict {
        pass,
        rationale: rationale.into(),
        usage: Usage::default(),
    }
}

pub fn choice(to: &str) -> Choice {
    Choice {
        to: to.into(),
        entry_prompt: None,
        usage: Usage::default(),
    }
}

pub fn choice_with_addendum(to: &str, addendum: &str) -> Choice {
    Choice {
        to: to.into(),
        entry_prompt: Some(addendum.into()),
        usage: Usage::default(),
    }
}

/// An artifact sink that refuses one particular claimed path, standing in for
/// the real store's "that file is not there" / "that escapes the project root".
pub struct RefusingArtifacts {
    pub refuse: &'static str,
}

impl ArtifactSink for RefusingArtifacts {
    fn capture(&self, state: &str, cycle: u32, claim: &Artifact) -> Result<Artifact> {
        if claim.path == self.refuse {
            return Err(CoreError::other(format!(
                "resolving claimed artifact path {}",
                claim.path
            )));
        }
        FakeArtifacts.capture(state, cycle, claim)
    }
}
