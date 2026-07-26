# Add `loop recap`

## Outcome

Give the operator a deterministic post-run answer to “what happened and why?” without requiring ledger queries. `preview` explains the declaration before a run; `recap` explains the observed execution after or during one.

The ledger remains the source of truth. Recap is a report over it, not another state or history store.

## CLI contract

```sh
loop recap
loop recap > run-recap.md
```

Print a stable, Markdown-friendly report to stdout containing:

1. **Run summary** — ticket, outcome/current state, terminal or interruption, machine hash, effective run budgets recorded at start, totals, cycle counts, and Navigator invocation count.
2. **Attempt timeline** — one section per `state_entered`, grouped in ledger order, with state/cycle/attempt, model/thinking, skills, MCP, session id, Worker summary and usage, artifacts, proposal/rationale, Navigator choice, guard outcomes, check output, Judge rationale/usage, committed destination, retries, and errors that occurred before the next attempt.
3. **Why it ended** — terminal transition or the fatal guardrail/error; for an unfinished run, the folded resume point and the last durable event.
4. **Inspection pointers** — `loop session …` commands for Worker attempts and `loop logs --raw | jq …` for the complete event stream.

Additional behavior:

- Support an active or interrupted run by reporting “recap to date”; completion is not required.
- An empty ledger is an explicit command error, not an empty report.
- Do not use an LLM. The report must be reproducible from the same ledger.
- Do not omit failed attempts merely because they produced no commit.
- Do not treat Worker summaries or artifact claims as independent proof. Label Worker-authored testimony separately from harness checks and Judge decisions.
- The report must still work if `machine.fnl` is missing or invalid. It may load the current machine only to enrich labels or compare hashes. If the current machine hash differs from `run_started.machine_hash`, print a warning and do not use current declarations to explain historical decisions.

## Implementation work

- Add the `Recap` subcommand in `crates/loop-cli/src/main.rs`.
- Add command orchestration in `crates/loop-cli/src/commands.rs`.
- Create a dedicated report/timeline module, preferably `crates/loop-cli/src/report.rs`, that associates events with attempts without altering fold/resume semantics. Keep event grouping pure and unit-testable.
- Reuse `loop_core::fold` / `fold_with_loop_heads` for totals, status, cycles, and resume state; do not implement a second fold.
- Reuse the same event wording/helpers as `status` and `logs` where concise summaries appear, while preserving full check output and Judge rationale in the detailed attempt sections.
- If machine hashing is not currently exposed at the CLI boundary, add only the narrow helper needed to compare the loaded machine with the ledger's recorded hash. Do not snapshot or create another manifest as part of this task.
- Add fixtures/tests in `crates/loop-cli/tests/e2e.rs` (and focused unit tests for grouping) covering:
  - successful, failed, aborted, active, and crash-interrupted runs;
  - retries and multiple cycles/attempts of the same state;
  - Navigator routing and guard-failure routes;
  - checks and criteria independently passing/failing/skipping;
  - missing Worker output, session id, artifacts, or machine;
  - current-machine hash mismatch;
  - stable ordering and no loss of early events beyond `status`'s recent window.
- `examples/local/ledger.jsonl` is a useful end-to-end fixture; add a documented expected recap excerpt or golden output only if it remains maintainable.

## Documentation work

Update existing docs rather than creating a competing audit guide:

- `README.md`: add `loop recap` to the after-run commands.
- `docs/01-getting-started.md`: make recap the first post-run narrative, with `status`, `logs`, raw ledger, and `session` as progressively deeper views.
- `docs/02-how-it-works.md`: add recap under “Inspecting a run,” explain its evidence labels, and retain the direct-ledger recipes for advanced queries.
- `docs/04-cli-reference.md`: document output sections, partial-run behavior, machine-hash warning, stdout, and exit behavior; update the subcommand list/count.
- `examples/README.md`: show recap against the worked ledger if a stable example is added.
- No new standalone document is required unless `docs/02-how-it-works.md` becomes unwieldy; it is already the canonical audit/inspection document.

## Relationship to `preview`

Keep the visual vocabulary aligned: ticket, budgets, state, model, skills, MCP, transitions, guards, and loops should have the same labels and ordering in both commands. Share report types/formatters where useful, but do not force historical ledger events into a preview-only model or vice versa.

## Non-goals

- AI-generated summaries.
- Duplicate transcripts or artifact snapshots.
- Run archives, approvals, pause/resume controls, or dashboards.
- Replacing the pi session as the authoritative detailed Worker history.
