# 06 — End-to-end walkthrough

A concrete ticket, start to finish, so the abstractions land.

## The ticket

> **PROJ-1487** — Add a `churn_score` column to the retention Spark pipeline and
> expose it on `GET /accounts/:id`. Backfill 30 days.

Two moving parts that make it a good example: a **Spark job** that can fail for
*real* reasons (bad SQL, schema drift) or *transient* ones (a flaky executor, a
staging cluster hiccup), and an **API contract** that must be validated
separately from the pipeline.

## Authoring, live in harness

You open pi interactively with the `loop-author` playbook and the task text. You
and the agent shape `./.loop/machine.fnl`: pick the states, wire the loops,
write the QA criteria, choose which toolbox tools each stage gets. This is the
"work with an agent to plan the implementation live" step. The output is
[`examples/local/machine.fnl`](../examples/local/machine.fnl) — the same file
`loop run` drives.

## The graph

```
        ┌──────────┐
        │  plan    │  (given — you wrote it during authoring)
        └────┬─────┘
             ▼
        ┌──────────┐   build red / review found blockers
        │implement │◀──────────────────────────────┐
        └────┬─────┘                                 │
             ▼                                        │
        ┌──────────┐  blockers                        │
        │  review  │──────────────────────────────────┤
        └────┬─────┘                                   │
             ▼ clean                                    │
        ┌──────────┐  fail (real error)     ┌──────────┴─┐
        │qa-staging│───────────────────────▶│   debug     │
        └────┬─────┘◀──────────────────────┐└─────────────┘
             │        fail (transient)      │  (self-loop retry)
             │ pass                         │
             ▼                              │
      ┌───────────────┐                     │
      │validate-       │  contract mismatch │
      │contract        │────────────────────┘  (→ implement)
      └────┬───────────┘
           ▼ pass
        ┌──────────┐
        │ open-pr  │──▶ ● done
        └──────────┘
```

Loops with bounded cycles: `qa = {qa-staging, debug}` max 4; the
`qa-staging → qa-staging` transient self-loop max 3.

## What actually happens (the ledger trace)

Abbreviated from [`examples/local/ledger.jsonl`](../examples/local/ledger.jsonl). Read it as
"harness action → why".

1. **`run_started`** — harness records the machine hash and budgets (`$8`, 90
   min, 40 transitions).

2. **`implement` (cycle 1)** — spawns a worker with
   `claude-sonnet-5` at high thinking and the `read`, `edit`, `write`, `bash`,
   and `spark_build` tools. The worker writes the column + API field, runs
   `spark_build` (which prints
   `LOOP_VARS {"build":{"status":"pass","id":"b-8842"}}` → `vars_set`), calls
   `transition(to="review", …)`.

3. **Guard implement→review** — structural pass; no `when`; **Judge** (haiku,
   low) reads the diff artifact against the criteria "checklist addressed, builds
   clean, no TODOs" → `pass`. `transition_committed`.

4. **`review` (cycle 1)** — this stage's playbook *is* your `run-review` skill:
   the worker is a coordinator that fans out adversarial sub-reviewers. It finds
   a real issue (backfill window off by a day) and calls
   `transition(to="implement", rationale="backfill covers 29 days not 30")`.

5. **Guard review→implement** — an intentional back-edge; committed.
   `implement` re-enters as attempt 2 of its first cycle.

6. **`implement` (attempt 2)** — this fresh worker receives the ledger digest and
   earlier artifacts, sees the review finding, fixes the window, and builds green
   again → `review`.

7. **`review` (attempt 2)** — clean → `qa-staging`.

8. **`qa-staging` (cycle 1)** — spawns with a **read-only** tool set
   (`read, bash, staging_deploy, spark_run, fetch_job_output` — no `edit`).
   Deploys to namespace `loop-PROJ-1487-1` (cycle-scoped), runs the Spark job,
   fetches output. The job **fails**: `fetch_job_output` emits
   `LOOP_VARS {"qa":{"result":"fail","error_class":"transient","detail":"executor lost"}}`.

9. **Routing on the fail** — the machine has two edges out of `qa-staging` for a
   fail. The `when` guard `qa.error_class == 'transient'` selects the
   **self-loop**. Harness retries `qa-staging` (cycle 2, transient-retry counter
   1) with backoff. This is the "debug transient problems, retest" path — no
   debug agent spawned, no code touched, just a re-run.

10. **`qa-staging` (cycle 2)** — job runs, but now a *real* failure:
    `error_class: "real"`, detail "column churn_score not found in gold schema".
    `when: qa.error_class != 'transient'` selects `→ debug`.

11. **`debug` (cycle 1)** — playbook `debug-spark`, with `use_playbook` tool so it
    can pull `debug-transient` guidance if it turns out to be flaky after all. It
    diagnoses a missing schema-registry migration, fixes it, rebuilds. Proposes
    `transition(to="qa-staging")`.

12. **`qa-staging` (cycle 3)** — passes:
    `LOOP_VARS {"qa":{"result":"pass"}}`. `when: qa.result == 'pass'` →
    `validate-contract`.

13. **`validate-contract`** — its prompt is a **local, per-ticket playbook**
    (`./.loop/playbooks/validate-contract.md`, resolved local-first over the
    toolbox), written for this endpoint and these fields rather than overloading
    the generic `qa` playbook. It hits the staging API and checks the response
    schema against the OpenAPI spec (the `contract_check` command, which emits the
    gating `LOOP_VARS`). Matches. → `open-pr`.

14. **`open-pr`** — `open-pr` tool is idempotent (checks for an existing PR for
    the branch first), opens the PR with a body assembled from the ledger digest
    (what changed, what QA ran, the cycle count). `transition(to="done")`.

15. **`run_finished`** — `status: done`, totals: `$3.44`, 57 min, 10 transitions.

## A navigator moment

Suppose at step 11 the debug worker had instead concluded it needed data it could
only get by *re-implementing* differently, and proposed
`transition(to="implement")`. If `debug → implement` **isn't** a declared edge,
the harness fires the **Navigator**: it sees the graph, the debug rationale, and
the reachable neighbors of `debug` (`qa-staging`). It either routes to
`qa-staging` with an entry addendum ("the schema fix is in; re-run QA and capture
the gold schema so implement has what it needs") or, if it judges implement truly
necessary and unreachable, chooses `escalate`. Either way the choice is one cheap
constrained call, logged as `navigator_invoked`.

## What you touched vs what the toolbox gave you

- **You wrote (per-ticket, discarded after):** `task.md`, `plan.md`, the
  structured `qa_cases`, one bespoke local playbook
  (`playbooks/validate-contract.md`), and ~40 lines of `machine.fnl` wiring —
  mostly picking states and thresholds.
- **The toolbox gave you (reused, untouched):** `implement`, `review` (=
  run-review), `qa`, `debug-spark`, `debug-transient`, `open-pr` playbooks; the
  `spark_build`, `spark_run`, `staging_deploy`, `fetch_job_output`,
  `contract_check`, `open-pr` tools; and the `standard-ticket` machine template
  you copied from.

That ratio — a few dozen lines of unique wiring over a fat library of primitives
— is the whole point.
