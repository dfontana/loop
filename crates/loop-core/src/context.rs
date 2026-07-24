//! The context namespace (docs/04-toolbox.md) — the `$UPPER_SNAKE` variables a
//! playbook template and a scoped-tool's `valueFromCmd` can both see.

use std::collections::BTreeMap;

use crate::machine::QaCase;
use crate::vars::Vars;

#[derive(Clone, Debug, Default)]
pub struct Context {
    pub ticket_id: String,
    pub task: String,
    pub plan: String,
    pub state: String,
    pub prev_state: Option<String>,
    pub cycle: u32,
    pub attempt: u32,
    pub ledger_digest: String,
    /// The Navigator's get-back-on-track note, when it fired.
    pub entry_addendum: Option<String>,
    pub qa_cases: Vec<QaCase>,
    /// Captured artifacts by name → `$ARTIFACT_<NAME>`.
    pub artifacts: BTreeMap<String, String>,
    /// Ledger vars, flattened into `$BUILD_ID`-style names.
    pub vars: Vars,
}

impl Context {
    /// The full substitution map. Later crates render templates by replacing
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
        m.insert("LEDGER_DIGEST".into(), self.ledger_digest.clone());
        m.insert(
            "ENTRY_ADDENDUM".into(),
            self.entry_addendum.clone().unwrap_or_default(),
        );
        m.insert("QA_CASES".into(), self.qa_cases_md());
        for (name, path) in &self.artifacts {
            m.insert(format!("ARTIFACT_{}", name.to_uppercase()), path.clone());
        }
        for (name, value) in self.vars.to_env() {
            m.entry(name).or_insert(value);
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
