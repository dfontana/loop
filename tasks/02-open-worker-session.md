# Open a Worker's pi session

## Outcome

Make the Worker's full history available through pi's existing session UI instead of copying, parsing, or persisting a second transcript in loop.

This is viable with the current data model: each `state_entered` ledger event already records the deterministic `session_id` passed to the Worker. pi persists that session and can reopen it with `pi --session <id>`.

Judge and Navigator spawns remain intentionally sessionless; this command is for Worker stages only.

## CLI contract

```sh
loop session                         # latest Worker attempt
loop session implement               # latest attempt for a state
loop session implement --cycle 2     # latest attempt in that cycle
loop session implement --cycle 2 --attempt 1
```

- Select by reverse ledger order, not timestamp sorting.
- With no filters, select the latest `state_entered` event that has a non-empty `session_id`.
- A state positional argument filters by exact state id. `--cycle` and `--attempt` further filter it; require the state argument when either is used so the selection cannot silently cross states.
- Before launching, print one concise line identifying state, cycle, attempt, and session id.
- Start the configured pi binary in the project directory with inherited stdin/stdout/stderr and `--session <id>`. Use `--session`, not `--session-id`: opening history should fail if the recorded session is gone, not create an empty replacement with the same id.
- Respect `LOOP_PI_BIN`. This does not need a valid machine or toolbox; the ledger event and project path are sufficient, and `LOOP_PI_BIN` is available from `Config::defaults(paths)`.
- Propagate a failed pi launch or non-zero pi exit as command failure.
- If the chosen attempt has no later `worker_output`, warn that the session may still be active or may have crashed. Do not parse or mutate the session file.
- If no matching event or usable session id exists, return a specific error that includes the requested filters.

## Why this is the primary history mechanism

pi sessions already contain assistant messages, tool calls, tool results, commands, usage, and branching. loop should retain only the durable session id needed to find that history. It should not grow its own transcript format, session-file locator, exporter, or TUI.

## Implementation work

- Add the `Session` subcommand and its positional/flag arguments in `crates/loop-cli/src/main.rs`.
- Implement ledger selection and interactive pi launch in `crates/loop-cli/src/commands.rs`; extract a pure selector helper so matching behavior is unit-testable.
- Keep session selection based on `EventPayload::StateEntered`; no ledger schema change should be necessary because `session_id`, `state`, `cycle`, and `attempt` are already present in `crates/loop-core/src/event.rs` and emitted by `crates/loop-engine/src/lib.rs`.
- Verify that `crates/loop-cli/src/stage.rs` continues assigning one stable id per state/cycle/attempt and `crates/loop-runner/src/command.rs` continues passing it to pi as `--session-id` when creating the Worker session.
- Add tests in `crates/loop-cli/tests/e2e.rs` for selection, filters, missing ids, missing sessions, and operation with a missing machine. Use a temporary fake `LOOP_PI_BIN` that records argv/cwd and exits, or extend `crates/mock-pi/` if that produces a cleaner reusable fixture; do not launch an interactive real pi in tests.

## Documentation work

Update, rather than create a second session-format document:

- `README.md`: mention `loop session` beside `status`/`logs` as the way to inspect what a Worker actually did.
- `docs/01-getting-started.md`: add reopening the latest or a named stage session to “When it stops.”
- `docs/02-how-it-works.md`: document that `state_entered.session_id` is the link to pi's authoritative Worker history, and distinguish it from the concise loop ledger. Add the command under “Inspecting a run.”
- `docs/04-cli-reference.md`: document selection precedence, flags, warnings, dependency on pi's session store, stdout/stderr behavior, and exit behavior; update the subcommand list/count.
- Link to pi's upstream session documentation rather than restating its file format or UI controls.

## Non-goals

- Persisting duplicate Worker transcripts in `.loop/`.
- Reading or rendering pi's session JSONL inside loop.
- Sessions for Judge or Navigator roles.
- Continuing the orchestration stage from the reopened interactive session; this is human inspection and optional follow-up, not `loop resume`.
