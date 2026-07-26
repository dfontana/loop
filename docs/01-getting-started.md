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

`doctor` runs exactly four checks and says nothing about your machine's contents:

```
  ok    `pi` on PATH
  ok    ~/.config/loop/config.fnl
  ok    vendored ext materialized
  ok    .loop/machine.fnl

all good
```

On a fresh install you will see failures for everything after the first check, because you have not scaffolded anything yet:

```
  ok    `pi` on PATH
  FAIL  ~/.config/loop/config.fnl — run `loop init <TICKET>` to scaffold the toolbox
  FAIL  vendored ext materialized — run `loop init` to write them
  FAIL  .loop/machine.fnl — run `loop init <TICKET>` in this project
```

and `doctor` exits non-zero with `error: 3 problem(s)`. That is expected. Fix the first line if it failed — install `pi`, or set `LOOP_PI_BIN` — and let `loop init` fix the rest.

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

It also creates an empty `~/.config/loop/skills/` and writes three vendored TypeScript tools into `~/.config/loop/ext/`. Those are the tools `loop` injects into the agents it spawns; leave them alone, since `loop` restores them whenever their contents drift from what the binary carries.

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

Each state in the machine is one agent stage. `loop` renders that stage's prompt, spawns `pi` with it, and waits. The agent does the work — reads files, edits, runs commands — and ends its turn by calling an injected `transition` tool that declares where the run should go next, with a rationale and any artifacts it wants later stages to see.

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

Whichever it is, `loop status` is the first thing to reach for. It folds the ledger and prints where the run is, what it has cost, and how many cycles each looping stage has burned:

```
running — at `review`
  5 transitions, $1.23, 12m3s
  cycles: implement#2, qa#1
```

For the event-by-event view, use `logs`:

```
loop logs            # the last 20 events, oldest first
loop logs -n 50      # a larger human-readable tail
```

`loop status --json` gives you a machine-readable folded summary for scripting.

If the run was interrupted — you hit Ctrl-C, the machine slept, a spawn crashed — use:

```
loop resume
```

`resume` re-reads the ledger, works out where the run actually got to, and continues from there. An interrupted _stage_ re-runs from the beginning as a new attempt rather than picking up mid-thought, so stages should be safe to run twice. `loop run` refuses to start on top of an existing run and tells you to resume; `loop resume` refuses when there is nothing to continue.

Underneath both is `.loop/ledger.jsonl` — one JSON object per line, appended and fsynced, never rewritten. To access the complete ledger without knowing its path, use `loop logs --raw`; it is meant to compose with `jq` and other tools. To start a genuinely fresh run, delete it.

## Where to go next

- [How a run works](02-how-it-works.md) — the three agent roles, the guard tiers, the ledger event schema, and how to read a run after the fact. Read this next if anything above felt like a black box.
- [Customizing](03-customizing.md) — every machine and config key, writing your own playbooks and skills, wiring MCP servers, and the full template-variable list. Read this when the default spine stops fitting the ticket.
- [CLI reference](04-cli-reference.md) — every command, flag, environment variable, and exit code, including `LOOP_PI_BIN` and the directory overrides.
- [Design notes](05-design-notes.md) — why the harness owns control flow instead of the agent, and the known gaps you should be aware of before trusting a run unattended.
