//! Parsing pi's newline-delimited JSON event stream.
//!
//! Shape (docs/json.md in the pi package): the first line is
//! `{"type":"session","id":"…"}`, then `AgentSessionEvent`s. The ones that
//! matter here:
//!
//! - `{"type":"message_end","message":{"role":"assistant","content":[…],"usage":{…}}}`
//!   — final text and token/cost usage.
//! - `{"type":"tool_execution_end","toolCallId":…,"toolName":…,"result":…,"isError":…}`
//!   — where `LOOP_TRANSITION` / `LOOP_VARS` / `LOOP_VERDICT` / `LOOP_CHOICE`
//!   markers surface, in the result's text content.
//!
//! `usage.cost` is an object of per-category costs; sum its numeric fields.

use loop_core::{Proposal, Result, Usage, Vars};

/// Everything worth keeping from one spawn's stream.
#[derive(Clone, Debug, Default)]
pub struct StreamOutcome {
    pub session_id: Option<String>,
    /// The last assistant text block — the stage summary.
    pub summary: String,
    pub usage: Usage,
    /// Trusted vars, deep-merged in stream order.
    pub vars: Vars,
    /// Raw payloads found after each marker, keyed by marker name.
    pub markers: Vec<(String, String)>,
}

impl StreamOutcome {
    /// The last `LOOP_TRANSITION` payload, parsed.
    pub fn proposal(&self) -> Result<Option<Proposal>> {
        todo!("T4")
    }

    /// The last payload for a given marker.
    pub fn marker(&self, name: &str) -> Option<&str> {
        let _ = name;
        todo!("T4")
    }
}

/// Parse a whole stream. Unparseable lines are skipped, not fatal — pi may
/// interleave warnings, and a run must not die on a stray line.
///
/// TASK T4.
pub fn parse_stream(reader: impl std::io::BufRead) -> Result<StreamOutcome> {
    let _ = reader;
    todo!("T4")
}

/// Pull `LOOP_<NAME> {json}` markers out of a tool result's text.
///
/// TASK T4. A marker must start a line; the payload is the rest of that line.
/// Tolerate surrounding prose, several markers in one blob, and a payload that
/// isn't valid JSON (skip it rather than failing).
pub fn scan_markers(text: &str) -> Vec<(String, String)> {
    let _ = text;
    todo!("T4")
}
