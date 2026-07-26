# Open a Worker's pi session

## Outcome

Make a Worker's full history available through pi's existing session UI instead of copying, parsing, or persisting a second transcript in loop.

`loop session` should help a human choose an attempt from a terminal picker. The picker is the normal path: its rows describe the state, cycle, attempt, time, outcome, and Worker summary. The opaque `session_id` is only an internal selection key; it must not be the option the user has to recognize.

This is viable with the current data model: each `state_entered` ledger event already records the deterministic `session_id` passed to the Worker. pi persists that session and can reopen it with `pi --session <id>`.

Judge and Navigator spawns remain intentionally sessionless; this command is for Worker stages only.

## CLI contract

```sh
loop session                         # interactive picker, all Worker attempts
loop session implement               # picker filtered to the exact state
loop session implement --latest      # latest usable implement attempt, no picker
loop session --latest                # latest usable Worker attempt, no picker
```

- The optional positional state is an **exact prefilter**, not an instruction to open a session immediately. It preserves useful queries such as “the last implement attempt” without making a human remember cycle and attempt numbers.
- Without `--latest`, use the picker when stdin and stdout are terminals. A non-interactive invocation must fail with a hint to use `--latest`; it must never silently choose a session because output is being piped.
- `--latest` is the deterministic automation escape hatch. It selects the last usable candidate in reverse ledger order, after applying the optional state filter. `--cycle` and `--attempt` are deliberately not part of this contract: older attempts are what the picker is for.
- The picker starts in `All attempts` mode. `Ctrl+O` cycles the candidate scope:
  1. `All attempts` — every `state_entered` with a non-empty session id;
  2. `Latest per state` — the newest usable attempt for each exact state;
  3. `Incomplete` — attempts with no matching `worker_output` yet; then back to `All attempts`. The current mode is visible in the header, and the state prefilter remains in force in every mode. This makes “last implementor” available either as `loop session implement --latest` for scripts or as a readable picker view.
- Candidates are displayed newest-first in reverse ledger order; timestamps are display metadata, not the ordering key. Fuzzy search operates on the visible state id, cycle/attempt, timestamp, outcome, and Worker summary. It must not require a session id in the query.
- Each row is human-readable, for example:

  ```text
  implement — cycle 2, attempt 1 — 2026-07-26 12:04 — finished
    Added the retry guard and updated the tests.
  ```

  If a current machine can supply a state description, it may enrich the row, but a missing or invalid `machine.fnl` must not prevent selection. The ticket and state id remain visible so rows do not become ambiguous. Duplicate-looking rows are disambiguated by cycle, attempt, and timestamp, never by exposing the session id.

- `Enter` opens the highlighted candidate; `Esc`/`Ctrl+C` cancels without launching pi. Restore the terminal before launching pi so pi receives the original stdin/stdout/stderr unchanged.
- Before launch, print one concise human line identifying the selected state, cycle, attempt, and outcome/summary. Do not print the opaque id as the normal selection UI; it remains an implementation detail.
- Start the configured pi binary in the project directory with inherited stdin/stdout/stderr and `--session <id>`. Use `--session`, not `--session-id`: opening history should fail if the recorded session is gone, not create an empty replacement with the same id.
- Respect `LOOP_PI_BIN`. This does not need a valid machine or toolbox; the ledger event and project path are sufficient, and `LOOP_PI_BIN` is available from `Config::defaults(paths)`.
- Propagate a failed pi launch or non-zero pi exit as command failure.
- If the chosen attempt has no matching `worker_output`, warn that the session may still be active or may have crashed. Do not parse or mutate the pi session file or the ledger.
- If no matching event or usable session id exists, return a specific error that includes the requested state filter and selection mode.

## Picker architecture decision

A direct `fzf` implementation is a reasonable small prototype, but the inline native picker is the better long-term fit for this command because selection is structured data and `Ctrl+O` changes the candidate set.

| Approach | Advantages | Costs / risks |
| --- | --- | --- |
| External `fzf` subprocess | Proven interaction model, excellent fuzzy ranking, preview/bind support, very little rendering code, and no Rust TUI dependency | Requires an extra executable and version/TTY checks; the selected line needs a delimiter/escaping protocol to map a readable row back to an opaque candidate; `Ctrl+O` mode changes require `reload(...)`, shell quoting, and temporary/precomputed lists; subprocess behavior is harder to test portably; terminal cleanup and inherited streams are split across two processes |
| Inline `ratatui` + fuzzy matcher | One native binary; the UI owns `Candidate` values and can map a highlighted row directly to an event ordinal; `Ctrl+O`, state filters, detail panes, status labels, and terminal restoration are ordinary application state; matching and selection logic can be unit-tested without a terminal | Adds terminal lifecycle and rendering code, `crossterm`/`ratatui` dependencies, and a responsibility to restore raw mode on every exit path; we own the small picker UI and its key bindings |
| `skim` as a complete picker | Rust and fzf-like behavior without requiring a system `fzf` | It owns a complete fuzzy-finder UI/event loop, so embedding an inline custom layout and mode switching is less direct than using a matcher with our own UI |

Use an inline picker built with `ratatui`, `crossterm`, and the synchronous `nucleo-matcher` API. The ledger is read into a finite in-memory candidate list, so a background matcher is unnecessary at this scale; use the higher-level `nucleo` API later only if session counts make synchronous matching visibly slow. Do not add an `fzf` runtime dependency or shell-based reload protocol.

## Candidate and selection model

- Add a pure candidate builder (for example in `crates/loop-cli/src/session_picker.rs`) that reads `Event` values and returns one candidate per usable `state_entered`, retaining its ledger ordinal as the internal identity. The ordinal is not persisted and is not shown to the user.
- Associate events by ledger episode: a candidate starts at its `state_entered` and ends at the next `state_entered`; within that range, associate the matching `worker_output` (same state/cycle) and any errors. `worker_output` currently has no attempt field, so matching the bounded episode is intentional and avoids a ledger-schema change. Capture the timestamp, model/thinking, summary, usage, artifacts, and incomplete/crash evidence needed for the row/detail view.
- Keep filtering/ranking separate from terminal rendering. Pure helpers should cover exact state filtering, the three `Ctrl+O` modes, fuzzy query ranking, latest selection, and mapping a visible result back to its candidate ordinal. Tests must prove that duplicate display text cannot open the wrong session.
- The terminal layer should only translate key events into picker state and return `Option<CandidateOrdinal>`. Use a small inline viewport rather than taking over an alternate screen, and guard raw mode/terminal restoration with cleanup that also runs on errors and cancellation.
- `commands::session` should read/repair the ledger through `Ledger`, build the candidates, run either the picker or `--latest` policy, print the human selection line/warning, and then launch pi. It must not call the machine-loading path as a prerequisite for the command.

## Why this is the primary history mechanism

pi sessions already contain assistant messages, tool calls, tool results, commands, usage, and branching. loop should retain only the durable session id needed to find that history. It should not grow its own transcript format, session-file locator, exporter, or TUI for viewing the transcript.

## Implementation work

- Add the `Session` subcommand and its optional state/`--latest` argument in `crates/loop-cli/src/main.rs`.
- Implement ledger candidate construction, picker policy, and interactive pi launch in `crates/loop-cli/src/commands.rs` plus a focused picker module; keep selection behavior pure and unit-testable.
- Keep session selection based on `EventPayload::StateEntered`; no ledger schema change should be necessary because `session_id`, `state`, `cycle`, and `attempt` are already present in `crates/loop-core/src/event.rs` and emitted by `crates/loop-engine/src/lib.rs`.
- Verify that `crates/loop-cli/src/stage.rs` continues assigning one stable id per state/cycle/attempt and `crates/loop-runner/src/command.rs` continues passing it to pi as `--session-id` when creating the Worker session.
- Add the minimal `ratatui`, `crossterm`, and `nucleo-matcher` dependencies to `crates/loop-cli/Cargo.toml`; keep all matcher/UI code out of the engine and ledger crates.
- Add tests in `crates/loop-cli/tests/e2e.rs` for candidate rendering/selection, exact state filtering, all three modes, latest non-interactive selection, missing ids, incomplete attempts, missing sessions, and operation with a missing machine. Use a temporary fake `LOOP_PI_BIN` that records argv/cwd and exits, or extend `crates/mock-pi/` if that produces a cleaner reusable fixture; do not launch an interactive real pi in tests. Unit-test the picker reducer and candidate mapping without requiring a PTY.

## Documentation work

Update, rather than create a second session-format document:

- `README.md`: mention `loop session` beside `status`/`logs` as the way to inspect what a Worker actually did, and show the state-filtered/latest forms.
- `docs/01-getting-started.md`: add picking the latest or a named stage session to “When it stops,” including the non-interactive `--latest` escape hatch.
- `docs/02-how-it-works.md`: document that `state_entered.session_id` is the link to pi's authoritative Worker history, distinguish it from the concise loop ledger, and explain the picker modes and evidence labels. Add the command under “Inspecting a run.”
- `docs/04-cli-reference.md`: document the interactive and `--latest` forms, state filtering, `Ctrl+O` modes, terminal requirements, human-readable rows, warnings, dependency on pi's session store, stdout/stderr behavior, and exit behavior; update the subcommand list/count.
- Link to pi's upstream session documentation rather than restating its file format or UI controls.

## Non-goals

- Persisting duplicate Worker transcripts in `.loop/`.
- Reading or rendering pi session JSONL inside loop.
- Making `fzf` a runtime dependency.
- Sessions for Judge or Navigator roles.
- Continuing the orchestration stage from the reopened interactive session; this is human inspection and optional follow-up, not `loop resume`.
