//! The context namespace (docs/03-customizing.md) — the `$UPPER_SNAKE` variables a
//! stage prompt template can see.

use std::collections::BTreeMap;

use crate::core::machine::{Machine, QaCase};

#[derive(Clone, Debug, Default)]
pub struct Context {
    pub ticket_id: String,
    pub task: String,
    pub plan: String,
    pub state: String,
    pub prev_state: Option<String>,
    pub cycle: u32,
    pub attempt: u32,
    /// True when this entry follows a stage that died mid-flight rather than a
    /// clean arrival — a resumed crash or an in-process retry after the worker
    /// process failed. A stage prompt that does something expensive and
    /// non-idempotent (opening a PR, kicking a deploy) can branch on it to
    /// check for its own half-finished work first.
    pub crashed: bool,
    pub ledger_digest: String,
    /// The Navigator's get-back-on-track note, when it fired.
    pub entry_addendum: Option<String>,
    pub qa_cases: Vec<QaCase>,
    /// Captured artifacts by name → `$ARTIFACT_<NAME>`.
    pub artifacts: BTreeMap<String, String>,
}

impl Context {
    /// The run-independent context: everything the machine already knows, with
    /// every run-dependent field at the value a first entry would carry —
    /// cycle 1, attempt 1, no previous state, no crash, no Navigator addendum,
    /// no artifacts, an empty digest.
    ///
    /// This is what the same state would get on an empty ledger, and it is
    /// what `loop preview` renders with. Nothing at run time builds a context
    /// this way: the runtime folds the real ledger instead, which is why a
    /// preview render is representative rather than exact.
    pub fn representative(machine: &Machine, state: &str) -> Self {
        Self {
            ticket_id: machine.ticket.clone(),
            task: machine.task.clone(),
            plan: machine.plan.clone(),
            state: state.to_string(),
            cycle: 1,
            attempt: 1,
            qa_cases: machine.qa_cases.clone(),
            // Everything else — no previous state, not crashed, empty digest,
            // no addendum, no artifacts — is exactly `Default`, and spelling
            // it out again meant a new field had two definitions of "what
            // preview shows".
            ..Default::default()
        }
    }

    /// The full substitution map. The renderers substitute templates by replacing
    /// `$NAME` for each key; unknown `$NAMES` are left untouched so `$HOME`
    /// still works.
    pub fn to_map(&self) -> BTreeMap<String, String> {
        let mut m = BTreeMap::new();
        m.insert("TICKET_ID".into(), self.ticket_id.clone());
        m.insert("TASK".into(), self.task.clone());
        m.insert("PLAN".into(), self.plan.clone());
        m.insert("STATE".into(), self.state.clone());
        m.insert(
            "PREV_STATE".into(),
            self.prev_state.clone().unwrap_or_default(),
        );
        m.insert("CYCLE".into(), self.cycle.to_string());
        m.insert("ATTEMPT".into(), self.attempt.to_string());
        // "1" or empty, matching every other optional value in this map: a
        // stage prompt tests it by interpolating it, and an absent flag has to
        // render as nothing rather than as the word "false".
        m.insert(
            "CRASHED".into(),
            if self.crashed { "1" } else { "" }.to_string(),
        );
        m.insert("LEDGER_DIGEST".into(), self.ledger_digest.clone());
        m.insert(
            "ENTRY_ADDENDUM".into(),
            self.entry_addendum.clone().unwrap_or_default(),
        );
        m.insert("QA_CASES".into(), self.qa_cases_md());
        for (name, path) in &self.artifacts {
            m.insert(format!("ARTIFACT_{}", name.to_uppercase()), path.clone());
        }
        m
    }

    fn qa_cases_md(&self) -> String {
        self.qa_cases
            .iter()
            .map(|c| format!("- **{}** — {}", c.id, c.desc))
            .collect::<Vec<_>>()
            .join("\n")
    }
}
