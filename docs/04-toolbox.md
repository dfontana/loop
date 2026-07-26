# 04 — The toolbox

> **Superseded in part by [09-implementation-plan.md](09-implementation-plan.md).**
> The toolbox lives at **`~/.config/loop/`**, not `~/.loop/`, and everything
> loop *generates* (rendered prompts) goes to **`~/.local/state/loop/`**, so
> nothing generated lands in the directory you hand-edit. `loop.config.yaml` is
> now `config.fnl`.
>
> **There is no toolbox `mcp.json`, and `PI_AGENT_DIR` is not set.** A state
> names servers with `:mcp`; they resolve against *your* own
> `~/.pi/agent/mcp.json`, which loop never reads. See "MCP" below.
>
> The second kind of reusable thing is now a **skill**, not a scoped-tool: a
> `SKILL.md` plus the scripts beside it, loaded by `pi --skill`. Per-stage tool
> allowlists are gone, and gating moved from `LOOP_VARS`/`when` to a `:check`
> the harness runs itself — [09](09-implementation-plan.md#why-when-guards-and-scoped-tools-were-cut)
> records why. Everything else here holds.

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
  skills/                       # SKILLS — situational know-how + the scripts that carry it out
    spark-build/
      SKILL.md                  #   what it is for and when to reach for it
      build.sh                  #   the script; also usable as a transition :check
    spark-run/
      SKILL.md
      run.sh  classify.sh
    staging-deploy/
    contract-check/
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
  skills/                       # LOCAL skills, same local-first override
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
2. Run the build after substantive changes; do not finish on a red build.
3. When the checklist is complete and the build is green, call
   `transition(to="review", rationale=…, artifacts=[…])`.
4. If you cannot make progress, call `transition(blocked=true, rationale=…)`
   and the harness will route you.
```

### Every stage has a prompt, and the prompt is its playbook

There is no separate "stage prompt" concept — **a stage's prompt *is* the markdown
file named by its `playbook:`.** A stage without a resolvable playbook is an error
`loop validate` catches. Resolution is **local overrides toolbox:**

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
- **Offered as situational know-how** — guidance the worker reaches for when it
  hits the situation, rather than the stage itself. That is now a **skill**, not
  a second usage mode for a playbook: `debug-transient` lives in `skills/` and
  the `debug` stage lists it in `:skills`.

Templating is `$UPPER_SNAKE` placeholders filled from the **context namespace**
(below). Unknown `$NAMES` pass through untouched, so `$HOME` etc. still work.

## These are existing pi-extensions, not new loop code

Some of what a stage gets is already-installed packages in
[`~/opencode/pi-extensions`](../../pi-extensions) — loop *configures* them, it
doesn't reimplement them. Keeping this straight avoids the trap of vendoring a
second copy:

| Toolbox piece | Backed by | How loop points it at the toolbox |
|---|---|---|
| `skills/<name>/` | **pi's own skill loader** | `--no-skills` plus one `--skill <path>` per skill the state named |
| a state's `:mcp` names | [`mcp`](../../pi-extensions/extensions/mcp) extension | nothing staged: the entry message asks the stage to `mcp({connect: …})` each name |
| `select_review_model` (in the `review` stage) | [`review-model-selector`](../../pi-extensions/extensions/review-model-selector) extension | activated per spawn; nothing to configure |
| `ext/*.ts` (transition/verdict/choose) | **loop's own**, vendored here | `-e`-injected per spawn (Worker / Judge / Navigator) |

The `implement`/`review` *playbooks* likewise mirror the
[`run-plan`](../../pi-extensions/skills/run-plan) /
[`run-review`](../../pi-extensions/skills/run-review) skills — same
`select_review_model` + four-angle adversarial fan-out, run as a loop stage
instead of interactively.

## Skills — the "what with"

A skill is situational know-how bound into a stage: a `SKILL.md` saying what it
is for and when to reach for it, plus the scripts that carry it out. pi loads it
by path; loop only resolves the name.

```
~/.config/loop/skills/staging-deploy/
  SKILL.md      # "deploy the branch to this ticket's namespace; the script
                #  refuses anything but dev/staging and reads its own token"
  deploy.sh     # validates its arguments, keys the namespace on $TICKET_ID/$CYCLE
```

Resolution is **local-first**, exactly like playbooks: `./.loop/skills/<name>/`,
then `~/.config/loop/skills/<name>/` (a bare `<name>.md` works too, for a skill
that is pure prose). A name containing `/` is an exact path.

The harness exports `$TICKET_ID` and `$CYCLE` into the spawn, so a script can
key a mutation on the cycle — the injectable cycle identity doubles as an
idempotency key, which is what makes a crash-resumed stage safe to re-enter.

### What a skill is and isn't a boundary for

A skill's script is **not** a security boundary. Any stage with `bash` could
call the same CLI directly, and the agent can read the script. What the script
buys is that the *intended* path is validated in one reviewable, testable place
— and, crucially, that the harness can run the identical code as a transition
`:check`. When the same `build.sh` appears in a state's `:skills` and in the
outgoing edge's `:check`, the agent's reading and the gate's are the same
reading, and there is no version of "it passed for me" the harness disagrees
with.

That symmetry is the point of the split:

- **A skill** bounds what a stage *knows* — instructions, not capability.
- **A check** bounds what a stage can *transition past* — capability, and the
  only tier a worker cannot author, because the harness runs it out of process
  after the stage exits.

### MCP — the "what else with"

MCP servers reach a stage through the [`mcp`](../../pi-extensions/extensions/mcp)
extension's single proxy tool. loop deliberately does **not** model them: it
ships no `mcp.json`, stages none, and never sets `PI_AGENT_DIR`, because doing
so would replace the servers you actually configured with an empty set.

What a state declares is which of *your* servers this stage should reach:

```fennel
:qa-staging {:playbook "qa" :mcp ["warehouse"]}
```

That extension starts every session with every server **off**, and the panel
that turns one on (`/mcp`) does not exist headless. The only way in is the
agent calling `mcp({connect: "warehouse"})`, so the names ride into the stage's
entry message as exactly that instruction, ahead of the work.

Two consequences worth naming:

- The names are **unverifiable at load time** — they belong to a file loop
  doesn't read, so a typo surfaces as a failed connect, not a `loop validate`
  error. What validate *can* catch is `:mcp` on a machine whose
  `:pi-extensions` omits `mcp`, where the tool wouldn't exist at all.
- This bounds *reach*, not trust. An unnamed server is one the stage never
  connects, but a named one is fully available to it; as with skills, the tier
  that decides what a stage may transition past is the edge's `:check`.

## Per-stage binding

A state names the skills it gets; the harness resolves each to a path and passes
it as `pi --skill`, after `--no-skills` turns off ambient discovery. So a stage
loads exactly what its machine declared.

```fennel
:states
{:qa-staging {:playbook "qa"
              :skills ["staging-deploy" "spark-run"]}
 :debug      {:playbook "debug-spark"
              :skills ["spark-build" "debug-transient"]}}
```

There is **no exclude list and no tool allowlist.** An earlier design gave each
state a `--tools` allowlist so a QA stage could be denied `edit`/`write`; that
was cosmetic, because the machine-wide default handed every stage `bash`, and a
stage with bash can `sed -i` whatever it likes. What actually keeps a validation
stage from fixing what it is grading is the pair of tiers above: an edge gated
on a command the harness runs, and a Judge that never sees the stage's own claim
of success.

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

The same namespace is substituted into a transition's `:check` command, and
`$TICKET_ID`/`$STATE`/`$CYCLE`/`$ATTEMPT` are exported into both the spawn's and
the check's environment — so a skill script and the check that re-runs it see
the same cycle.

## Toolbox changes

The toolbox is intentionally live: each stage resolves its playbook and skills
immediately before it starts. An edit to
`~/.config/loop/playbooks/implement.md` therefore affects later stages of an
in-flight run, but never a stage that has already started. Your `mcp.json` is
read by the `mcp` extension at connect time, so an edit there lands on the next
stage that connects.

Skill *scripts* are read at the moment they run, by the agent and by any check
that invokes them — so editing one mid-run changes the gate as well as the
instructions. That is usually what you want while hacking a ticket together, and
exactly what you don't want mid-audit. Keep the toolbox in version control.

`loop validate` errors if a referenced playbook or skill doesn't resolve, or if
two transitions share the same `from`/`to` pair (only the first would ever be
taken). It warns on an edge with neither a `:check` nor a `:criteria` — the
worker's proposal would be committed unexamined.
