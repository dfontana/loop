//! In-process fakes for the engine's collaborator traits, plus small machine
//! fixture helpers. No Lua, no subprocess, no filesystem, no API key — every
//! trait the engine depends on is faked here.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::PathBuf;

use loop_core::{
    AgentRunner, ArtifactClaim, ArtifactRef, ArtifactSink, Budgets, Check, CheckOutcome,
    CheckRunner, Choice, Context, CoreError, Defaults, Event, EventPayload, JudgeSpec, LoopSpec,
    Machine, ModelChoice, ModelSpec, NavigatorSpec, OnExhausted, OnFail, PlaybookRef, Proposal,
    QaCase, Result, State, StateId, Thinking, Totals, Transition, Usage, Verdict, WorkerResult,
    WorkerSpec,
};

use crate::prompts::{StageBuilder, StagePlan};

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
        navigator_max_invocations: 5,
        source_hash: "sha256:test".into(),
        source_path: PathBuf::from("machine.fnl"),
        dir: PathBuf::from("."),
    }
}

pub fn state(id: &str) -> State {
    State {
        id: id.into(),
        playbook: PlaybookRef::Inline("test playbook".into()),
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
            exit_code: Some(0),
            output: output.into(),
        });
    }

    pub fn script_fail(&self, output: &str) {
        self.script(CheckOutcome {
            passed: false,
            exit_code: Some(1),
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
                exit_code: Some(0),
                output: String::new(),
            }))
    }
}

// ── LedgerSink fake ───────────────────────────────────────────────────────

#[derive(Default)]
pub struct FakeLedger {
    pub events: Vec<Event>,
}

impl loop_core::LedgerSink for FakeLedger {
    fn append(&mut self, payload: EventPayload) -> Result<Event> {
        let e = Event::now(payload);
        self.events.push(e.clone());
        Ok(e)
    }

    fn read_all(&self) -> Result<Vec<Event>> {
        Ok(self.events.clone())
    }
}

impl FakeLedger {
    pub fn kinds(&self) -> Vec<&'static str> {
        self.events.iter().map(|e| e.kind()).collect()
    }

    pub fn payloads_of<'a>(&'a self, kind: &str) -> Vec<&'a EventPayload> {
        self.events
            .iter()
            .filter(|e| e.kind() == kind)
            .map(|e| &e.payload)
            .collect()
    }
}

// ── ArtifactSink fake ─────────────────────────────────────────────────────

#[derive(Default)]
pub struct FakeArtifacts;

impl ArtifactSink for FakeArtifacts {
    fn capture(&self, state: &str, cycle: u32, claim: &ArtifactClaim) -> Result<ArtifactRef> {
        Ok(ArtifactRef {
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
    /// how a test asserts on what actually reached the playbook rather than on
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
        let model = self
            .machine
            .resolve_model(st, &ModelChoice::default(), &model_spec());
        let skills = self.machine.resolve_skills(st, &[]);
        let mcp = self.machine.resolve_mcp(st, &[]);
        let spec = WorkerSpec {
            ticket: self.machine.ticket.clone(),
            state: state.clone(),
            cycle,
            attempt,
            model,
            // The fake resolves a skill name to a stand-in path; nothing here
            // touches the filesystem.
            skill_paths: skills.iter().map(PathBuf::from).collect(),
            system_prompt_path: PathBuf::from("/dev/null"),
            entry_message: format!("enter {state} cycle {cycle} attempt {attempt}"),
            reachable: self.machine.neighbors(state),
            mcp,
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
        self.contexts.borrow_mut().push(context.clone());
        Ok(StagePlan {
            spec,
            context,
            skills,
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
            check_output: check_output.map(str::to_string),
            criteria: criteria.into(),
            worker_digest: worker_summary.into(),
            artifact_paths: artifacts.iter().map(|a| PathBuf::from(&a.path)).collect(),
            model: self.machine.judge.clone(),
            cwd: PathBuf::new(),
        })
    }

    fn build_navigator(
        &self,
        from: &StateId,
        proposal: Option<&Proposal>,
    ) -> Result<NavigatorSpec> {
        let mut reachable = self.machine.neighbors(from);
        if let Some(esc) = &self.machine.escalation_state {
            if !reachable.contains(esc) {
                reachable.push(esc.clone());
            }
        }
        Ok(NavigatorSpec {
            graph_summary: String::new(),
            ledger_digest: String::new(),
            from: from.clone(),
            proposal: proposal.cloned(),
            reachable,
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
        session_id: None,
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

#[allow(dead_code)]
pub fn zero_totals() -> Totals {
    Totals::default()
}

/// An artifact sink that refuses one particular claimed path, standing in for
/// the real store's "that file is not there" / "that escapes the project root".
pub struct RefusingArtifacts {
    pub refuse: &'static str,
}

impl ArtifactSink for RefusingArtifacts {
    fn capture(&self, state: &str, cycle: u32, claim: &ArtifactClaim) -> Result<ArtifactRef> {
        if claim.path == self.refuse {
            return Err(CoreError::other(format!(
                "resolving claimed artifact path {}",
                claim.path
            )));
        }
        FakeArtifacts.capture(state, cycle, claim)
    }
}
