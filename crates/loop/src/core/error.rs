//! The one error type every crate in the workspace returns.
//!
//! Deliberately coarse: the harness's job is to explain a failure to a human
//! reading a terminal, not to let callers branch on failure kinds. The one
//! exception is [`CoreError::Transient`], which the engine *does* branch on to
//! decide between a retry and an abort.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// The machine or config file is malformed, or the graph is invalid.
    #[error("machine: {0}")]
    Machine(String),

    /// A Fennel guard closure failed to evaluate.
    #[error("guard `{guard}`: {detail}")]
    Guard { guard: String, detail: String },

    /// A `pi` spawn failed, or its event stream could not be understood.
    #[error("agent ({role}): {detail}")]
    Agent { role: String, detail: String },

    /// A referenced playbook, tool, or prose file could not be resolved.
    #[error("could not resolve {kind} `{name}`{}", .searched.iter().map(|p| format!("\n  searched: {}", p.display())).collect::<String>())]
    Unresolved {
        kind: &'static str,
        name: String,
        searched: Vec<PathBuf>,
    },

    /// A guardrail tripped: budget, wallclock, transition count, cycle cap.
    #[error("budget exceeded: {0}")]
    BudgetExceeded(String),

    /// Something failed but is worth retrying (a flaky spawn, a partial read).
    #[error("transient: {0}")]
    Transient(String),

    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },

    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("{0}")]
    Other(String),
}

impl CoreError {
    pub fn machine(msg: impl Into<String>) -> Self {
        Self::Machine(msg.into())
    }

    pub fn other(msg: impl Into<String>) -> Self {
        Self::Other(msg.into())
    }

    pub fn agent(role: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::Agent {
            role: role.into(),
            detail: detail.into(),
        }
    }

    pub fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }

    /// True when the engine should consider re-running rather than aborting.
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::Transient(_))
    }
}

pub type Result<T> = std::result::Result<T, CoreError>;

/// Attach a path-flavored context string to an `io::Error`.
pub trait IoContext<T> {
    fn io_ctx(self, context: impl Into<String>) -> Result<T>;
}

impl<T> IoContext<T> for std::result::Result<T, std::io::Error> {
    fn io_ctx(self, context: impl Into<String>) -> Result<T> {
        self.map_err(|e| CoreError::io(context, e))
    }
}
