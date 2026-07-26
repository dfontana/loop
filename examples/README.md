# examples

Concrete, runnable-shaped config for the PROJ-1487 ticket used throughout the
docs.

The two directories here **are** the two-location model, not just a grouping:

- **[`local/`](local/)** = a ticket's `./.loop/` — unique to PROJ-1487, thrown
  away when the ticket is done.
- **[`toolbox/`](toolbox/)** = the portable `~/.config/loop/` — reused across
  every ticket, untouched by this one.

> **Updated for v1** ([docs/09](../docs/09-implementation-plan.md)). Machines
> and global config are **Fennel**, in the plain-table schema
> `crates/loop-fennel/src/convert.rs` documents; the toolbox lives in
> `~/.config/loop/`. [`local/machine.yaml`](local/machine.yaml) is kept only as
> the side-by-side comparison [docs/02](../docs/02-language.md) argues over — no
> YAML loader exists.

A stage's `playbook:` resolves **local-first**, then toolbox — so `local/` wins on
a name clash (see [docs/04](../docs/04-toolbox.md)). Everything is now filled in:
no file references a playbook/tool/script that isn't present.

## Verify the example

This static smoke test needs only Rust/Cargo; it does not invoke pi, staging, or
external tools. It stages the two example locations in a disposable sandbox,
then validates the machine and folds its recorded ledger:

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

A live `loop run` additionally needs pi with the configured extensions plus the
Spark/staging credentials and binaries named by the example tools.

## `local/` — per-ticket (`./.loop/`), discarded after PROJ-1487

| File | What it is | Doc |
|---|---|---|
| [`local/machine.fnl`](local/machine.fnl) | **The ticket machine** — what `loop run` actually loads. References prose + playbooks + tools by path/name. | [02](../docs/02-language.md), [06](../docs/06-example-walkthrough.md), [09](../docs/09-implementation-plan.md) |
| [`local/machine.yaml`](local/machine.yaml) | Historical pre-v1 YAML sketch retained for the language comparison; nothing loads it. | [02](../docs/02-language.md) |
| [`local/task.md`](local/task.md) | The ticket task, prose. `task: task.md` → `$TASK`. | [04](../docs/04-toolbox.md) |
| [`local/plan.md`](local/plan.md) | The plan, co-authored live. `plan: plan.md` → `$PLAN`. | [04](../docs/04-toolbox.md) |
| [`local/playbooks/validate-contract.md`](local/playbooks/validate-contract.md) | A **bespoke, local** stage prompt — resolves local-first over the toolbox. | [04](../docs/04-toolbox.md) |
| [`local/ledger.jsonl`](local/ledger.jsonl) | The full run trace `machine.fnl` produces — read alongside doc 06. | [03](../docs/03-ledger.md), [06](../docs/06-example-walkthrough.md) |

## `toolbox/` — the reusable globals (`~/.config/loop/`), untouched by this ticket

| File | What it is | Backed by |
|---|---|---|
| [`toolbox/config.fnl`](toolbox/config.fnl) | Global defaults: provider, worker/judge/navigator models, budgets, which extensions load. | loop |
| [`toolbox/playbooks/implement.md`](toolbox/playbooks/implement.md) | Generic implement playbook (== a pi skill). | ~ [`run-plan`](../../pi-extensions/skills/run-plan) |
| [`toolbox/playbooks/review.md`](toolbox/playbooks/review.md) | Adversarial review; `select_review_model` + four-angle fan-out. | == [`run-review`](../../pi-extensions/skills/run-review) |
| [`toolbox/playbooks/qa.md`](toolbox/playbooks/qa.md) | Grounded, evidence-backed QA. Reused by `qa-staging`. | loop |
| [`toolbox/playbooks/debug-spark.md`](toolbox/playbooks/debug-spark.md) | Diagnose a *real* pipeline QA failure and fix it; bound to `debug`. | loop |
| [`toolbox/playbooks/open-pr.md`](toolbox/playbooks/open-pr.md) | Assemble the PR body from the ledger and open/update the PR. | loop |
| [`toolbox/skills/spark-build/`](toolbox/skills/spark-build) | Build + unit-check the pipeline. `build.sh` is also the `implement → review` and `debug → qa-staging` edge check. | pi skill |
| [`toolbox/skills/spark-run/`](toolbox/skills/spark-run) | Run a job; `classify.sh` owns the transient/real taxonomy and backs all three edges out of `qa-staging` via `--expect`. | pi skill |
| [`toolbox/skills/staging-deploy/`](toolbox/skills/staging-deploy) | Deploy to a cycle-scoped namespace; validates its own env argument, fetches its own token. | pi skill |
| [`toolbox/skills/contract-check/`](toolbox/skills/contract-check) | Validate a staging response against the OpenAPI spec; also the `validate-contract → open-pr` check. | pi skill |
| [`toolbox/skills/open-pr/`](toolbox/skills/open-pr) | Open or update the branch's PR, idempotently. | pi skill |
| [`toolbox/skills/ci-status/`](toolbox/skills/ci-status) | Generic CI read/wait — a library item this ticket doesn't load. | pi skill |
| [`toolbox/skills/debug-transient.md`](toolbox/skills/debug-transient.md) | Transient-vs-real checklist; situational know-how the `debug` stage loads. | pi skill |
| [`toolbox/machines/standard-ticket.fnl`](toolbox/machines/standard-ticket.fnl) | Machine template: the plain code-only spine. | loop |
| [`toolbox/machines/data-pipeline-ticket.fnl`](toolbox/machines/data-pipeline-ticket.fnl) | Machine template PROJ-1487 is derived from. | loop |
| [`toolbox/ext/transition-tool.ts`](toolbox/ext/transition-tool.ts) | The Worker's transition tool. | loop (vendored) |
| [`toolbox/ext/verdict-tool.ts`](toolbox/ext/verdict-tool.ts) | The Judge's only tool. | loop (vendored) |
| [`toolbox/ext/choose-tool.ts`](toolbox/ext/choose-tool.ts) | The Navigator's only tool. | loop (vendored) |

> **"Backed by" matters.** Skills use pi's own loader — the harness resolves a
> name to a path and passes `--skill <path>`; it does not parse or rewrite the
> format. Anything marked with a `pi-extensions` link is an *existing installed
> package* loop configures, not code it ships: it activates `mcp` and
> `review-model-selector` per spawn and leaves their own configuration alone.
> There is no `mcp.json` here on purpose — a state's `:mcp` names servers out
> of *your* `~/.pi/agent/mcp.json`, and the stage connects them itself. Only
> the three `ext/*.ts` are loop's own.
> See [docs/04](../docs/04-toolbox.md#these-are-existing-pi-extensions-not-new-loop-code)
> and [docs/05](../docs/05-orchestration.md).

Suggested reading order: skim `local/machine.fnl`, then read `local/ledger.jsonl`
top to bottom next to [docs/06](../docs/06-example-walkthrough.md) — the trace is
the fastest way to feel how the pieces move.
