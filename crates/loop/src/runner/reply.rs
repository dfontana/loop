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

use std::fmt::Write as _;
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

/// The one parser both tool-less roles answer to: the first non-empty line
/// must be exactly one of `options`, and everything after it is the body.
///
/// The Judge and the Navigator have identical contracts — a bare token on line
/// one, prose underneath — differing only in the token set, so they run the
/// same three steps here instead of spelling them out twice. Returns the
/// *option* that matched rather than what the model typed, so a caller gets
/// back its own canonical spelling and never the model's casing.
///
/// `None` means the reply did not follow the contract. Both callers must fail
/// toward a human on that — the Judge fails closed, the Navigator escalates —
/// which is why this returns nothing rather than guessing.
///
/// Tolerances are deliberately narrow but not hostile: leading blank lines,
/// surrounding whitespace, markdown emphasis or backticks around the token,
/// and a trailing colon are all stripped, because those are formatting habits
/// rather than ambiguity. Anything else — a preamble sentence, "PASS with
/// reservations", a near-miss on a state name — is not an answer. No prefix
/// matching and no fuzzy fallback: guessing which option was meant is how a
/// run ends up somewhere nobody chose.
fn parse_first_line<'o, S: AsRef<str>>(reply: &str, options: &'o [S]) -> Option<(&'o str, String)> {
    let mut lines = reply.lines().skip_while(|l| l.trim().is_empty());
    // Case-folded through `to_lowercase` rather than `eq_ignore_ascii_case`:
    // a state id is whatever the machine author wrote, and matching a
    // non-ASCII one case-insensitively is the behaviour that was already here.
    let token = lines.next()?.trim().trim_matches(DECORATION).to_lowercase();
    let chosen = options
        .iter()
        .find(|o| o.as_ref().to_lowercase() == token)?;
    let body = lines.collect::<Vec<_>>().join("\n").trim().to_string();
    Some((chosen.as_ref(), body))
}

/// The decoration a model might wrap a bare token in — backticks, emphasis,
/// quotes, a trailing colon or period, a markdown bullet or heading marker.
///
/// The space belongs in the set, not in a preceding `trim`: decoration and
/// whitespace interleave (`## PASS`, `* debug`, `**  done  **`), so stripping
/// them in two passes stops at the first space and leaves the token unmatched.
/// An unmatched first line fails closed — the Judge returns no verdict and the
/// Navigator escalates — so this is the one character whose absence turns a
/// formatting habit into a blocked run.
const DECORATION: [char; 9] = ['`', '*', '_', '"', '\'', ':', '.', '#', ' '];

/// The contract [`parse_first_line`] enforces, stated to the model.
///
/// Built from the same `options` the parser is handed, so the prompt cannot
/// offer a token the parser rejects. The Judge and Navigator prompts each used
/// to word this paragraph themselves, next to two parsers that turned out to
/// be one function — and a reworded prompt drifting from the parser is silent
/// in the worst possible way, since an unrecognized first line reads as a
/// failing verdict rather than as a broken contract.
///
/// `example` stands in for the first line inside the fenced shape, `body` for
/// the second; `on_miss` says what becomes of a reply that ignores all this.
/// Those three differ per role and none is derivable, so they are the
/// parameters — the option list is rendered here, from the same slice the
/// parser gets, and is not.
pub fn first_line_contract<S: AsRef<str>>(
    options: &[S],
    example: &str,
    body: &str,
    on_miss: &str,
) -> String {
    let mut out = String::new();
    let _ = write!(
        out,
        "Reply in exactly this shape:\n\n```\n{example}\n{body}\n```\n\n\
         The first line must be one of these, alone, with nothing else on it:\n\n"
    );
    for option in options {
        let _ = writeln!(out, "- `{}`", option.as_ref());
    }
    let _ = write!(out, "\n{on_miss}\n\n");
    out
}

/// Parse a Judge's reply: [`VERDICT_PASS`] or [`VERDICT_FAIL`] alone on the
/// first non-empty line, rationale on the rest.
///
/// `None` is **not** a pass. An unavailable or confused grader waving work
/// through is the exact hole the Judge exists to close.
pub fn parse_verdict(reply: &str) -> Option<(bool, String)> {
    let (token, rationale) = parse_first_line(reply, &[VERDICT_PASS, VERDICT_FAIL])?;
    Some((token == VERDICT_PASS, rationale))
}

/// Parse a Navigator's reply: a state name alone on the first non-empty line,
/// an optional note on the rest.
///
/// The first line is matched against `choices` rather than parsed, so the
/// Navigator cannot name a state that isn't there — the same guarantee the
/// `choose` tool's enum used to give, enforced at the boundary instead of in
/// the schema. `None` means no choice matched, and the caller escalates.
pub fn parse_choice(reply: &str, choices: &[String]) -> Option<(StateId, Option<String>)> {
    let (chosen, note) = parse_first_line(reply, choices)?;
    Some((chosen.to_string(), Some(note).filter(|n| !n.is_empty())))
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

    // ── the shared contract ──────────────────────────────────────────────

    /// The seam this contract exists to close: the prompt's option list and
    /// the parser's are the same slice, so anything the model is offered is
    /// something the parser will accept. Drift here is silent in the worst way
    /// — an unrecognized first line reads as a failing verdict, or as a
    /// Navigator escalation, rather than as a broken prompt.
    #[test]
    fn every_option_the_contract_offers_is_one_the_parser_accepts() {
        let sets: [Vec<String>; 3] = [
            vec![VERDICT_PASS.into(), VERDICT_FAIL.into()],
            vec!["debug".into(), "implement".into(), "escalate".into()],
            vec!["only-one".into()],
        ];
        for options in &sets {
            let contract = first_line_contract(options, "<x>", "<why>", "or else");
            for option in options {
                assert!(
                    contract.contains(&format!("- `{option}`")),
                    "`{option}` is not offered by:\n{contract}"
                );
                let (chosen, body) = parse_first_line(&format!("{option}\nthe note"), options)
                    .unwrap_or_else(|| panic!("the parser rejected the offered `{option}`"));
                assert_eq!(chosen, option);
                assert_eq!(body, "the note");
            }
        }
    }

    /// Both roles get the identical skeleton — the Judge's contract used to be
    /// a separately worded paragraph saying the same thing.
    #[test]
    fn both_roles_share_the_contract_skeleton() {
        let judge = first_line_contract(&[VERDICT_PASS, VERDICT_FAIL], "PASS", "<why>", "x");
        let nav = first_line_contract(&["debug"], "<state>", "<note>", "y");
        let skeleton = "The first line must be one of these, alone, with nothing else on it:";
        assert!(judge.contains(skeleton), "{judge}");
        assert!(nav.contains(skeleton), "{nav}");
    }

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

    /// Decoration and whitespace interleave in real replies, and both roles
    /// fail *toward a human* on a first line they cannot match — the Judge
    /// blocks a transition it actually approved, the Navigator escalates a run
    /// it actually routed. So a markdown bullet or heading, which is the most
    /// ordinary thing a model can do to a bare token, must not cost that.
    #[test]
    fn a_token_behind_a_bullet_or_a_heading_still_matches() {
        assert!(parse_verdict("## PASS\nlooks right").unwrap().0);
        assert!(!parse_verdict("**  FAIL  **\nmissing a test").unwrap().0);

        assert_eq!(parse_choice("* debug\nn", &choices()).unwrap().0, "debug");
        assert_eq!(
            parse_choice("# escalate", &choices()).unwrap().0,
            "escalate"
        );

        // `-` is deliberately *not* decoration: it is a legal character in a
        // state id, and a set that strips it would be reshaping the name it is
        // supposed to be matching. A `- debug` bullet is the one markdown
        // habit this does not absorb, exactly as before.
        assert!(parse_choice("- debug", &choices()).is_none());

        // Nor is any of this a licence to bury the token in prose: the
        // tolerance is for formatting around a bare token, not for a sentence
        // that contains one.
        assert!(parse_verdict("## Verdict: PASS").is_none());
        assert!(parse_choice("* go to debug", &choices()).is_none());
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
