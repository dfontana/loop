# Getting started

`loop` drives one ticket to completion. You describe the ticket as a small state machine — a handful of stages and the edges between them — and `loop` runs an agent in each stage until the machine reaches a terminal.

By the end of this page you will have a machine scaffolded, validated, and running against a real ticket. Budget about twenty minutes.

## Prerequisites

**Rust and Cargo**, to build the binary. The workspace targets Rust 1.85 or newer (edition 2024).

**`pi` on your `PATH`.** `loop` does not talk to a model itself — every stage spawns `pi` as a subprocess, and `pi` is the agent that reads the repo, edits files, and runs commands. Without it you can scaffold, validate, and draw a machine, but you cannot run one.

If your `pi` lives somewhere unusual, `LOOP_PI_BIN` points at it. That and every other environment variable are covered in [the CLI reference](04-cli-reference.md).

Nothing else. There is no daemon, no server, no database — the whole run lives in files under your project.

## Install

Build the CLI from the workspace root. The crate is `loop-cli`; the binary it produces is called `loop`.

```
cargo build --release -p loop-cli
```

That leaves the binary at `target/release/loop`. Put it on your `PATH` however you normally do — a symlink into `~/.local/bin` is enough.

Then check the environment:

```
loop doctor
```

`doctor` runs exactly three checks and says nothing about your machine's contents:

```
  ok    `pi` on PATH
  ok    ~/.config/loop/config.fnl
  ok    .loop/machine.fnl

all good
```

On a fresh install you will see failures for everything after the first check, because you have not scaffolded anything yet:

```
  ok    `pi` on PATH
  FAIL  ~/.config/loop/config.fnl — run `loop init <TICKET>` to scaffold the toolbox
  FAIL  .loop/machine.fnl — run `loop init <TICKET>` in this project
```

and `doctor` exits non-zero with `error: 2 problem(s)`. That is expected. Fix the first line if it failed — install `pi`, or set `LOOP_PI_BIN` — and let `loop init` fix the rest.

## Scaffold a ticket

Change into the repository you want the ticket worked on, and name the ticket:

```
loop init PROJ-1487
```

`init` writes two trees.

The first is your **toolbox** at `~/.config/loop/`. It is global, written once, and shared by every ticket on the machine. `init` only creates files that are absent, so it is safe to re-run and it will never clobber your edits:

```
  created /home/you/.config/loop/config.fnl
  created /home/you/.config/loop/machines/standard-ticket.fnl
  created /home/you/.config/loop/playbooks/implement.md
  created /home/you/.config/loop/playbooks/review.md
  created /home/you/.config/loop/playbooks/qa.md
  created /home/you/.config/loop/playbooks/open-pr.md
  created /home/you/.config/loop/playbooks/debug-transient.md
```

It also creates an empty `~/.config/loop/skills/`. Everything in this tree is yours to edit — `loop` writes a file only when it is absent, and never rewrites one you have changed.

The second tree is **this ticket**, at `./.loop/`: `machine.fnl` copied from the `standard-ticket` template with `$TICKET` replaced by `PROJ-1487`, a `task.md` and `plan.md` stub, and an empty `playbooks/` directory for prompts that are specific to this ticket.

The closing output is your checklist for the rest of this page:

```
initialized /home/you/src/yourrepo/.loop for PROJ-1487
  1. write .loop/task.md and .loop/plan.md
  2. hack .loop/machine.fnl into the shape this ticket needs
  3. loop validate
  4. loop run
```

Pick a different starting shape with `--template <name>`, which names a file in `~/.config/loop/machines/`. Out of the box there is only `standard-ticket`.

## Write the task and plan

`.loop/task.md` and `.loop/plan.md` are plain prose. Nothing parses them — `loop` reads each file whole and makes its text available to stage prompts as `$TASK` and `$PLAN`.

Write `task.md` for an agent that has your repository but not your head: what to change, where, and what "done" means. Keep it short.

Write `plan.md` as a numbered checklist. This matters more than it looks: the default machine gates the first edge on "every item in the plan is addressed", and a Judge can only evaluate that against an actual list.

`$TASK` and `$PLAN` are two of about a dozen variables a stage prompt can interpolate. The full list, and the rule that a variable only reaches the agent where a prompt actually writes it, are in [customizing](03-customizing.md).

## Shape the machine

Open `.loop/machine.fnl`. It is Fennel — a Lisp that evaluates to a plain table — but for now read it as a declaration, not a program. There is no code that runs during your run.

The `standard-ticket` template gives you a four-stage spine:

**implement → review → test → open-pr → done**

with two back-edges. If `review` finds blocking defects the run routes back to `implement`; if `test` fails, likewise. That is what makes it a loop rather than a pipeline — a stage that is not satisfied sends work backwards instead of forwards, and the `fix` loop caps how many times that can happen before the run gives up and lands on the `blocked` terminal.

Each edge carries a gate. Some gates are a `:check` — a shell command the harness runs itself, in its own subprocess, after the stage exits. Others are `:criteria` — prose evaluated by an independent, cheap Judge model that sees the stage's output but not the stage's session. The template ships with criteria on every edge and a commented-out check on the way out of `test`; swapping in the command that actually runs your suite is the single highest-value edit you can make to it.

The fastest way to see what you actually have is to draw it:

```
loop diagram
```

It prints mermaid to stdout — no fences, no prose — so `loop diagram > machine.mmd` gives you a file you can paste into anything that renders mermaid:

```
---
title: "PROJ-1487"
---
stateDiagram-v2
    %% generated by `loop diagram` from machine.fnl

    state "blocked (escalation)" as blocked
    state "open-pr" as open_pr

    [*] --> implement
    implement --> review : judge
    review --> test : judge
    test --> open_pr : judge
    open_pr --> done : judge

    %% back-edges taken when a guard fails (`:on-fail {:route ...}`)
    review --> implement : guard fails
    test --> implement : guard fails

    blocked --> [*]
    done --> [*]

    note right of implement
        loop "fix": max 4 cycles, then escalate to blocked
    end note
```

Every edge label tells you how that edge is gated: `check`, `judge`, both, or `unguarded`. Redraw after each edit — `diagram` never touches the toolbox, so a machine with a playbook that does not resolve yet still draws.

The diagram shows the shape. To see what the stages inside it actually resolve to — which playbook file wins, which model each one ends up on after the four config layers are merged, what the edge guards really are — preview it:

```
loop preview
```

Every number in that report is computed by the code the run itself uses, so it is what you will get rather than what the file appears to say. It is where a `:thinking "high"` you forgot to delete, a playbook resolving to the toolbox copy instead of your local one, or a `:check` still commented out becomes visible.

Then look at one stage in full:

```
loop preview implement
```

That adds the resolved playbook and its frontmatter, the exact `--skill` paths and MCP names the Worker gets, the variables the playbook body actually interpolates — and a **representative render**: the prompt with the template variables filled in.

Read that render for shape, not for text. It is built with cycle 1, attempt 1, no previous state, no artifacts, and an empty ledger digest, because those values do not exist until a run has been somewhere. `$PREV_STATE`, `$LEDGER_DIGEST`, `$CYCLE`, `$ATTEMPT`, `$CRASHED`, `$ENTRY_ADDENDUM` and the `$ARTIFACT_*` paths will all differ in a real run — the report says so next to the render. What it does tell you exactly is whether your playbook wired the variables in at all, which is the mistake that actually happens: a stage whose prompt never writes `$TASK` gets no task.

Both forms are read-only. Preview spawns nothing, runs no `:check`, connects to no MCP server, and writes no ledger, artifact, or rendered prompt file — you can run it as often as you edit.

The whole vocabulary — every key, how playbooks and skills resolve, how models are chosen per stage — is in [the machine reference](03-customizing.md#machinefnl--the-ticket-machine). Do not try to learn it before your first run. Change the ticket's task, plan, and test command; leave the rest.

## Validate before you run

```
loop validate
```

`validate` loads the machine and lints it: unreachable states, states with no path to a terminal, edges pointing at names that do not exist, playbooks and skills that do not resolve, loops whose head is never re-entered. A clean machine prints one line:

```
PROJ-1487 — 4 states, 4 transitions, no problems found
```

Problems print one per line, tagged and column-aligned:

```
error  review -> test: transition `to` `test` names no state or terminal
warn   open-pr: transition `open-pr` → `done` has neither `check` nor `criteria`: the worker's proposal is committed unexamined
```

Any `error` exits non-zero with `error: 1 error(s)`, and you should fix it before running — most of them mean the run would escalate the first time it reached the broken edge. **Warnings alone exit 0.** They suppress the "no problems found" line but do not stop anything; the warning above is telling you that on that edge the agent's own claim that it succeeded is the only thing being recorded.

Run `validate` after every edit to the machine. It is cheap and it costs no tokens.

## Run it

```
loop run
```

Each state in the machine is one agent stage. `loop` renders that stage's prompt, spawns `pi` with it, and waits. The agent does the work — reads files, edits, runs commands — and ends its turn by writing a small JSON handoff to the file named in `$LOOP_HANDOFF`, declaring where the run should go next, with a rationale and any artifacts it wants later stages to see. The harness reads that file once the process exits; nothing the agent says in prose moves the run.

The agent proposes; **the harness disposes.** `loop` takes that proposal and decides for itself whether to allow it: the edge has to exist in the machine, the edge's `:check` command has to exit zero, and the edge's `:criteria` has to satisfy a separate Judge model that never sees the working agent's session. Only then does the transition commit and the next stage start. If the agent says it is blocked, or names somewhere it cannot go, a third cheap model — the Navigator — picks a legal next state instead.

Every one of those steps is appended to a ledger on disk before the next one starts, which is what makes a run resumable and auditable after the fact. The full mechanics — the three roles, the guard tiers, what each event means — are in [how a run works](02-how-it-works.md).

The run ends with a summary line:

```
Done — done after 6 transitions, $3.41, 18m4s
```

That is status, terminal state, committed transitions, dollars, wall clock.

Runs are also bounded independently of the machine's logic: cost, wall-clock, and a maximum transition count, checked before each stage spawns. `--max-transitions N` tightens the transition budget for one invocation, and only ever tightens it — it cannot raise the machine's own ceiling.

## When it stops

There are exactly three outcomes.

**Done** — the run reached a terminal state normally. Exit 0.

**Failed** — the run reached the escalation terminal (`blocked` in the default machine), or an edge with `:on-fail "abort"` failed its guard. The machine ran correctly; the work did not converge. The process exits non-zero with:

```
error: run ended at `blocked` without completing — see `loop status`
```

**Aborted** — a guardrail stopped the run rather than the machine finishing it: a budget breach, or an escalation with nowhere to escalate to:

```
error: run aborted — see `loop status` for the guardrail
```

Whichever it is, `loop recap` is the first thing to reach for. It reads the ledger and writes the whole run out as Markdown: what it was started with, one section per stage attempt, and why it stopped.

```
loop recap                  # to the terminal
loop recap > run-recap.md   # or straight into the ticket
```

Four sections, always in this order:

1. **Run summary** — the ticket, the budgets the run started under, its outcome, totals, cycle counts, and how often the Navigator had to step in.
2. **Attempt timeline** — every `state_entered`, in order, with the model and skills it ran under, the Worker's summary and cost, its proposal, each guard tier's outcome, the check's captured output, the Judge's rationale, and the committed move. **Failed attempts are in here too**, including the ones that produced no commit at all.
3. **Why it ended** — the terminal transition, or the guardrail that stopped it. For a run still going or interrupted, the resume point and the last durable event instead.
4. **Inspecting further** — the `loop sessions` and `loop logs --raw` commands for this particular run.

Two things make it worth trusting. It is **deterministic** — no LLM writes any of it, so the same ledger always renders the same report. And it **labels evidence by author**: a `**Worker**` block is the Worker's own account of what it did and proves nothing on its own, while `**Check**` is output from a command the harness ran itself and `**Judge**` is an independent verdict. When those three disagree, the recap shows you the disagreement rather than a smoothed-over summary.

It also works on a run that is still going, or one that crashed — "recap to date" is a normal answer. The only thing it refuses is an empty ledger.

Then there are progressively deeper views. `loop status` is the quick one-screen answer while a run is in flight:

```
running — at `review`
  5 transitions, $1.23, 12m3s
  cycles: implement#2, qa#1
```

For the event-by-event view, use `logs`:

```
loop logs            # the last 20 events, oldest first
loop logs -n 50      # a larger human-readable tail
loop logs --raw      # the complete ledger as JSONL, for jq
```

`loop status --json` gives you a machine-readable folded summary for scripting.

`recap` tells you _that_ the review failed twice, and what the Judge said about it. When you want the reasoning behind it, reach for the transcript:

```
loop sessions
```

That lists every worker attempt in the run, oldest first — one line each, in columns: time, stage, cycle, attempt, outcome, the session id, and the worker's own summary.

```
2026-07-26T12:01  implement  1  1  crashed   PROJ-1-implement-1-1  error: executor lost
2026-07-26T12:03  implement  1  2  finished  PROJ-1-implement-1-2  Added the retry guard and updated the tests.
2026-07-26T12:05  review     1  1  finished  PROJ-1-review-1-1     Found a defect in the backfill window.
```

Hand one of those ids back to open it:

```
loop session PROJ-1-implement-1-2
```

loop then hands the terminal to pi, in the same session that stage ran in, with its full history: every message, tool call, and result. Nothing is copied or re-rendered — the transcript was always pi's, and `loop` only kept the id.

There is no picker to learn, because the listing is columns and your shell already has one — field 6 is always the session id:

```sh
loop sessions | fzf | awk '{print $6}' | xargs loop session
```

Two shortcuts, for when you already know what you want:

```
loop sessions implement           # only attempts at that stage
loop session --latest implement   # the newest one, no id needed
```

`--latest` is the scripted path: one input, one answer, and nothing to choose.

If the run was interrupted — you hit Ctrl-C, the machine slept, a spawn crashed — use:

```
loop resume
```

`resume` re-reads the ledger, works out where the run actually got to, and continues from there. An interrupted _stage_ re-runs from the beginning as a new attempt rather than picking up mid-thought, so stages should be safe to run twice. `loop run` refuses to start on top of an existing run and tells you to resume; `loop resume` refuses when there is nothing to continue.

Underneath all of them is `.loop/ledger.jsonl` — one JSON object per line, appended and fsynced, never rewritten. Everything above is a view over it: `recap` narrates it, `status` folds it, `logs` prints it, `session` looks up the ids inside it. To access the complete ledger without knowing its path, use `loop logs --raw`; it is meant to compose with `jq` and other tools. To start a genuinely fresh run, delete it.

## Where to go next

- [How a run works](02-how-it-works.md) — the three agent roles, the guard tiers, the ledger event schema, and how to read a run after the fact. Read this next if anything above felt like a black box.
- [Customizing](03-customizing.md) — every machine and config key, writing your own playbooks and skills, wiring MCP servers, and the full template-variable list. Read this when the default spine stops fitting the ticket.
- [CLI reference](04-cli-reference.md) — every command, flag, environment variable, and exit code, including `LOOP_PI_BIN` and the directory overrides.
- [Design notes](05-design-notes.md) — why the harness owns control flow instead of the agent, and the known gaps you should be aware of before trusting a run unattended.
