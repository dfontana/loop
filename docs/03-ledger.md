# 03 — The ledger

The ledger is one append-only JSONL file per run (`./.loop/ledger.jsonl`). It is
**event-sourced**: the current state of the run is not stored, it is *folded*
from the events. This is the same trick Temporal/Restate/DBOS use for durable
execution, and it's what makes the run resumable, auditable, and cheap to reason
about.

Design rules:

- **Append-only, one JSON object per line.** Never rewrite. A crash mid-write
  costs at most the last (partial) line, which the reader discards.
- **Small and greppable.** No transcripts, no large blobs in the ledger body.
  Big outputs are **artifacts** on disk, referenced by path + content hash.
- **The ledger is the source of truth for control flow.** Full agent transcripts
  live in pi's own session files (referenced by `session_id`); the ledger holds
  the decisions, not the reasoning.

## Event types

Every line has `ts` (ISO-8601) and `type`. Payload by type:

| `type` | Payload | Meaning |
|---|---|---|
| `run_started` | `ticket, machine_hash, resolved_config, config: {budget_usd, wallclock_s, max_transitions}` | Snapshot of the fully-resolved machine + toolbox at start, so mid-run edits don't change behavior. |
| `state_entered` | `state, cycle, attempt, session_id, model, thinking, tools[]` | A worker is about to run this stage. |
| `worker_output` | `state, cycle, summary, artifacts: [{name, path, sha256}], usage: {tokens, cost_usd}` | Digest of what the worker did. |
| `transition_proposed` | `from, to \| blocked:true, rationale, by: "worker"` | The worker's `transition` tool call. |
| `guard_checked` | `from, to, structural, when, criteria, judge_rationale` | Result of the three guard tiers. |
| `navigator_invoked` | `from, proposal, chosen_to, entry_prompt_path, usage` | Only when the proposal was invalid/blocked. |
| `transition_committed` | `from, to, cycle` | The move is official; current state advances. |
| `vars_set` | `scope, values: {...}` | Structured variables extracted for `when` guards (e.g. `{build:{status:"pass"}}`). |
| `error` | `state, kind: transient\|fatal, detail, retry_of` | A failure, classified. |
| `note` | `text` | Human or harness annotation. |
| `run_finished` | `status: done\|failed\|aborted, terminal_state, totals: {cost_usd, wallclock_s, transitions}` | Terminal. |

## Folding to current state

The reducer is total and deterministic:

```text
fold(events):
    current   = machine.entry
    cycles    = {}          # state → count of times entered as a loop head
    attempts  = {}          # (state,cycle) → count
    vars      = {}          # structured ledger vars for `when`
    for e in events:
        match e.type:
            state_entered        → attempts[(e.state,e.cycle)] += 1
            vars_set             → vars = deep_merge(vars, e.values)
            transition_committed → current = e.to
                                   if is_loop_head(e.to): cycles[e.to] += 1
            run_finished         → return DONE(e.status)
    return RUNNING(current, cycles, attempts, vars)
```

`loop status` is just a pretty-printer over this fold. `loop resume` runs the
fold, then:

- If the last event is `run_finished`, nothing to do.
- If the last `state_entered` has **no** following `worker_output`, that stage
  crashed mid-flight → **re-enter it** (a fresh worker, `attempt+1`). Stages must
  therefore be **idempotent enough** to re-run — see the note below.
- If a `worker_output` exists but no `transition_committed`, resume at the guard
  check for that proposal.

Because state is derived, there's no separate "state file" to corrupt or to get
out of sync with the log.

## Structured vars vs prose

`when` guards need machine-readable inputs, not "the build looked fine". Two ways
vars get into the ledger:

1. **Tool-emitted** — a `scoped-tools` command like `spark_build` prints a final
   `LOOP_VARS {"build":{"status":"pass","id":"b-8842"}}` line the harness scrapes
   into a `vars_set` event. The tool asserts the fact; the model can't fake it.
2. **Worker-declared** — the `transition` tool accepts an optional
   `vars` object the worker fills in. Convenient, but **untrusted** — never gate a
   QA pass on a worker-declared var; require a tool-emitted one.

This is the mechanism that keeps "did it actually pass?" grounded in real command
exit codes rather than the worker's optimism (see [07-risks.md](07-risks.md)).

## Artifacts

- Written to `./.loop/artifacts/<state>-<cycle>-<name>` with a sibling `.sha256`.
- Referenced from `worker_output.artifacts` and injectable downstream as
  `$ARTIFACT_<NAME>` in prompts and as hidden-param inputs to tools.
- Write via temp-file + atomic rename so a crash never leaves a half-written
  artifact a later stage might read.

## Idempotency & re-entry

Re-running a crashed stage must not double-apply side effects. Guidance:

- **Reads and pure compute** re-run freely.
- **Mutations** (deploys, migrations, PR creation) should be keyed by
  `$TICKET-$STATE-$CYCLE` so the underlying tool is idempotent (create-or-get,
  not create-again). The cycle id is exactly the injectable identity the design
  already provides — reuse it as the idempotency key.
- The `open_pr` stage checks for an existing PR for the branch before opening one.

## Example (abbreviated)

```jsonl
{"ts":"2026-07-23T18:00:01Z","type":"run_started","ticket":"PROJ-1487","machine_hash":"sha256:9f…","config":{"budget_usd":8,"wallclock_s":5400,"max_transitions":40}}
{"ts":"2026-07-23T18:00:02Z","type":"state_entered","state":"implement","cycle":1,"attempt":1,"session_id":"01f9…","model":"claude-sonnet-5","thinking":"high","tools":["read","edit","write","bash","spark_build"]}
{"ts":"2026-07-23T18:07:44Z","type":"vars_set","scope":"build","values":{"build":{"status":"pass","id":"b-8842"}}}
{"ts":"2026-07-23T18:07:45Z","type":"worker_output","state":"implement","cycle":1,"summary":"Added churn_score column + API field; build green.","artifacts":[{"name":"diff","path":".loop/artifacts/implement-1-diff.patch","sha256":"…"}],"usage":{"tokens":48211,"cost_usd":0.62}}
{"ts":"2026-07-23T18:07:45Z","type":"transition_proposed","from":"implement","to":"review","rationale":"Plan items done, build green.","by":"worker"}
{"ts":"2026-07-23T18:08:10Z","type":"guard_checked","from":"implement","to":"review","structural":"pass","when":"skip","criteria":"pass","judge_rationale":"All three checklist items present in diff; no TODO markers."}
{"ts":"2026-07-23T18:08:10Z","type":"transition_committed","from":"implement","to":"review","cycle":1}
```

See [`examples/ledger.jsonl`](../examples/local/ledger.jsonl) for a full run trace
including a transient-QA retry and a navigator reconciliation.
