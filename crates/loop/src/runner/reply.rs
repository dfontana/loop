//! Each role's answer: the contract it is given, and reading it back out.
//!
//! Three roles, three shapes, one principle: **every unreadable answer fails
//! toward a human**, never toward the next state. A Worker with no usable
//! handoff becomes a blocked proposal and goes to the Navigator; a Judge with
//! no usable verdict fails closed; a Navigator with no usable choice escalates.
//!
//! The Worker's half of its contract ([`handoff_protocol`], the block appended
//! to every rendered stage prompt) lives here beside the parser that has to accept
//! it — the Judge's and Navigator's prompts sit beside theirs in
//! [`crate::runner::command`] for the same reason. A contract stated in one
//! module and parsed in another drifts, and the drift is silent.
//!
//! This module is what replaced `transition-tool.ts`, `verdict-tool.ts`, and
//! `choose-tool.ts`. The Worker's answer is still structured data parsed with
//! serde — it just arrives in a file it wrote rather than in a marker line
//! scraped off a tool result. The other two roles have no tools at all (that
//! isolation is the point of them), so their answers are prose against a fixed
//! first-line contract, stated in the system prompts `command.rs` builds.

use std::path::Path;

use crate::core::{Proposal, Result, StateId};

/// The handoff protocol, appended by the harness to every rendered Worker
/// prompt. This is the entire agent-side contract for ending a stage.
///
/// It replaces an injected `transition` tool. That tool's one advantage was a
/// `to` parameter typed as an enum of reachable states — but the harness
/// re-checks the target against the graph regardless (see the engine's
/// `route_proposal`), so the enum only ever saved a Navigator spawn, never a
/// bad commit. What it cost was a TypeScript extension, pi's
/// extension ABI, and a JSON round-trip through a scraped marker line.
///
/// A file keeps the part that mattered — the decision arrives as structured
/// data the harness parses with serde, not as prose it has to interpret — and
/// drops the coupling. Any agent CLI that can write a file can drive a loop.
///
/// Writing no file, or writing something unparseable, both land in
/// [`read_handoff`] as `None`: the engine synthesizes a blocked proposal
/// carrying [`crate::core::ABSENT_HANDOFF_RATIONALE`] and the Navigator routes
/// it.
pub fn handoff_protocol(handoff_path: &std::path::Path, reachable: &[String]) -> String {
    let mut out = String::from("\n\n---\n\n## Ending this stage\n\n");
    out.push_str(&format!(
        "When this stage's goal is met — and only then — write your handoff to \
         `{}` and stop. The harness reads that file after you exit; it is the \
         only way your decision reaches it. Nothing you write in prose moves \
         the run.\n\n",
        handoff_path.display()
    ));

    out.push_str("```json\n{\n");
    out.push_str("  \"to\": \"<next state>\",\n");
    out.push_str("  \"blocked\": false,\n");
    out.push_str("  \"rationale\": \"why this is the right next step\",\n");
    out.push_str("  \"artifacts\": [{\"name\": \"diff\", \"path\": \"relative/path.patch\"}]\n");
    out.push_str("}\n```\n\n");

    if reachable.is_empty() {
        out.push_str(
            "This state has no outgoing edges, so there is no valid `to`. If you \
             reach the end of your work here, set `\"blocked\": true` with a \
             rationale.\n\n",
        );
    } else {
        out.push_str("`to` must be one of:\n\n");
        for state in reachable {
            out.push_str(&format!("- `{state}`\n"));
        }
        out.push_str(
            "\nNaming anything else does not create an edge — it sends the run to \
             the Navigator to be rerouted, which costs a spawn and is capped.\n\n",
        );
    }

    out.push_str(
        "If you cannot make progress, set `\"blocked\": true` and omit `to`, with a \
         rationale precise enough for someone else to act on. `artifacts` is \
         optional: list files later stages should receive (diffs, reports, \
         samples), and the harness snapshots each one.\n\n\
         Write the file exactly once, as the last thing you do.\n",
    );
    out
}

/// Read a Worker's handoff file.
///
/// `Ok(None)` covers every way an answer can be absent or unusable — no file
/// (the agent ended its turn without writing one), an unreadable file, or JSON
/// that isn't a [`Proposal`]. All of them mean the same thing to the engine,
/// which synthesizes a blocked proposal and routes it, so none of them is an
/// error here.
///
/// Deliberately tolerant in one direction only: a file whose *shape* is right
/// but whose `to` names a state that doesn't exist parses fine and is caught
/// downstream by the graph check, exactly as an off-graph proposal always was.
pub fn read_handoff(path: &Path) -> Option<Proposal> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<Proposal>(&raw).ok()
}

/// Remove a stale handoff before spawning.
///
/// Paths are per-attempt, so this should never find anything — but "should
/// never" is doing real work here. If a previous attempt's file survived under
/// a path this attempt reuses, reading it back would hand the engine a
/// proposal the current Worker never made, and the run would advance on a
/// decision nobody took. Cheap insurance against that class of bug.
pub fn clear_handoff(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(crate::core::CoreError::io(
            format!("clearing stale handoff {}", path.display()),
            e,
        )),
    }
}

/// The two tokens a Judge may open with. Named here, beside the parser that
/// accepts them, and spliced into the prompt by
/// [`crate::runner::command::judge_prompt`] — the Navigator's token set is
/// already shared between its prompt and [`parse_choice`] this way, and the
/// Judge is the role where a silent divergence is worst: an unrecognized first
/// line fails closed, so a reworded prompt would read as every judgement
/// suddenly failing rather than as a broken contract.
pub const VERDICT_PASS: &str = "PASS";
pub const VERDICT_FAIL: &str = "FAIL";

/// Parse a Judge's reply: [`VERDICT_PASS`] or [`VERDICT_FAIL`] alone on the
/// first non-empty line, rationale on the rest.
///
/// `None` means the reply did not follow the contract, and the caller must
/// treat that as a failure — never as a pass. An unavailable or confused
/// grader waving work through is the exact hole the Judge exists to close, so
/// this returns nothing rather than guessing.
///
/// Tolerances are deliberately narrow but not hostile: leading blank lines,
/// surrounding whitespace, markdown emphasis or backticks around the token,
/// and a trailing colon are all stripped, because those are formatting habits
/// rather than ambiguity about the verdict. Anything else — a preamble
/// sentence, "PASS with reservations", a bare rationale — is not a verdict.
pub fn parse_verdict(reply: &str) -> Option<(bool, String)> {
    let (first, rest) = split_first_line(reply)?;
    let token = normalize_token(first);
    let pass = if token == VERDICT_PASS.to_lowercase() {
        true
    } else if token == VERDICT_FAIL.to_lowercase() {
        false
    } else {
        return None;
    };
    Some((pass, rest))
}

/// Parse a Navigator's reply: a state name alone on the first non-empty line,
/// an optional note on the rest.
///
/// The first line is matched against `choices` rather than parsed, so the
/// Navigator cannot name a state that isn't there — the same guarantee the
/// `choose` tool's enum used to give, enforced at the boundary instead of in
/// the schema. `None` means no choice matched, and the caller escalates.
///
/// Matching is case-insensitive and ignores decoration for the same reason
/// [`parse_verdict`] does. It is otherwise exact: no prefix matching, no fuzzy
/// fallback. A near-miss on a state name is a Navigator that did not follow
/// the contract, and guessing which state it meant is how a run ends up
/// somewhere nobody chose.
pub fn parse_choice(reply: &str, choices: &[String]) -> Option<(StateId, Option<String>)> {
    let (first, rest) = split_first_line(reply)?;
    let token = normalize_token(first);
    let chosen = choices.iter().find(|c| c.to_lowercase() == token)?;
    let note = if rest.is_empty() { None } else { Some(rest) };
    Some((chosen.clone(), note))
}

/// Split a reply into its first non-empty line and the trimmed remainder.
fn split_first_line(reply: &str) -> Option<(&str, String)> {
    let mut lines = reply.lines().skip_while(|l| l.trim().is_empty());
    let first = lines.next()?;
    let rest = lines.collect::<Vec<_>>().join("\n").trim().to_string();
    Some((first, rest))
}

/// Lowercase a first-line token with the decoration a model might wrap it in
/// stripped — backticks, asterisks, quotes, and a trailing colon or period.
fn normalize_token(line: &str) -> String {
    line.trim()
        .trim_matches(|c: char| matches!(c, '`' | '*' | '_' | '"' | '\'' | ':' | '.' | '#' | ' '))
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // ── worker handoff ───────────────────────────────────────────────────

    /// The JSON example in the prompt has to be a thing [`read_handoff`] will
    /// actually accept. It is hand-written prose in `handoff_protocol`, and
    /// [`Proposal`] is a serde struct somewhere else entirely — nothing but
    /// this test stops a field being added to one and not the other, and the
    /// failure mode is every Worker writing a handoff the harness discards.
    #[test]
    fn the_prompt_example_is_a_proposal_the_parser_accepts() {
        let dir = tempdir().unwrap();
        let handoff = dir.path().join("h.json");
        let prompt = handoff_protocol(&handoff, &["review".to_string()]);

        let json = prompt
            .split("```json")
            .nth(1)
            .and_then(|rest| rest.split("```").next())
            .expect("the protocol block shows a fenced json example");
        // The example uses `<next state>` as a placeholder; substitute a real
        // one, since that is what a Worker is being told to do.
        let json = json.replace("<next state>", "review");
        std::fs::write(&handoff, &json).unwrap();

        let p = read_handoff(&handoff).expect("the documented example must parse");
        assert_eq!(p.to.as_deref(), Some("review"));
        assert_eq!(p.artifacts.len(), 1, "the example's artifact survived");
    }

    #[test]
    fn handoff_round_trips_a_full_proposal() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("handoff.json");
        std::fs::write(
            &path,
            r#"{"to":"review","blocked":false,"rationale":"build green",
                "artifacts":[{"name":"diff","path":"d.patch"}]}"#,
        )
        .unwrap();

        let p = read_handoff(&path).expect("a proposal");
        assert_eq!(p.to.as_deref(), Some("review"));
        assert!(!p.blocked);
        assert_eq!(p.rationale, "build green");
        assert_eq!(p.artifacts.len(), 1);
    }

    /// `blocked` and `artifacts` both default, so the smallest honest handoff
    /// a blocked worker can write is two fields.
    #[test]
    fn handoff_accepts_the_minimal_blocked_shape() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("handoff.json");
        std::fs::write(&path, r#"{"to":null,"blocked":true,"rationale":"stuck"}"#).unwrap();

        let p = read_handoff(&path).expect("a proposal");
        assert!(p.blocked);
        assert!(p.to.is_none());
        assert!(p.artifacts.is_empty());
    }

    #[test]
    fn missing_or_malformed_handoff_is_none_not_an_error() {
        let dir = tempdir().unwrap();
        assert!(read_handoff(&dir.path().join("nope.json")).is_none());

        let bad = dir.path().join("bad.json");
        std::fs::write(&bad, "not json at all").unwrap();
        assert!(read_handoff(&bad).is_none());

        // Right JSON, wrong shape: `rationale` is required.
        let wrong = dir.path().join("wrong.json");
        std::fs::write(&wrong, r#"{"to":"review"}"#).unwrap();
        assert!(read_handoff(&wrong).is_none());
    }

    /// The bug this guards against: a stale file being read as the current
    /// attempt's decision.
    #[test]
    fn clear_handoff_removes_a_stale_file_and_tolerates_absence() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("handoff.json");
        std::fs::write(&path, r#"{"to":"review","rationale":"old"}"#).unwrap();

        clear_handoff(&path).unwrap();
        assert!(read_handoff(&path).is_none());
        clear_handoff(&path).expect("clearing an absent file is fine");
    }

    // ── judge verdict ────────────────────────────────────────────────────

    #[test]
    fn verdict_reads_pass_and_fail_with_rationale() {
        let (pass, why) = parse_verdict("PASS\nEvery plan item is in the diff.").unwrap();
        assert!(pass);
        assert_eq!(why, "Every plan item is in the diff.");

        let (pass, why) = parse_verdict("FAIL\nThe suite was asserted, not run.").unwrap();
        assert!(!pass);
        assert_eq!(why, "The suite was asserted, not run.");
    }

    #[test]
    fn verdict_tolerates_formatting_but_not_prose() {
        assert!(parse_verdict("\n\n  **PASS**  \nfine").unwrap().0);
        assert!(parse_verdict("`FAIL`:\nnope").is_some());
        assert!(!parse_verdict("`FAIL`:\nnope").unwrap().0);

        // A preamble is not a verdict — this is the fail-closed case.
        assert!(parse_verdict("Let me assess this.\nPASS").is_none());
        assert!(parse_verdict("PASS with reservations\nmostly").is_none());
        assert!(parse_verdict("").is_none());
        assert!(parse_verdict("   \n  ").is_none());
    }

    #[test]
    fn verdict_without_a_rationale_still_parses() {
        let (pass, why) = parse_verdict("PASS").unwrap();
        assert!(pass);
        assert!(why.is_empty());
    }

    // ── navigator choice ─────────────────────────────────────────────────

    fn choices() -> Vec<String> {
        vec!["debug".into(), "implement".into(), "escalate".into()]
    }

    #[test]
    fn choice_reads_a_state_and_its_note() {
        let (to, note) = parse_choice(
            "debug\nThe migration test is flaky; isolate it.",
            &choices(),
        )
        .unwrap();
        assert_eq!(to, "debug");
        assert_eq!(note.unwrap(), "The migration test is flaky; isolate it.");
    }

    #[test]
    fn choice_without_a_note_is_none() {
        let (to, note) = parse_choice("escalate", &choices()).unwrap();
        assert_eq!(to, "escalate");
        assert!(note.is_none());
    }

    #[test]
    fn choice_matches_case_insensitively_through_decoration() {
        assert_eq!(parse_choice("`Debug`\nn", &choices()).unwrap().0, "debug");
        assert_eq!(
            parse_choice("  IMPLEMENT  ", &choices()).unwrap().0,
            "implement"
        );
    }

    /// The guarantee the `choose` tool's enum used to give: a state that isn't
    /// offered cannot be picked. No fuzzy fallback — a near-miss escalates
    /// rather than routing somewhere nobody chose.
    #[test]
    fn choice_rejects_anything_not_offered() {
        assert!(parse_choice("review\nnot on the list", &choices()).is_none());
        assert!(parse_choice("deb\nprefix of a real one", &choices()).is_none());
        assert!(parse_choice("I think we should go to debug", &choices()).is_none());
        assert!(parse_choice("", &choices()).is_none());
    }
}
