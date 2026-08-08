---
name: loop-authoring
description: Set up, edit, and drive a `loop` ticket machine — `.loop/machine.fnl`, its stage prompts, and its skills — on the user's behalf. Use when the user wants to turn a ticket into an agent workflow, mentions machine.fnl / stage prompts / `.loop/`, runs `loop init|validate|preview|run|resume|recap`, or asks why a loop run stalled, thrashed, or escalated.
---

# Authoring a loop

`loop` drives **one ticket** to completion. The ticket is declared as a small state machine: a handful of agent stages and the gated edges between them. `loop` spawns a headless `pi` agent per stage and refuses to move until the edge's gate passes.

You own the setup. The user describes the ticket in prose; you produce a validated `.loop/` they can run. Read what you need from the repo rather than interrogating them.

## The model, in five lines

- **States are stages.** Entering one renders a prompt and spawns a Worker agent in the project directory.
- **The agent proposes, the harness disposes.** A Worker ends its stage by writing JSON to `$LOOP_HANDOFF` naming where to go next. That is a request.
- **Edges are gated.** `:check` is a shell command the _harness_ runs after the stage exits (exit 0 passes). `:criteria` is prose an independent Judge rules on. Only then does the move commit.
- **One directory.** Everything for the ticket lives in `<project>/.loop/`. Nothing resolves from outside it.
- **One durable file.** `.loop/ledger.jsonl`, append-only. All run state is folded from it, so runs are resumable, auditable, and greppable.

## The job, end to end

Run this sequence. Do not skip step 6 — it is free and catches most of what would otherwise burn tokens.

1. **`loop doctor`** — confirms `pi` is on `PATH` and reports whether a machine already exists. If `.loop/machine.fnl` exists, you are _editing_, not scaffolding: skip to step 4.
2. **Gather the ticket.** Ask the user for what only they know; read the rest out of the repo. Checklist: `references/recipes.md`.
3. **Scaffold.** `loop init <TICKET>` for the bundled four-stage spine, or `loop init <TICKET> --from <DIR>` to copy a `.loop/`-shaped directory the user already likes. Never overwrites an existing file.
4. **Write `.loop/task.md` and `.loop/plan.md`.** `task.md` is for an agent that has the repo but not the user's head. `plan.md` is a numbered checklist — the default `implement → review` gate is "every item in the plan is addressed", and a Judge can only check that against a list.
5. **Shape `.loop/machine.fnl`.** Keys: `references/machine.md`. Then the stage prompts and skills those states name: `references/stage-prompts.md`.
6. **Verify, in this order:**
   - `loop validate` — reachability, dangling references, unguarded edges. Errors exit 1.
   - `loop preview` — the _resolved_ view: which file each stage prompt landed on, the merged model, the effective skill and MCP lists, every edge's real guard. Every number is computed by the code the run itself uses.
   - `loop preview <state>` — adds the rendered prompt, so you can see whether `$TASK` and `$PLAN` are actually wired in.
   - `loop diagram` — mermaid on stdout, to confirm the graph is the one you meant.
7. **Run it** (`loop run`) only when the user asks — it spends real money. Then `loop recap` for the write-up, `loop status` for a one-screen answer, `loop sessions` / `loop session <ID>` for a Worker's actual transcript.

## Rules that prevent most failures

- **No variable reaches a stage unless its stage prompt interpolates it.** There is no automatically prepended context header. A stage prompt that never writes `$TASK` gives the agent no task. This is the single most common authoring bug; `loop preview <state>` reports which variables the body actually writes.
- **Push every mechanical fact into a `:check`.** It is the one signal a Worker cannot author, because it runs in the harness's own subprocess after the stage exits. Reserve `:criteria` for the genuinely fuzzy remainder. A failed check is not appealable — the Judge is never even spawned.
- **A stage prompt is _told_; a skill is _offered_.** Anything the stage must know is a stage prompt. A skill the model chooses not to open did nothing. Full contrast: `references/stage-prompts.md`.
- **Stages must be idempotent.** An interrupted stage re-runs from the top as a new attempt — nothing checkpoints inside a stage. An `open-pr` stage checks for an existing PR; a deploy is keyed on something stable. `$CRASHED` is `1` on a re-entry after a death.
- **`:budgets` may only tighten** loop's built-in floor (`15.0` USD / `7200` s / `60` transitions). Writing a larger number changes nothing.
- **`states[0]` of a `:loops` entry is the loop head** — the state whose _re-entry_ counts a cycle. Getting the order wrong silently counts the wrong thing.
- **Write `:description` on every state.** It is what the Navigator reads when it has to route a stuck run, and the label `loop diagram` draws.
- **Never invent a key.** Every struct is `deny_unknown_fields`; a typo is a load error listing the keys that exist. If unsure, check `references/machine.md` rather than guessing.

## Reference index

Read the one you need; they are self-contained.

| File | When |
| --- | --- |
| `references/machine.md` | Every `machine.fnl` key — top level, states, transitions, loops, guards, budgets, escalation — plus a complete annotated machine and the removed keys that error by name. |
| `references/stage-prompts.md` | Stage prompts vs. skills, name resolution, frontmatter, the complete template-variable list, the four-layer model resolution, MCP servers. |
| `references/runtime.md` | What actually happens during a run: the ordered stage sequence, the three agent roles, the handoff protocol, the ledger event schema, artifacts, resume semantics. Read when a run misbehaved. |
| `references/cli.md` | Every command, flag, environment variable, and exit code. |
| `references/recipes.md` | The interview checklist, machine shapes worth copying, and a triage table for a run that stalled, thrashed, or escalated. |
