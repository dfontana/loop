//! `$UPPER_SNAKE` substitution over the context namespace.

use std::collections::BTreeMap;

/// One run of a scanned template: literal text, or a `$NAME` token.
enum Piece<'a> {
    Text(&'a str),
    /// The bare identifier, without the leading `$`.
    Var(&'a str),
}

/// Split a template into literal text and `$NAME` tokens.
///
/// Longest-name-first matching, so `$ARTIFACT_DIFF_PATH` is not eaten by
/// `$ARTIFACT_DIFF`. `$$` is a literal `$`.
///
/// Implemented as maximal-munch identifier scanning rather than substring
/// matching against each known name: at every `$` we consume the *whole*
/// following `[A-Za-z_][A-Za-z0-9_]*` run before anything consults a variable
/// map. That gives "longest match wins" for free — `$ARTIFACT_DIFF_PATH` is
/// one token, so it can never be truncated to `$ARTIFACT_DIFF` — and it also
/// keeps a genuinely unknown longer name (e.g. `$ARTIFACT_DIFF_PATHOLOGY`)
/// from being partially replaced and left with a dangling suffix.
///
/// [`substitute`] and [`referenced_vars`] share it, so what `loop preview`
/// reports a playbook references is by construction what rendering looks up.
///
/// Byte-indexed slicing is safe throughout: every boundary this takes is at a
/// `$` or at an ASCII identifier edge, and no byte of a multi-byte codepoint
/// is ASCII, so a codepoint is never split.
fn pieces(template: &str) -> Vec<Piece<'_>> {
    let bytes = template.as_bytes();
    let len = bytes.len();
    let mut out = Vec::new();
    let mut text_start = 0usize;
    let mut i = 0usize;

    while i < len {
        if bytes[i] != b'$' {
            i += 1;
            continue;
        }

        // `$$` is one literal `$`: close the text run *after* the first
        // dollar and resume past the second.
        if i + 1 < len && bytes[i + 1] == b'$' {
            out.push(Piece::Text(&template[text_start..i + 1]));
            i += 2;
            text_start = i;
            continue;
        }

        let start = i + 1;
        if start < len && is_ident_start(bytes[start]) {
            let mut end = start + 1;
            while end < len && is_ident_continue(bytes[end]) {
                end += 1;
            }
            if text_start < i {
                out.push(Piece::Text(&template[text_start..i]));
            }
            out.push(Piece::Var(&template[start..end]));
            i = end;
            text_start = i;
            continue;
        }

        // A `$` not followed by an identifier is ordinary text.
        i += 1;
    }

    if text_start < len {
        out.push(Piece::Text(&template[text_start..]));
    }
    out
}

/// Replace every `$NAME` present in `vars`. **Unknown `$NAMES` pass through
/// untouched** so `$HOME` and shell snippets in a playbook still work.
pub fn substitute(template: &str, vars: &BTreeMap<String, String>) -> String {
    let mut out = String::with_capacity(template.len());
    for piece in pieces(template) {
        match piece {
            Piece::Text(text) => out.push_str(text),
            Piece::Var(name) => match vars.get(name) {
                Some(value) => out.push_str(value),
                None => {
                    out.push('$');
                    out.push_str(name);
                }
            },
        }
    }
    out
}

/// Every `$NAME` a template writes, in first-appearance order, deduplicated.
///
/// The caller decides which of them are loop variables: this reports what the
/// template *asks for*, including names that will pass through untouched.
/// `loop preview` splits the two against [`crate::core::Context::to_map`].
pub fn referenced_vars(template: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for piece in pieces(template) {
        if let Piece::Var(name) = piece
            && !out.iter().any(|seen| seen == name)
        {
            out.push(name.to_string());
        }
    }
    out
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// The short positional kickoff message a stage is spawned with — "you are
/// entering STATE, cycle N", plus the navigator's addendum when present.
///
/// `mcp` is the servers the state named. They lead, because the `mcp`
/// extension starts every session with every server **off** and offers no
/// flag to change that — the agent connecting them is the only way in, and it
/// has to happen before the work that needs them.
pub fn entry_message(ctx: &crate::core::Context, mcp: &[String]) -> String {
    let mut msg = String::new();
    if !mcp.is_empty() {
        msg.push_str(
            "Before anything else, connect the MCP servers this stage needs — they \
             start the session disconnected, and `mcp({connect: \"…\"})` is what \
             turns one on:\n\n",
        );
        for server in mcp {
            msg.push_str(&format!("- `mcp({{connect: \"{server}\"}})`\n"));
        }
        msg.push_str(
            "\nIf one fails to connect, say so in your handoff rationale rather \
             than working around it.\n\n",
        );
    }
    msg.push_str(&format!(
        "You are entering **{}**, cycle {}.",
        ctx.state, ctx.cycle
    ));
    if let Some(prev) = &ctx.prev_state {
        if !prev.is_empty() {
            msg.push_str(&format!(" (previous state: {prev})"));
        }
    }
    if let Some(addendum) = &ctx.entry_addendum {
        if !addendum.is_empty() {
            msg.push_str("\n\n");
            msg.push_str(addendum);
        }
    }
    msg
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn replaces_known_names() {
        let v = vars(&[("TICKET_ID", "PROJ-1"), ("CYCLE", "2")]);
        assert_eq!(
            substitute("ticket $TICKET_ID cycle $CYCLE.", &v),
            "ticket PROJ-1 cycle 2."
        );
    }

    #[test]
    fn unknown_names_pass_through_untouched() {
        let v = vars(&[("TICKET_ID", "PROJ-1")]);
        assert_eq!(
            substitute("home is $HOME, ticket $TICKET_ID", &v),
            "home is $HOME, ticket PROJ-1"
        );
    }

    #[test]
    fn double_dollar_is_literal() {
        let v = vars(&[("FOO", "bar")]);
        assert_eq!(
            substitute("price is $$5, not $FOO", &v),
            "price is $5, not bar"
        );
    }

    #[test]
    fn longest_name_wins_over_a_shorter_prefix() {
        let v = vars(&[
            ("ARTIFACT_DIFF", "short-value"),
            ("ARTIFACT_DIFF_PATH", "long-value"),
        ]);
        assert_eq!(
            substitute("see $ARTIFACT_DIFF_PATH here", &v),
            "see long-value here"
        );
        assert_eq!(
            substitute("see $ARTIFACT_DIFF here", &v),
            "see short-value here"
        );
    }

    #[test]
    fn unknown_longer_name_is_not_partially_replaced() {
        // ARTIFACT_DIFF is known but ARTIFACT_DIFF_PATHOLOGY is not: the whole
        // unknown token must survive, not "short-value" + "OLOGY".
        let v = vars(&[("ARTIFACT_DIFF", "short-value")]);
        assert_eq!(
            substitute("$ARTIFACT_DIFF_PATHOLOGY", &v),
            "$ARTIFACT_DIFF_PATHOLOGY"
        );
    }

    #[test]
    fn template_with_no_variables_is_unchanged() {
        let v = vars(&[("FOO", "bar")]);
        assert_eq!(
            substitute("plain text, no dollars here.", &v),
            "plain text, no dollars here."
        );
    }

    #[test]
    fn non_ascii_text_survives_substitution() {
        let v = vars(&[("STATE", "qa")]);
        assert_eq!(
            substitute("état — $STATE — 完了 $$5", &v),
            "état — qa — 完了 $5"
        );
    }

    /// The whole point of one shared scanner: every name `referenced_vars`
    /// reports is a name `substitute` would look up, and no others.
    #[test]
    fn referenced_vars_reports_what_substitute_looks_up() {
        let template = "$TASK then $PLAN, again $TASK, $$NOT_A_VAR, $HOME, bare $ sign";
        assert_eq!(
            referenced_vars(template),
            vec!["TASK", "PLAN", "HOME"],
            "first-appearance order, deduplicated, `$$` and a bare `$` excluded"
        );

        // Substituting only the reported names leaves nothing else changed.
        let v = vars(&[("TASK", "T"), ("PLAN", "P"), ("HOME", "H")]);
        assert_eq!(
            substitute(template, &v),
            "T then P, again T, $NOT_A_VAR, H, bare $ sign"
        );
    }

    #[test]
    fn referenced_vars_takes_the_whole_token_not_a_prefix() {
        assert_eq!(
            referenced_vars("$ARTIFACT_DIFF_PATH and $ARTIFACT_DIFF"),
            vec!["ARTIFACT_DIFF_PATH", "ARTIFACT_DIFF"]
        );
    }

    #[test]
    fn referenced_vars_is_empty_for_a_template_with_no_variables() {
        assert!(referenced_vars("plain text, no dollars here.").is_empty());
    }

    #[test]
    fn entry_message_includes_state_and_cycle() {
        let ctx = crate::core::Context {
            state: "implement".into(),
            cycle: 3,
            ..Default::default()
        };
        let msg = entry_message(&ctx, &[]);
        assert!(msg.contains("implement"));
        assert!(msg.contains('3'));
    }

    #[test]
    fn entry_message_includes_navigator_addendum() {
        let ctx = crate::core::Context {
            state: "debug".into(),
            cycle: 1,
            entry_addendum: Some("Focus on the schema mismatch.".into()),
            ..Default::default()
        };
        let msg = entry_message(&ctx, &[]);
        assert!(msg.contains("Focus on the schema mismatch."));
    }

    /// The connect instruction has to come *before* the "you are entering"
    /// line: a stage that reads its work first and its tooling second may
    /// reach for a server that is still off.
    #[test]
    fn entry_message_asks_for_a_connect_per_named_server_up_front() {
        let ctx = crate::core::Context {
            state: "qa-staging".into(),
            cycle: 1,
            ..Default::default()
        };
        let msg = entry_message(&ctx, &["linear".into(), "warehouse".into()]);

        assert!(msg.contains(r#"mcp({connect: "linear"})"#), "{msg}");
        assert!(msg.contains(r#"mcp({connect: "warehouse"})"#), "{msg}");
        assert!(msg.find("connect").unwrap() < msg.find("entering").unwrap());
    }

    /// A stage that names no server must not be told anything about MCP —
    /// otherwise every spawn pays for a paragraph about a tool it won't use.
    #[test]
    fn entry_message_says_nothing_about_mcp_when_no_server_is_named() {
        let ctx = crate::core::Context {
            state: "review".into(),
            cycle: 1,
            ..Default::default()
        };
        let msg = entry_message(&ctx, &[]);
        assert!(!msg.to_lowercase().contains("mcp"), "{msg}");
    }
}
