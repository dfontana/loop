//! `$UPPER_SNAKE` substitution, matching the scoped-tools convention.

use std::collections::BTreeMap;

/// Replace every `$NAME` present in `vars`. **Unknown `$NAMES` pass through
/// untouched** so `$HOME` and shell snippets in a playbook still work.
///
/// Longest-name-first matching, so `$ARTIFACT_DIFF_PATH` is not eaten by
/// `$ARTIFACT_DIFF`. `$$` is a literal `$`.
///
/// TASK T3.
pub fn substitute(template: &str, vars: &BTreeMap<String, String>) -> String {
    let _ = (template, vars);
    todo!("T3")
}

/// The short positional kickoff message a stage is spawned with — "you are
/// entering STATE, cycle N", plus the navigator's addendum when present.
///
/// TASK T3.
pub fn entry_message(ctx: &loop_core::Context) -> String {
    let _ = ctx;
    todo!("T3")
}
