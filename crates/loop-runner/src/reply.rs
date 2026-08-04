//! Reading each role's answer back out.
//!
//! Three roles, three shapes, one principle: **every unreadable answer fails
//! toward a human**, never toward the next state. A Worker with no usable
//! handoff becomes a blocked proposal and goes to the Navigator; a Judge with
//! no usable verdict fails closed; a Navigator with no usable choice escalates.
//!
//! This module is what replaced `transition-tool.ts`, `verdict-tool.ts`, and
//! `choose-tool.ts`. The Worker's answer is still structured data parsed with
//! serde — it just arrives in a file it wrote rather than in a marker line
//! scraped off a tool result. The other two roles have no tools at all (that
//! isolation is the point of them), so their answers are prose against a fixed
//! first-line contract, stated in the system prompts `command.rs` builds.

use std::path::Path;

use loop_core::{Proposal, Result, StateId};

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
        Err(e) => Err(loop_core::CoreError::io(
            format!("clearing stale handoff {}", path.display()),
            e,
        )),
    }
}

/// Parse a Judge's reply: `PASS` or `FAIL` alone on the first non-empty line,
/// rationale on the rest.
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
    let pass = match normalize_token(first).as_str() {
        "pass" => true,
        "fail" => false,
        _ => return None,
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
