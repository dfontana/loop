# examples

A complete worked ticket, PROJ-1487: add a churn-score field to a Spark retention pipeline and expose it through the API.

The two directories here **are** the two-location model, not just a grouping:

- **[`local/`](local/)** — a ticket's `./.loop/`. Unique to PROJ-1487, thrown away when the ticket is done.
- **[`toolbox/`](toolbox/)** — the portable `~/.config/loop/`. Reused across every ticket, untouched by this one.

A stage's `:playbook` resolves local-first, then toolbox, so `local/` wins on a name clash. Everything referenced here is present: no file points at a playbook, skill, or script that doesn't exist.

See [Where configuration lives](../docs/03-customizing.md#where-configuration-lives) for the rules these directories illustrate.

## Verify it

This is a static smoke test — it needs only Rust and Cargo, and never invokes pi, staging, or any external service. It stages both locations in a disposable sandbox, validates the machine, then folds the recorded ledger:

```sh
fixture="$(mktemp -d)"
mkdir -p "$fixture/project/.loop" "$fixture/config/loop" "$fixture/state"
cp -R examples/local/. "$fixture/project/.loop/"
cp -R examples/toolbox/. "$fixture/config/loop/"

LOOP_CONFIG_DIR="$fixture/config/loop" LOOP_STATE_DIR="$fixture/state" \
  cargo run --quiet -p loop-cli -- --dir "$fixture/project" validate
LOOP_CONFIG_DIR="$fixture/config/loop" LOOP_STATE_DIR="$fixture/state" \
  cargo run --quiet -p loop-cli -- --dir "$fixture/project" status --json

rm -rf "$fixture"
```

Expected output:

```
PROJ-1487 — 6 states, 10 transitions, no problems found
{
  "current": "done",
  "cycles": {
    "qa-staging": 3
  },
  "navigator_invocations": 0,
  "status": "done",
  "totals": {
    "cost_usd": 3.58,
    "transitions": 10,
    "wallclock_s": 3414
  }
}
```

Actually _running_ this machine would additionally need pi, the `warehouse` MCP server in your own `~/.pi/agent/mcp.json`, and the Spark and staging credentials the example's skills reach for.

## `local/` — the per-ticket `./.loop/`

| File | What it is |
| --- | --- |
| [`machine.fnl`](local/machine.fnl) | The ticket machine — what `loop run` loads. Six states, ten transitions, two declared loops. |
| [`task.md`](local/task.md) | The ticket, as prose. Reaches playbooks as `$TASK`. |
| [`plan.md`](local/plan.md) | The plan. Reaches playbooks as `$PLAN`. |
| [`playbooks/validate-contract.md`](local/playbooks/validate-contract.md) | A bespoke, ticket-specific stage prompt — resolves local-first over the toolbox. |
| [`ledger.jsonl`](local/ledger.jsonl) | The full run trace this machine produced. |

## `toolbox/` — the reusable `~/.config/loop/`

| Entry | What it is |
| --- | --- |
| [`config.fnl`](toolbox/config.fnl) | Global defaults: provider, worker/judge/navigator models, budgets, context. |
| [`playbooks/implement.md`](toolbox/playbooks/implement.md) | Generic implement stage. |
| [`playbooks/review.md`](toolbox/playbooks/review.md) | Adversarial review, four-angle fan-out. |
| [`playbooks/qa.md`](toolbox/playbooks/qa.md) | Grounded, evidence-backed QA. Reused by `qa-staging`. |
| [`playbooks/debug-spark.md`](toolbox/playbooks/debug-spark.md) | Diagnose a _real_ pipeline failure and fix it. |
| [`playbooks/open-pr.md`](toolbox/playbooks/open-pr.md) | Assemble the PR body from the ledger and open it. |
| [`skills/spark-build/`](toolbox/skills/spark-build) | Build and unit-check the pipeline. `build.sh` also backs two edge checks. |
| [`skills/spark-run/`](toolbox/skills/spark-run) | Run a job. `classify.sh` owns the transient-vs-real taxonomy and backs all three edges out of `qa-staging`. |
| [`skills/staging-deploy/`](toolbox/skills/staging-deploy) | Deploy to a cycle-scoped namespace. |
| [`skills/contract-check/`](toolbox/skills/contract-check) | Validate a staging response against the OpenAPI spec. |
| [`skills/open-pr/`](toolbox/skills/open-pr) | Open or update the branch's PR, idempotently. |
| [`skills/ci-status/`](toolbox/skills/ci-status) | Generic CI read/wait — a library item this ticket doesn't load. |
| [`skills/debug-transient.md`](toolbox/skills/debug-transient.md) | A bare-`.md` skill: the transient-vs-real checklist the `debug` stage loads. |
| [`machines/standard-ticket.fnl`](toolbox/machines/standard-ticket.fnl) | Machine template: the plain code-only spine. |
| [`machines/data-pipeline-ticket.fnl`](toolbox/machines/data-pipeline-ticket.fnl) | The template PROJ-1487 is derived from. |
| [`ext/`](toolbox/ext) | The three vendored tools — `transition`, `verdict`, `choose`. |

## What is loop's own, and what isn't

`ext/*.ts` are loop's own code. They are compiled into the binary and written here automatically; you never author them, and hand edits are reverted on the next `loop init` or `loop run`.

Everything else in `toolbox/` is ordinary content you write. Skills use pi's own loader — loop resolves a name to a path and passes `--skill <path>`, and never parses the format. There is no `mcp.json` here on purpose: a state's `:mcp` names servers out of _your_ `~/.pi/agent/mcp.json`, and the stage connects them itself.

## Suggested reading order

1. [`local/machine.fnl`](local/machine.fnl) — skim it for the shape of the graph. The three-way fail routing out of `qa-staging` is the part worth slowing down for: a transient flake retries in place with backoff, a real failure spawns the debugger, a pass moves on — and each branch is decided by one script's exit code rather than by an agent's judgment.
2. `loop diagram` on it, to see that graph drawn.
3. [`local/ledger.jsonl`](local/ledger.jsonl) top to bottom, next to [How a run works](../docs/02-how-it-works.md). The trace is the fastest way to feel how the pieces move.
4. `loop recap` on the same fixture, for that trace as a narrative: ten attempts across six states, each with the Worker's account, the check output, and the Judge's rationale labelled separately. Note the stderr warning it prints — `machine_hash` in this recorded ledger is the elided `9f3c…` rather than a real digest, so the recap correctly refuses to let the machine beside it explain the run.
