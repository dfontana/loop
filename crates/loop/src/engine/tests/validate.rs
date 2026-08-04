//! `loop validate` — linting a machine that never runs.
//!
//! Every case here is a machine an author could plausibly write and a reason it
//! must not reach a spawn. Filesystem resolution is the caller's job, so a stage
//! prompt and a skill each arrive as a closure the test controls.

use crate::core::{OnExhausted, OnFail, StagePromptRef};
use crate::engine::test_support::*;
use crate::engine::{Severity, validate};

fn always_resolves(_: &StagePromptRef) -> bool {
    true
}

fn never_resolves(_: &StagePromptRef) -> bool {
    false
}

/// Lint with everything resolving — the common case, where the machine's shape
/// is the only thing under test.
fn lint(m: &crate::core::Machine) -> Vec<crate::engine::Diagnostic> {
    validate(m, &always_resolves, &|_| true)
}

/// Whether any diagnostic's message contains `needle`.
fn says(diags: &[crate::engine::Diagnostic], needle: &str) -> bool {
    diags.iter().any(|d| d.message.contains(needle))
}

/// …and at `Error` severity specifically.
fn errors_saying(diags: &[crate::engine::Diagnostic], needle: &str) -> bool {
    diags
        .iter()
        .any(|d| d.severity == Severity::Error && d.message.contains(needle))
}

#[test]
fn catches_missing_entry_state() {
    // Set directly: the builder would insert the state `entry` names, which is
    // exactly what this test needs to be missing.
    let mut m = machine().terminal("done").build();
    m.entry = "nope".into();
    assert!(says(&lint(&m), "entry state"));
}

#[test]
fn catches_dangling_transition_targets() {
    let mut m = machine().entry("a").build();
    m.transitions.push(edge("a", "nowhere"));
    assert!(says(&lint(&m), "names no state or terminal"));
}

#[test]
fn catches_unreachable_state() {
    let m = machine()
        .entry("a")
        .terminal("done")
        .state("island")
        .edge(edge("a", "done"))
        .build();
    assert!(says(&lint(&m), "unreachable"));
}

#[test]
fn catches_no_path_to_terminal() {
    let m = machine()
        .entry("a")
        .terminal("done")
        .edge(edge("a", "dead_end"))
        .build();
    assert!(says(&lint(&m), "no path to any terminal"));
}

#[test]
fn catches_unresolved_stage_prompt() {
    let m = machine()
        .entry("a")
        .terminal("done")
        .edge(edge("a", "done"))
        .build();
    let diags = validate(&m, &never_resolves, &|_| true);
    assert!(says(&diags, "does not resolve"));
}

#[test]
fn catches_loop_head_never_re_entered() {
    let m = machine()
        .entry("a")
        .terminal("done")
        .edge(edge("a", "done"))
        .loop_over(loop_spec("orphan", &["a"], 3, OnExhausted::Escalate))
        .build();
    assert!(says(&lint(&m), "never re-entered"));
}

#[test]
fn catches_escalation_state_naming_nothing() {
    let mut m = machine()
        .entry("a")
        .terminal("done")
        .edge(edge("a", "done"))
        .build();
    m.escalation_state = Some("nowhere".into());
    assert!(says(&lint(&m), "names no state or terminal"));
}

/// A non-terminal escalation state is legal: the engine commits to it directly
/// and then runs it as an ordinary stage, which is how a machine gets a "go do
/// recovery work" destination instead of only a "give up here" one. The linter
/// used to reject this while the loader accepted it, so a machine could load
/// and then fail its own `loop validate`.
#[test]
fn accepts_a_non_terminal_escalation_state() {
    let mut m = machine()
        .entry("a")
        .terminal("done")
        .edge(edge("a", "done"))
        .build();
    m.escalation_state = Some("a".into());
    let diags = lint(&m);
    assert!(
        !says(&diags, "escalation_state"),
        "a declared state is a valid escalation target: {diags:?}"
    );
}

/// Two edges between the same pair used to be disambiguated by their `when`
/// guards. Now `Machine::edge` takes the first and the rest are dead, so the
/// duplicate has to be an error — not a silently ignored `criteria`.
#[test]
fn rejects_duplicate_edges_between_the_same_pair() {
    let m = machine()
        .entry("a")
        .terminal("done")
        .edge(judged_edge("a", "done", "first"))
        .edge(judged_edge("a", "done", "second"))
        .build();
    let diags = lint(&m);
    assert!(
        errors_saying(&diags, "duplicate transition"),
        "got: {diags:?}"
    );
}

/// Skills resolve through the same filesystem seam stage prompts do, so a typo
/// in a skill name is caught by `loop validate` rather than at spawn time.
#[test]
fn reports_a_skill_that_does_not_resolve() {
    let m = machine()
        .entry("qa-staging")
        .with(state_with_skills("qa-staging", &["contract-check"]))
        .terminal("done")
        .edge(judged_edge("qa-staging", "done", "looks correct"))
        .build();

    let diags = validate(&m, &always_resolves, &|name| name != "contract-check");
    assert!(errors_saying(&diags, "contract-check"), "got: {diags:?}");
}

/// A stage loads the union of the machine's `:defaults {:skills ..}` and the
/// state's — so that union is what has to lint. Checking only the state's layer
/// left a typo in the machine defaults to surface as a failed spawn mid-run,
/// which is the one place `validate` exists to prevent.
#[test]
fn checks_skills_that_come_from_the_machine_defaults() {
    let m = machine()
        .entry("implement")
        .terminal("done")
        .edge(judged_edge("implement", "done", "looks correct"))
        .default_skills(&["hose-typo"])
        .build();

    let diags = validate(&m, &always_resolves, &|name| name != "hose-typo");
    let d = diags
        .iter()
        .find(|d| d.message.contains("hose-typo"))
        .unwrap_or_else(|| panic!("expected a diagnostic for the default skill: {diags:?}"));
    assert_eq!(d.severity, Severity::Error);
    assert!(
        d.message.contains(":defaults"),
        "the diagnostic must say where the name came from: {}",
        d.message
    );
}

/// Same reasoning for MCP, one layer up: a server named in the machine's
/// `:defaults` is the identical misconfiguration as one named on the state.
#[test]
fn checks_mcp_servers_that_come_from_the_machine_defaults() {
    let m = machine()
        .entry("implement")
        .terminal("done")
        .edge(judged_edge("implement", "done", "looks correct"))
        .default_mcp(&["warehouse"])
        .without_extensions()
        .build();

    assert!(says(&lint(&m), "MCP servers"));
}

/// The server names are the user's business, but the tool that connects them
/// is loop's: a stage told to call `mcp({connect: …})` in a spawn without the
/// `mcp` extension fails at run time for a reason `validate` can see now.
#[test]
fn reports_named_mcp_servers_without_the_mcp_extension() {
    let mut m = machine()
        .entry("qa-staging")
        .with(state_with_mcp("qa-staging", &["warehouse"]))
        .terminal("done")
        .edge(judged_edge("qa-staging", "done", "looks correct"))
        .without_extensions()
        .build();

    let diags = lint(&m);
    assert!(errors_saying(&diags, "pi-extensions"), "got: {diags:?}");

    // With the extension declared, an unverifiable server name is not an error:
    // loop never reads the user's mcp.json and has nothing to check it against.
    m.pi_extensions.push("mcp".into());
    let ok = lint(&m);
    assert!(!says(&ok, "MCP"), "got: {ok:?}");
}

/// An edge with neither tier commits whatever the worker proposed. That is
/// occasionally right — an unconditional hand-off — so it warns rather than
/// failing. It must still be said: with `when` gone, unguarded is the shape you
/// get by forgetting.
#[test]
fn warns_on_an_edge_with_no_check_and_no_criteria() {
    let m = machine()
        .entry("a")
        .terminal("done")
        .edge(edge("a", "done"))
        .build();

    let diags = lint(&m);
    assert!(
        diags
            .iter()
            .any(|d| d.severity == Severity::Warning && d.message.contains("committed unexamined")),
        "got: {diags:?}"
    );
}

/// The counterpart: a guarded edge is silent, so the warning stays worth
/// reading.
#[test]
fn does_not_warn_on_a_guarded_edge() {
    let m = machine()
        .entry("a")
        .terminal("done")
        .edge(judged_edge("a", "done", "the work is done"))
        .build();

    let diags = lint(&m);
    assert!(!says(&diags, "committed unexamined"), "got: {diags:?}");
}

/// A loop head re-entered only by an `on_fail: route` is still a loop head.
/// The shipped `standard-ticket` template loops exactly this way — a failed
/// Judge routes back to `implement` — so treating routes as non-re-entry made
/// `loop validate` reject the template it ships with.
#[test]
fn counts_an_on_fail_route_as_loop_head_re_entry() {
    let m = machine()
        .entry("implement")
        .terminal("done")
        .edge(crate::core::Transition {
            on_fail: OnFail::Route("implement".into()),
            ..judged_edge("review", "done", "no blocking defects")
        })
        .edge(judged_edge("implement", "review", "plan done"))
        .loop_over(loop_spec(
            "fix",
            &["implement", "review"],
            4,
            OnExhausted::Escalate,
        ))
        .build();

    let diags = lint(&m);
    assert!(!says(&diags, "never re-entered"), "got: {diags:?}");
}
