//! `mock-pi` — a scripted stand-in for the real `pi`, so the whole harness is
//! testable deterministically, offline, and for $0.
//!
//! Point the harness at it with `LOOP_PI_BIN=/path/to/mock-pi`. It accepts (and
//! ignores) pi's flags, then emits a scripted `--mode json` event stream.
//!
//! # The script
//!
//! `LOOP_MOCK_SCRIPT` names a JSON file:
//!
//! ```json
//! {
//!   "default": { "summary": "did the thing", "transition": {"to": "review", "rationale": "…"} },
//!   "steps": [
//!     { "match": {"state": "implement", "cycle": 1},
//!       "vars":  {"build": {"status": "pass", "id": "b-1"}},
//!       "summary": "implemented",
//!       "transition": {"to": "review", "rationale": "plan items done"},
//!       "usage": {"tokens": 100, "cost_usd": 0.01} },
//!     { "match": {"state": "qa-staging", "cycle": 1},
//!       "vars": {"qa": {"result": "fail", "error_class": "transient"}},
//!       "transition": {"to": "qa-staging", "rationale": "flaky executor"} },
//!     { "match": {"state": "debug"}, "exit": "crash" },
//!     { "match": {"role": "judge"}, "verdict": {"pass": true, "rationale": "evidence checks out"} },
//!     { "match": {"role": "navigator"}, "choice": {"to": "qa-staging", "entry_prompt": "re-run QA"} }
//!   ]
//! }
//! ```
//!
//! Steps are matched in order against the invocation (role, state, cycle,
//! attempt — read from the environment the harness exports); the first match
//! wins, and a matched step is consumed unless it sets `"repeat": true`.
//! Consumption state lives in a sidecar file next to the script, so a run's
//! successive spawns walk the script.
//!
//! `"exit": "crash"` emits a truncated stream and exits non-zero — that is how
//! the crash-resume path gets tested.
//!
//! TASK T4 implements this binary.

fn main() {
    todo!("T4")
}
