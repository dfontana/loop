//! Parsing pi's newline-delimited JSON event stream.
//!
//! Shape (docs/json.md in the pi package): the first line is
//! `{"type":"session","id":"…"}`, then `AgentSessionEvent`s. The ones that
//! matter here:
//!
//! - `{"type":"message_end","message":{"role":"assistant","content":[…],"usage":{…}}}`
//!   — final text and token/cost usage. Verified against the installed pi's
//!   `session-format.md`: `usage` is `{input,output,cacheRead,cacheWrite,
//!   totalTokens,cost:{input,output,cacheRead,cacheWrite,total}}`. We take
//!   `totalTokens` for tokens, and prefer `cost.total` for the dollar figure
//!   (falling back to summing whatever numeric fields are present, for
//!   forward-compatibility with a provider that omits `total`) — summing
//!   every field naively would double-count, since `total` already is that
//!   sum.
//! - `{"type":"tool_execution_end","toolCallId":…,"toolName":…,"result":…,"isError":…}`
//!   — `result` is exactly the extension's `execute()` return value
//!   (`{content:[{type:"text",text:"LOOP_TRANSITION {…}"}]}` etc., per
//!   `crates/loop-toolbox/ext/*.ts`). `LOOP_TRANSITION` / `LOOP_VARS` /
//!   `LOOP_VERDICT` / `LOOP_CHOICE` markers surface in that text.
//!
//! Every stream line is independently parsed; one that isn't valid JSON, or
//! doesn't look like an event we understand, is skipped rather than failing
//! the whole run — pi may interleave warnings, and a crash truncates the
//! final line rather than corrupting an earlier one.

use loop_core::{
    LOOP_CHOICE_MARKER, LOOP_TRANSITION_MARKER, LOOP_VARS_MARKER, LOOP_VERDICT_MARKER, Proposal,
    Result, Usage, Vars,
};
use serde_json::Value;

/// Everything worth keeping from one spawn's stream.
#[derive(Clone, Debug, Default)]
pub struct StreamOutcome {
    pub session_id: Option<String>,
    /// The last assistant text block — the stage summary.
    pub summary: String,
    pub usage: Usage,
    /// Trusted vars, deep-merged in stream order.
    pub vars: Vars,
    /// Raw payloads found after each marker, keyed by marker name, in the
    /// order they were scraped off the stream.
    pub markers: Vec<(String, String)>,
}

impl StreamOutcome {
    /// The last `LOOP_TRANSITION` payload, parsed.
    ///
    /// A malformed payload (the marker was found, but the rest of the line
    /// isn't valid JSON, or doesn't match [`Proposal`]'s shape) is treated the
    /// same as "no transition call" — skipped, not fatal. A worker that never
    /// calls `transition` is likewise `Ok(None)`; that's for the engine to
    /// interpret, not an error at this layer.
    pub fn proposal(&self) -> Result<Option<Proposal>> {
        Ok(self
            .marker(LOOP_TRANSITION_MARKER)
            .and_then(|payload| serde_json::from_str::<Proposal>(payload).ok()))
    }

    /// The last payload for a given marker name.
    pub fn marker(&self, name: &str) -> Option<&str> {
        self.markers
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, payload)| payload.as_str())
    }
}

/// Parse a whole stream. Unparseable lines are skipped, not fatal — pi may
/// interleave warnings, and a run must not die on a stray line.
pub fn parse_stream(mut reader: impl std::io::BufRead) -> Result<StreamOutcome> {
    let mut outcome = StreamOutcome::default();
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .map_err(|e| loop_core::CoreError::io("reading pi event stream", e))?;
        if n == 0 {
            break;
        }
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if trimmed.is_empty() {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Value>(trimmed) else {
            // Malformed JSON (a stray warning, a truncated final line from a
            // crash) — skip it and keep going.
            continue;
        };
        let Some(kind) = event.get("type").and_then(Value::as_str) else {
            continue;
        };

        match kind {
            "session" => {
                if let Some(id) = event.get("id").and_then(Value::as_str) {
                    outcome.session_id = Some(id.to_string());
                }
            }
            "message_end" => {
                let Some(message) = event.get("message") else {
                    continue;
                };
                if message.get("role").and_then(Value::as_str) != Some("assistant") {
                    continue;
                }
                if let Some(usage) = message.get("usage") {
                    outcome.usage += parse_usage(usage);
                }
                if let Some(text) = concat_text_blocks(message) {
                    if !text.trim().is_empty() {
                        outcome.summary = text;
                    }
                }
            }
            "tool_execution_end" => {
                let Some(result) = event.get("result") else {
                    continue;
                };
                if let Some(text) = concat_text_blocks(result) {
                    for (name, payload) in scan_markers(&text) {
                        if name == LOOP_VARS_MARKER {
                            if let Ok(v) = serde_json::from_str::<Value>(&payload) {
                                outcome.vars.merge(&Vars::from_value(v));
                            }
                            // A malformed LOOP_VARS payload is silently
                            // dropped from `vars` but still recorded below —
                            // it never gates anything since nothing merged.
                        }
                        outcome.markers.push((name, payload));
                    }
                }
            }
            _ => {}
        }
    }
    Ok(outcome)
}

/// Pull `LOOP_<NAME> {json}` markers out of a tool result's text.
///
/// A marker must start a line (after trimming leading whitespace); the
/// payload is the rest of that line, trimmed. This tolerates surrounding
/// prose (build logs before/after the marker line) and several markers in
/// one blob. The payload is returned as raw text — whether it's valid JSON
/// is somebody else's problem, so a malformed one never aborts the scan.
pub fn scan_markers(text: &str) -> Vec<(String, String)> {
    const NAMES: &[&str] = &[
        LOOP_TRANSITION_MARKER,
        LOOP_VARS_MARKER,
        LOOP_VERDICT_MARKER,
        LOOP_CHOICE_MARKER,
    ];
    let mut out = Vec::new();
    for raw_line in text.lines() {
        let line = raw_line.trim_start();
        for name in NAMES {
            let prefix_with_space = format!("{name} ");
            if let Some(rest) = line.strip_prefix(&prefix_with_space) {
                out.push((name.to_string(), rest.trim().to_string()));
                break;
            }
        }
    }
    out
}

/// Sum tokens and cost off one `usage` object. Handles both the real
/// `{totalTokens, cost:{..,total}}` shape and (defensively) simpler shapes a
/// fixture or a future provider might use.
fn parse_usage(usage: &Value) -> Usage {
    let tokens = usage
        .get("totalTokens")
        .and_then(Value::as_u64)
        .or_else(|| usage.get("tokens").and_then(Value::as_u64))
        .unwrap_or(0);

    let cost_usd = match usage.get("cost") {
        Some(Value::Object(map)) => map
            .get("total")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| map.values().filter_map(Value::as_f64).sum()),
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        _ => usage.get("cost_usd").and_then(Value::as_f64).unwrap_or(0.0),
    };

    Usage { tokens, cost_usd }
}

/// Join every `{"type":"text","text":…}` content block on an object that has
/// a `content: [...]` array (an `AssistantMessage` or a `ToolResultMessage`'s
/// `result`). Non-text blocks (thinking, images, tool calls) are skipped.
fn concat_text_blocks(v: &Value) -> Option<String> {
    let blocks = v.get("content")?.as_array()?;
    let mut buf = String::new();
    for block in blocks {
        if block.get("type").and_then(Value::as_str) == Some("text") {
            if let Some(t) = block.get("text").and_then(Value::as_str) {
                if !buf.is_empty() {
                    buf.push('\n');
                }
                buf.push_str(t);
            }
        }
    }
    if buf.is_empty() { None } else { Some(buf) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Cursor;

    fn line(v: &Value) -> String {
        format!("{}\n", serde_json::to_string(v).unwrap())
    }

    fn tool_result_line(tool_call_id: &str, tool_name: &str, text: &str) -> String {
        line(&json!({
            "type": "tool_execution_end",
            "toolCallId": tool_call_id,
            "toolName": tool_name,
            "result": {"content": [{"type": "text", "text": text}]},
            "isError": false,
        }))
    }

    fn assistant_message_end(text: &str, tokens: u64, cost: f64) -> String {
        line(&json!({
            "type": "message_end",
            "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": text}],
                "api": "messages",
                "provider": "anthropic",
                "model": "claude-sonnet-5",
                "usage": {
                    "input": 10, "output": 20, "cacheRead": 0, "cacheWrite": 0,
                    "totalTokens": tokens,
                    "cost": {"input": 0.1, "output": 0.2, "cacheRead": 0.0, "cacheWrite": 0.0, "total": cost},
                },
                "stopReason": "toolUse",
            }
        }))
    }

    #[test]
    fn full_worker_stream_parses_summary_usage_vars_and_proposal() {
        let mut stream = String::new();
        stream.push_str(&line(
            &json!({"type": "session", "version": 3, "id": "sess-1", "timestamp": "t", "cwd": "/proj"}),
        ));
        stream.push_str(&line(&json!({"type": "agent_start"})));
        stream.push_str(&line(&json!({"type": "turn_start"})));
        stream.push_str(&assistant_message_end("Running the build.", 111, 0.11));
        stream.push_str(&tool_result_line(
            "call_1",
            "spark_build",
            "Building...\nLOOP_VARS {\"build\":{\"status\":\"pass\",\"id\":\"b-1\"}}\nDone.",
        ));
        stream.push_str(&assistant_message_end(
            "Build passed, moving on.",
            222,
            0.22,
        ));
        stream.push_str(&tool_result_line(
            "call_2",
            "transition",
            "LOOP_TRANSITION {\"to\":\"review\",\"blocked\":false,\"rationale\":\"build green\",\"artifacts\":[],\"vars\":{}}",
        ));
        stream.push_str(&line(
            &json!({"type": "turn_end", "message": {}, "toolResults": []}),
        ));
        stream.push_str(&line(&json!({"type": "agent_end", "messages": []})));

        let outcome = parse_stream(Cursor::new(stream)).expect("parse ok");

        assert_eq!(outcome.session_id.as_deref(), Some("sess-1"));
        assert_eq!(outcome.summary, "Build passed, moving on.");
        assert_eq!(outcome.usage.tokens, 333);
        assert!((outcome.usage.cost_usd - 0.33).abs() < 1e-9);
        assert_eq!(outcome.vars.get_path("build.status").unwrap(), "pass");
        assert_eq!(outcome.vars.get_path("build.id").unwrap(), "b-1");

        let proposal = outcome.proposal().unwrap().expect("a proposal");
        assert_eq!(proposal.to.as_deref(), Some("review"));
        assert!(!proposal.blocked);
        assert_eq!(proposal.rationale, "build green");
    }

    #[test]
    fn multiple_loop_vars_lines_deep_merge_in_order() {
        let mut stream = String::new();
        stream.push_str(&tool_result_line(
            "c1",
            "spark_build",
            "LOOP_VARS {\"qa\":{\"result\":\"fail\",\"detail\":\"boom\"}}",
        ));
        stream.push_str(&tool_result_line(
            "c2",
            "spark_retest",
            "LOOP_VARS {\"qa\":{\"result\":\"pass\"}}",
        ));

        let outcome = parse_stream(Cursor::new(stream)).unwrap();
        assert_eq!(outcome.vars.get_path("qa.result").unwrap(), "pass");
        assert_eq!(outcome.vars.get_path("qa.detail").unwrap(), "boom");
    }

    #[test]
    fn loop_vars_line_embedded_in_surrounding_prose() {
        let text = "Starting job 42...\nsome noise here\nLOOP_VARS {\"deploy\":{\"status\":\"ok\"}}\nCleaning up temp files\nExit 0";
        let stream = tool_result_line("c1", "spark_deploy", text);

        let outcome = parse_stream(Cursor::new(stream)).unwrap();
        assert_eq!(outcome.vars.get_path("deploy.status").unwrap(), "ok");
    }

    #[test]
    fn malformed_json_line_mid_stream_is_skipped_not_fatal() {
        let mut stream = String::new();
        stream.push_str(&assistant_message_end("first", 10, 0.01));
        stream.push_str("not even json {{{\n");
        stream.push_str(&assistant_message_end("second", 20, 0.02));

        let outcome = parse_stream(Cursor::new(stream)).expect("must not fail the whole run");
        assert_eq!(outcome.summary, "second");
        assert_eq!(outcome.usage.tokens, 30);
    }

    #[test]
    fn stream_with_no_transition_call_yields_no_proposal() {
        let stream = assistant_message_end("did some work but never wrapped up", 5, 0.0);
        let outcome = parse_stream(Cursor::new(stream)).unwrap();
        assert!(outcome.proposal().unwrap().is_none());
    }

    #[test]
    fn malformed_marker_payload_is_skipped_not_fatal() {
        let stream = tool_result_line("c1", "transition", "LOOP_TRANSITION not-json-at-all");
        let outcome = parse_stream(Cursor::new(stream)).unwrap();
        assert!(outcome.proposal().unwrap().is_none());
    }

    #[test]
    fn truncated_stream_does_not_panic() {
        // Simulates a crash mid-write: a valid line, then a partial line with
        // no trailing newline that also isn't valid JSON.
        let mut stream = String::new();
        stream.push_str(&line(&json!({"type": "session", "id": "sess-2"})));
        stream.push_str("{\"type\":\"message_start\",\"message\":{\"role\":\"assistant\",\"con");
        // no trailing newline

        let outcome = parse_stream(Cursor::new(stream)).expect("must not panic or error");
        assert_eq!(outcome.session_id.as_deref(), Some("sess-2"));
        assert!(outcome.proposal().unwrap().is_none());
    }

    #[test]
    fn cost_sums_across_several_assistant_messages() {
        let mut stream = String::new();
        stream.push_str(&assistant_message_end("a", 100, 0.10));
        stream.push_str(&assistant_message_end("b", 50, 0.05));
        stream.push_str(&assistant_message_end("c", 25, 0.025));

        let outcome = parse_stream(Cursor::new(stream)).unwrap();
        assert_eq!(outcome.usage.tokens, 175);
        assert!((outcome.usage.cost_usd - 0.175).abs() < 1e-9);
    }

    #[test]
    fn scan_markers_finds_several_in_one_blob() {
        let text = "prefix\nLOOP_VARS {\"a\":1}\nmiddle\nLOOP_VARS {\"b\":2}\nsuffix";
        let found = scan_markers(text);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].0, LOOP_VARS_MARKER);
        assert_eq!(found[0].1, "{\"a\":1}");
        assert_eq!(found[1].1, "{\"b\":2}");
    }

    #[test]
    fn scan_markers_tolerates_non_json_payload() {
        let found = scan_markers("LOOP_VERDICT this is not json");
        assert_eq!(
            found,
            vec![(
                LOOP_VERDICT_MARKER.to_string(),
                "this is not json".to_string()
            )]
        );
    }

    #[test]
    fn non_assistant_messages_do_not_affect_usage_or_summary() {
        let mut stream = String::new();
        stream.push_str(&line(&json!({
            "type": "message_end",
            "message": {"role": "user", "content": "hello"}
        })));
        let outcome = parse_stream(Cursor::new(stream)).unwrap();
        assert_eq!(outcome.usage.tokens, 0);
        assert_eq!(outcome.summary, "");
    }
}
