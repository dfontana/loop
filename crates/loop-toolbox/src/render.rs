//! `$UPPER_SNAKE` substitution, matching the scoped-tools convention.

use std::collections::BTreeMap;

/// Replace every `$NAME` present in `vars`. **Unknown `$NAMES` pass through
/// untouched** so `$HOME` and shell snippets in a playbook still work.
///
/// Longest-name-first matching, so `$ARTIFACT_DIFF_PATH` is not eaten by
/// `$ARTIFACT_DIFF`. `$$` is a literal `$`.
///
/// Implemented as maximal-munch identifier scanning rather than substring
/// matching against each known name: at every `$` we consume the *whole*
/// following `[A-Za-z_][A-Za-z0-9_]*` run before ever consulting `vars`. That
/// gives "longest match wins" for free — `$ARTIFACT_DIFF_PATH` is looked up as
/// one token, so it can never be truncated to `$ARTIFACT_DIFF` — and it also
/// keeps a genuinely unknown longer name (e.g. `$ARTIFACT_DIFF_PATHOLOGY`)
/// from being partially replaced and left with a dangling suffix.
pub fn substitute(template: &str, vars: &BTreeMap<String, String>) -> String {
    let bytes = template.as_bytes();
    let len = bytes.len();
    let mut out = String::with_capacity(len);
    let mut i = 0usize;

    while i < len {
        if bytes[i] == b'$' {
            if i + 1 < len && bytes[i + 1] == b'$' {
                out.push('$');
                i += 2;
                continue;
            }

            let start = i + 1;
            if start < len && is_ident_start(bytes[start]) {
                let mut end = start + 1;
                while end < len && is_ident_continue(bytes[end]) {
                    end += 1;
                }
                let name = &template[start..end];
                if let Some(value) = vars.get(name) {
                    out.push_str(value);
                    i = end;
                    continue;
                }
            }

            // Unknown name, or `$` not followed by an identifier: emit the
            // `$` literally and let the following bytes be copied as
            // ordinary text on subsequent iterations.
            out.push('$');
            i += 1;
            continue;
        }

        let ch_len = utf8_len(bytes[i]);
        out.push_str(&template[i..i + ch_len]);
        i += ch_len;
    }

    out
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Length in bytes of the UTF-8 codepoint starting at `b`. `b` is always a
/// valid char boundary here since the identifier scan above only ever stops
/// on ASCII bytes (`$`, alnum/underscore boundaries) or consumes whole ASCII
/// runs, never splitting a multi-byte codepoint.
fn utf8_len(b: u8) -> usize {
    if b & 0x80 == 0 {
        1
    } else if b & 0xE0 == 0xC0 {
        2
    } else if b & 0xF0 == 0xE0 {
        3
    } else {
        4
    }
}

/// The short positional kickoff message a stage is spawned with — "you are
/// entering STATE, cycle N", plus the navigator's addendum when present.
pub fn entry_message(ctx: &loop_core::Context) -> String {
    let mut msg = format!("You are entering **{}**, cycle {}.", ctx.state, ctx.cycle);
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
    fn entry_message_includes_state_and_cycle() {
        let ctx = loop_core::Context {
            state: "implement".into(),
            cycle: 3,
            ..Default::default()
        };
        let msg = entry_message(&ctx);
        assert!(msg.contains("implement"));
        assert!(msg.contains('3'));
    }

    #[test]
    fn entry_message_includes_navigator_addendum() {
        let ctx = loop_core::Context {
            state: "debug".into(),
            cycle: 1,
            entry_addendum: Some("Focus on the schema mismatch.".into()),
            ..Default::default()
        };
        let msg = entry_message(&ctx);
        assert!(msg.contains("Focus on the schema mismatch."));
    }
}
