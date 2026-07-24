# examples

Concrete, runnable-shaped config for the PROJ-1487 ticket used throughout the
docs. These aren't wired to a real `loop` binary yet — they're the artifacts the
design implies, so the shapes are pinned before any code is written.

The two directories here **are** the two-location model, not just a grouping:

- **[`local/`](local/)** = a ticket's `./.loop/` — unique to PROJ-1487, thrown
  away when the ticket is done.
- **[`toolbox/`](toolbox/)** = the portable `~/.loop/` — reused across every
  ticket, untouched by this one.

A stage's `playbook:` resolves **local-first**, then toolbox — so `local/` wins on
a name clash (see [docs/04](../docs/04-toolbox.md)). Everything is now filled in:
no file references a playbook/tool/script that isn't present.

## `local/` — per-ticket (`./.loop/`), discarded after PROJ-1487

| File | What it is | Doc |
|---|---|---|
| [`local/machine.yaml`](local/machine.yaml) | The ticket machine, YAML surface. References prose + playbooks + tools by path/name. | [02](../docs/02-language.md), [06](../docs/06-example-walkthrough.md) |
| [`local/machine.fnl`](local/machine.fnl) | The *same* machine as a Fennel DSL — shows the logic/reuse ceiling. | [02](../docs/02-language.md) |
| [`local/task.md`](local/task.md) | The ticket task, prose. `task: task.md` → `$TASK`. | [04](../docs/04-toolbox.md) |
| [`local/plan.md`](local/plan.md) | The plan, co-authored live. `plan: plan.md` → `$PLAN`. | [04](../docs/04-toolbox.md) |
| [`local/playbooks/validate-contract.md`](local/playbooks/validate-contract.md) | A **bespoke, local** stage prompt — resolves local-first over the toolbox. | [04](../docs/04-toolbox.md) |
| [`local/ledger.jsonl`](local/ledger.jsonl) | The full run trace `machine.yaml` produces — read alongside doc 06. | [03](../docs/03-ledger.md), [06](../docs/06-example-walkthrough.md) |

## `toolbox/` — the reusable globals (`~/.loop/`), untouched by this ticket

| File | What it is | Backed by |
|---|---|---|
| [`toolbox/loop.config.yaml`](toolbox/loop.config.yaml) | Global defaults: provider, worker/judge/navigator models, budgets, which extensions load. | loop |
| [`toolbox/playbooks/implement.md`](toolbox/playbooks/implement.md) | Generic implement playbook (== a pi skill). | ~ [`run-plan`](../../pi-extensions/skills/run-plan) |
| [`toolbox/playbooks/review.md`](toolbox/playbooks/review.md) | Adversarial review; `select_review_model` + four-angle fan-out. | == [`run-review`](../../pi-extensions/skills/run-review) |
| [`toolbox/playbooks/qa.md`](toolbox/playbooks/qa.md) | Read-only QA; grounded pass/fail via `LOOP_VARS`. Reused by `qa_staging`. | loop |
| [`toolbox/playbooks/debug-spark.md`](toolbox/playbooks/debug-spark.md) | Diagnose a *real* pipeline QA failure and fix it; bound to `debug`. | loop |
| [`toolbox/playbooks/debug-transient.md`](toolbox/playbooks/debug-transient.md) | Transient-vs-real checklist, consumed as a tool via `use_playbook`. | loop |
| [`toolbox/playbooks/open-pr.md`](toolbox/playbooks/open-pr.md) | Assemble the PR body from the ledger and open/update the PR. | loop |
| [`toolbox/tools/spark.yaml`](toolbox/tools/spark.yaml) | `scoped-tools` for the Spark pipeline; emit gating `LOOP_VARS`. | [`scoped-tools`](../../pi-extensions/extensions/scoped-tools) |
| [`toolbox/tools/staging.yaml`](toolbox/tools/staging.yaml) | Deploy/contract/PR tools; safety guards, cycle-scoped idempotency, hidden secrets. | [`scoped-tools`](../../pi-extensions/extensions/scoped-tools) |
| [`toolbox/tools/ci.yaml`](toolbox/tools/ci.yaml) | Generic CI tools — library item this ticket doesn't bind. | [`scoped-tools`](../../pi-extensions/extensions/scoped-tools) |
| [`toolbox/tools/mcp.json`](toolbox/tools/mcp.json) | MCP server registry in real `.mcp.json` schema. | [`mcp`](../../pi-extensions/extensions/mcp) |
| [`toolbox/tools/bin/classify-spark.sh`](toolbox/tools/bin/classify-spark.sh) | The transient/real/unknown taxonomy `fetch_job_output` shells out to. | loop |
| [`toolbox/machines/standard-ticket.yaml`](toolbox/machines/standard-ticket.yaml) | Machine template: the plain code-only spine. | loop |
| [`toolbox/machines/data-pipeline-ticket.yaml`](toolbox/machines/data-pipeline-ticket.yaml) | Machine template PROJ-1487 is derived from. | loop |
| [`toolbox/ext/transition-tool.ts`](toolbox/ext/transition-tool.ts) | The Worker's transition tool. | loop (vendored) |
| [`toolbox/ext/verdict-tool.ts`](toolbox/ext/verdict-tool.ts) | The Judge's only tool. | loop (vendored) |
| [`toolbox/ext/choose-tool.ts`](toolbox/ext/choose-tool.ts) | The Navigator's only tool. | loop (vendored) |

> **"Backed by" matters.** Anything marked with a `pi-extensions` link is an
> *existing installed package* loop configures, not code it ships — the harness
> points `scoped-tools`/`mcp` at `toolbox/` via `PI_AGENT_DIR` and activates
> `review-model-selector` per spawn. Only the three `ext/*.ts` are loop's own.
> See [docs/04](../docs/04-toolbox.md#these-are-existing-pi-extensions-not-new-loop-code)
> and [docs/05](../docs/05-orchestration.md).

Suggested reading order: skim `local/machine.yaml`, then read `local/ledger.jsonl`
top to bottom next to [docs/06](../docs/06-example-walkthrough.md) — the trace is
the fastest way to feel how the pieces move.
