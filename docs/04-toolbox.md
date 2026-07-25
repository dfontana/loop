# 04 — The toolbox

> **Superseded in part by [09-implementation-plan.md](09-implementation-plan.md).**
> The toolbox lives at **`~/.config/loop/`**, not `~/.loop/`, and everything
> loop *generates* (the merged `scoped-tools.yaml`, `mcp.json`, rendered
> prompts) goes to **`~/.local/state/loop/`** — which is what `PI_AGENT_DIR`
> points at, so nothing generated lands in the directory you hand-edit.
> `loop.config.yaml` is now `config.fnl`. Everything else here holds.

The toolbox is the portable, out-of-project library the "hack it per ticket, then
discard" workflow draws on. A ticket's `./.loop/` directory holds only what's
*unique* to that ticket; everything reusable lives in `~/.loop/` and is
referenced by name.

```
~/.loop/                        # the toolbox — portable, versioned, outside any project
  loop.config.yaml              # global defaults: provider, models, budgets, judge/navigator models
  playbooks/                    # STAGES — reusable "how to do X" (prompt + default model/thinking)
    implement.md
    review.md
    qa.md
    debug-spark.md
    debug-transient.md
    open-pr.md
  tools/                        # TOOLS — pre-canned capabilities (scoped-tools YAML + mcp config)
    spark.yaml                  #   scoped-tools specs; harness merges tools/*.yaml → scoped-tools.yaml
    staging.yaml
    ci.yaml
    mcp.json                    #   .mcp.json for the `mcp` extension (see "Existing pi-extensions")
    bin/                        #   scripts the tool commandTemplates shell out to
      classify-spark.sh
  machines/                     # MACHINE TEMPLATES — starting points to copy
    standard-ticket.yaml
    data-pipeline-ticket.yaml
  ext/                          # loop's OWN harness extensions — vendored, not installed packages
    transition-tool.ts          #   Worker's transition tool
    verdict-tool.ts             #   Judge's only tool
    choose-tool.ts              #   Navigator's only tool

./.loop/                        # per-ticket — created by `loop init`, deleted when done
  machine.yaml                  # THE ticket (references playbooks/tools by name + task/plan by path)
  task.md                       # the ticket task — PROSE, referenced by machine as `task: task.md`
  plan.md                       # the plan you co-authored live — PROSE, `plan: plan.md`
  playbooks/                    # LOCAL, bespoke stage prompts for THIS ticket (override the toolbox)
    validate-contract.md        #   e.g. a stage the toolbox doesn't cover
  ledger.jsonl                  # the run record (gitignored)
  artifacts/                    # captured outputs
```

## Prose lives in markdown, referenced by the machine

Task and plan are prose you write and edit by hand (often co-authored live during
`loop plan`), so they live in their own markdown files — not stuffed into machine
config strings. The machine references them by path:

```yaml
task: task.md      # relative to the machine file
plan: plan.md
```

The harness reads their contents into the context namespace as `$TASK` and
`$PLAN`, which is how the `implement`/`qa`/`debug` playbooks below get the
specifics of *this* ticket while staying generic and reusable. An inline
`task: |` block is still allowed for a throwaway ticket; the file reference is the
default because prose belongs in markdown you can edit, diff, and read on its own.

There are exactly **two kinds of reusable thing**, and the distinction is the
core of the model.

## Playbooks (stages) — the "how"

A playbook is a stage's brain: a Markdown prompt plus default model/thinking. It
is, deliberately, **a pi skill** — same `SKILL.md` frontmatter shape your
`run-review`/`run-plan` already use — so the toolbox is compatible with pi's
skill discovery and you can invoke the same file interactively when authoring.

```markdown
---
name: implement
description: Implement the planned change, keep the build green, stop when the plan checklist is done.
model: claude-sonnet-5          # default; a machine state can override
thinking: high
---

# Implement

You are implementing ticket **$TICKET_ID**, cycle **$CYCLE**.

## The task
$TASK

## The plan
$PLAN

## What you have
- The repo at the current working tree.
- Prior context digest:
$LEDGER_DIGEST

## How to work
1. Implement the plan. Keep changes scoped to it.
2. Run `spark_build` after substantive changes; do not finish on a red build.
3. When the checklist is complete and the build is green, call
   `transition(to="review", rationale=…, artifacts=[…])`.
4. If you cannot make progress, call `transition(blocked=true, rationale=…)`
   and the harness will route you.
```

### Every stage has a prompt, and the prompt is its playbook

There is no separate "stage prompt" concept — **a stage's prompt *is* the markdown
file named by its `playbook:`.** A stage without a resolvable playbook is an error
`loop validate` catches. Resolution mirrors the `scoped-tools` global/project
merge you already have — **local overrides toolbox:**

1. `./.loop/playbooks/<name>.md` (per-ticket, bespoke) — wins if present.
2. `~/.loop/playbooks/<name>.md` (toolbox, reusable).

A `playbook:` value with a `/` or `./` is an exact path; a bare name runs the
lookup above. So a stage's prompt comes from one of three places:

- **A toolbox playbook**, when the stage is generic. `qa_staging` uses
  `playbook: qa` → `~/.loop/playbooks/qa.md`, specialized only by the injected
  `$TASK`/`$QA_CASES`/tools. This is why it *looked* like `validate_contract` had
  no prompt — it was reusing the generic `qa` playbook.
- **A local, per-ticket playbook**, when the stage is bespoke. `validate_contract`
  uses `playbook: validate-contract` → `./.loop/playbooks/validate-contract.md`,
  a prompt written for *this* endpoint and these fields. Dropping the file next to
  the machine is the whole mechanism — no toolbox edit, and it's discarded with
  the ticket. (See [`examples/playbooks/validate-contract.md`](../examples/local/playbooks/validate-contract.md).)
- **An inline prompt** on the state (`prompt: |`), for a one-off you don't even
  want as a file.

The rule of thumb: reach for a toolbox playbook first; if the stage's instructions
are specific to this ticket, write a local one; promote a local playbook into the
toolbox the second time you copy it into another ticket.

Playbooks then come in two *usage* modes, and the same file can serve both:

- **Directly bound to a state** — this state *is* a review, so its playbook is
  `review.md`. Most stages.
- **Offered as a tool** — a *toolkit* of situational know-how the worker calls
  when it hits the situation. E.g. `debug-spark.md` and `debug-transient.md` are
  handed to the `debug` stage as tools it may consult, rather than being the
  stage itself. A playbook-as-tool is exposed as a `use_playbook(name)` tool that
  returns the playbook's guidance for the worker to apply in-context.

Templating uses the `scoped-tools` convention you already have: `$UPPER_SNAKE`
placeholders filled from the **context namespace** (below). Unknown `$NAMES` pass
through untouched, so `$HOME` etc. still work.

## These are existing pi-extensions, not new loop code

Two of the three tool sources below are already-installed packages in
[`~/opencode/pi-extensions`](../../pi-extensions) — loop *configures* them, it
doesn't reimplement them. Keeping this straight avoids the trap of vendoring a
second copy:

| Toolbox piece | Backed by | How loop points it at the toolbox |
|---|---|---|
| `tools/*.yaml` scoped-tools | [`scoped-tools`](../../pi-extensions/extensions/scoped-tools) extension | harness exports `PI_AGENT_DIR=~/.loop` and merges `tools/*.yaml` into the one `scoped-tools.yaml` that extension reads |
| `tools/mcp.json` | [`mcp`](../../pi-extensions/extensions/mcp) extension | installed as `$PI_AGENT_DIR/mcp.json`; a stage lists `mcp` to reach it |
| `select_review_model` (in the `review` stage) | [`review-model-selector`](../../pi-extensions/extensions/review-model-selector) extension | activated per spawn; nothing to configure |
| `ext/*.ts` (transition/verdict/choose) | **loop's own**, vendored here | `-e`-injected per spawn (Worker / Judge / Navigator) |

The `implement`/`review` *playbooks* likewise mirror the
[`run-plan`](../../pi-extensions/skills/run-plan) /
[`run-review`](../../pi-extensions/skills/run-review) skills — same
`select_review_model` + four-angle adversarial fan-out, run as a loop stage
instead of interactively.

## Tools — the "what with"

A tool is a pre-canned capability bound into a stage. Three sources, all
already-solved in your pi setup:

1. **`scoped-tools` YAML** — the star of the show. A wrapped HTTP call or CLI
   invocation with typed, validated parameters and hidden call-time secrets. The
   agent never sees the command template or the token — only the tool's schema
   and its stdout. This is exactly where you push "as much static work as
   possible", per your goal. Example (`~/.loop/tools/staging.yaml`):

   ```yaml
   staging_deploy:
     description: Deploy the current branch to an isolated staging namespace for this ticket+cycle.
     parameters:
       branch:
         type: string
         description: Git branch to deploy
         validationCmd: git rev-parse --verify "$1" >/dev/null
     hiddenParameters:
       token:   { valueFromCmd: pass show staging/deploy-token }
       ns:      { valueFromCmd: echo "loop-$TICKET_ID-$CYCLE" }   # cycle-scoped, idempotent
     commandTemplate: |
       stagectl deploy --branch $BRANCH --namespace $NS --token $TOKEN --wait
     timeout: 600
   ```

   The harness makes `$TICKET_ID` and `$CYCLE` available to `valueFromCmd`, so
   the namespace is unique per cycle — the injectable cycle identity doubles as
   an idempotency key.

2. **MCP servers** — via your [`mcp`](../../pi-extensions/extensions/mcp)
   extension's single proxy tool, configured by `tools/mcp.json`. Bind an MCP
   server to a stage when it needs a rich external surface (Linear, a data
   warehouse, a browser). The stage's tool allowlist includes `mcp`. Note that
   extension defaults every server *off* per session, so a headless stage needs
   its server pre-enabled.

3. **Playbooks-as-tools** — `use_playbook(name)` as described above, for
   situational know-how.

## Per-stage binding

A state names the tools it gets; the harness translates that into the pi spawn's
`--tools` allowlist and the `-e`/config it loads. QA stages deliberately *omit*
`edit`/`write` so a validation stage can't quietly "fix" what it's supposed to be
judging.

```yaml
states:
  qa_staging:
    playbook: qa
    tools: [read, bash, staging_deploy, spark_run, fetch_job_output]   # note: no edit/write
  debug:
    playbook: debug-spark
    tools: [read, edit, bash, spark_build, use_playbook]               # can consult debug-transient
```

Defaults (`read, bash`) come from `loop.config.yaml`; the state list is the
allowlist on top. Dangerous defaults can be dropped per-state with an exclude
list that maps to pi's `--exclude-tools`.

## The context namespace

The variables available to playbook templates and to tool `valueFromCmd`s. The
harness computes these each stage and injects them:

| Variable | Source |
|---|---|
| `$TICKET_ID` | machine |
| `$TASK`, `$PLAN` | machine |
| `$STATE`, `$PREV_STATE` | fold |
| `$CYCLE`, `$ATTEMPT` | fold |
| `$LEDGER_DIGEST` | rolling summary the harness assembles |
| `$ARTIFACT_<NAME>` | path to a captured artifact |
| `$ENTRY_ADDENDUM` | navigator's get-back-on-track note, when present |
| any `vars_set` value | e.g. `$BUILD_ID` from `build.id` |

## Toolbox changes

The toolbox is intentionally live. A run stages tools and MCP configuration
before it begins, while each stage resolves its playbook immediately before it
starts. An edit to `~/.loop/playbooks/implement.md` therefore affects later
stages of an in-flight run, but never a stage that has already started; tool and
MCP edits apply on the next run or resume. Keep the toolbox in version control
if you need to audit or coordinate such changes. `loop validate` warns if a
referenced playbook/tool is missing or if the machine references a tool a stage
doesn't allowlist.
