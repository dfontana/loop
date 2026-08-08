# loop

A local, **ticket-level agent orchestrator**. You write a small state machine for one ticket — the task, the plan, the QA cases, the stages and how they connect — and `loop` drives headless [pi](https://github.com/earendil-works/pi-mono) agents around that machine until the ticket is done.

The harness is deterministic and cheap. The agents are non-deterministic and expensive. **The harness owns control flow and the ledger; the agent owns the work inside a stage.** An agent ends its stage by _proposing_ where to go next. The harness _disposes_: it runs the edge's check command, asks an independent Judge whether the criteria are met, and only then commits the move. Every step is appended to a JSONL ledger, so a run is auditable, resumable, and greppable.

The per-ticket machine is meant to be hacked together fast and thrown away. Everything a ticket needs lives in one directory, `.loop/` — the machine, its prose, its stage prompts, its skills, its ledger — so a ticket is self-contained, and starting a new one is `loop init --from` a directory you keep.

## Quickstart

```sh
cargo build --release -p loop         # binary lands at target/release/loop
loop doctor                           # pi on PATH? machine present?

loop init PROJ-1487                   # scaffolds ./.loop/
                                      #   ...or --from a .loop/ you already like
$EDITOR .loop/task.md .loop/plan.md   # what to do, and how
$EDITOR .loop/machine.fnl             # the stages and the edges between them

loop preview                          # what each stage resolves to, before spawning anything
loop validate                         # lint the graph and every reference
loop diagram                          # see what you actually declared
loop run                              # drive it to a terminal
```

While it runs, or after it stops:

```sh
loop status          # where it is and how it got there
loop recap           # every attempt, its evidence, and why the run ended
loop logs            # recent ledger events
loop sessions        # list every worker attempt and the session id that reopens it
loop session <ID>    # reopen one and read what that worker actually did
loop resume          # continue an interrupted run
```

`recap` is the post-run counterpart to `preview`: `preview` explains the machine you declared, `recap` explains the run that happened. It reads the ledger and nothing else — no LLM, no second history — so the same ledger always renders the same report, and it labels each piece of evidence by who authored it: the Worker's own summary, the harness's check output, the Judge's verdict. It writes Markdown to stdout, so `loop recap > run-recap.md` is a ticket write-up.

`status` folds the ledger into a summary; `session` is how you read the full transcript behind any line of it. loop keeps no transcript of its own — it records the session id pi filed each stage under, and hands it back:

```sh
loop sessions                     # every worker attempt, oldest first, one line each
loop sessions implement           # only attempts at that stage
loop session PROJ-1-implement-1-2 # reopen that attempt's transcript
loop session --latest implement   # newest one, no id needed (scripts, CI)
```

The listing is columns, not a menu — field 6 is the session id in every row — so choosing is the shell's job: `loop sessions | fzf | awk '{print $6}' | xargs loop session`.

Full walkthrough, and every key you can put in a machine: the [`loop-authoring` skill](skills/loop-authoring/SKILL.md).

## Documentation

Everything about **using** loop — authoring a machine, writing stage prompts and skills, the runtime semantics, and the CLI — lives in the [`loop-authoring` skill](skills/loop-authoring/). It ships alongside the binary and is written to be read by a coding agent as much as by you: point one at it and describe your ticket in prose, and it can scaffold, validate, and explain the `.loop/` it built.

| Where | What's in it |
| --- | --- |
| [skills/loop-authoring/SKILL.md](skills/loop-authoring/SKILL.md) | The workflow, end to end, and the rules that prevent most failures |
| [· references/machine.md](skills/loop-authoring/references/machine.md) | Every `machine.fnl` key — states, transitions, guards, loops, budgets — with a complete annotated machine |
| [· references/stage-prompts.md](skills/loop-authoring/references/stage-prompts.md) | Stage prompts vs. skills, name resolution, frontmatter, template variables, model resolution, MCP |
| [· references/runtime.md](skills/loop-authoring/references/runtime.md) | The run loop, the three roles, the handoff protocol, the ledger schema, artifacts, resume |
| [· references/cli.md](skills/loop-authoring/references/cli.md) | Every command, flag, environment variable, and exit code |
| [· references/recipes.md](skills/loop-authoring/references/recipes.md) | Interview checklist, machine shapes worth copying, and a triage table for a run that went wrong |
| [docs/design-notes.md](docs/design-notes.md) | Why it works this way, the tradeoffs, the reversed decisions, and the known gaps |
| [examples/](examples/) | A complete worked ticket — a real `.loop/`, and the ledger the run produced |

If you are evaluating the design rather than using it, read the **design notes** first, then `references/runtime.md`.

## How it fits with pi

`loop` sits on top of pi rather than replacing any of it:

- **Skills** are pi's own. loop resolves a name to a path and passes `--skill <path>`; it never parses the format.
- **MCP servers** are named, not shipped. A state's `:mcp` list names servers in _your_ `~/.pi/agent/mcp.json`, which loop never reads or writes.
- **Nothing is injected.** A Worker ends its stage by writing JSON to the file named in `$LOOP_HANDOFF`; the Judge and Navigator have no tools at all and answer against a fixed first-line contract. loop used to vendor three TypeScript tools for this and scrape their output back off pi's event stream — the decision still arrives as structured data, it just no longer costs a dependency on pi's extension ABI.

Which means the only pi-specific code left is a handful of argv builders, all of them in one module (`runner/command.rs`): one per role, plus one to reopen a session. Driving a different headless agent is a new set of `*_command` builders, not a port.

## Glossary

- **Machine** — the per-ticket definition: states, transitions, loops, budgets, and QA cases. One Fennel file (`machine.fnl`) that _references_ the task and plan prose, and names each stage's prompt and skills.
- **State / stage** — a node in the machine, bound to exactly one stage prompt.
- **Stage prompt** — what a stage is _told_: a markdown file in `./.loop/stage-prompts/`, rendered with the run's `$VAR`s and handed to `pi --append-system-prompt`. Always in that stage's context.
- **Skill** — what a stage is _offered_: a `SKILL.md` plus the scripts beside it, or a bare `.md`, passed to `pi --skill`. loop never opens one; the model sees its name and description and decides whether to load it.
- **Check** — a command the _harness_ runs itself after a stage exits; exit 0 passes the edge. The one signal a worker cannot author, because it never touches the worker's session.
- **Criteria** — a prose standard an independent Judge evaluates against the stage's output and artifacts.
- **Ledger** — append-only JSONL at `.loop/ledger.jsonl`. The source of truth for where a run is; all state is folded from it, never stored.
- **Cycle** — one traversal of a declared loop, counted on re-entry into the loop's head state.
- **Worker** — the pi agent spawned to execute a stage.
- **Judge** — a cheap, isolated agent that rules on a transition's criteria, so a worker never grades its own homework.
- **Navigator** — a cheap agent that picks a valid next state when a worker is blocked or proposes an edge that doesn't exist.

## Status

Working, and under active development. The `loop` crate in `crates/` drives `pi` (`crates/mock-pi` is its offline stand-in, for the tests); machines are authored in Fennel and evaluated in an embedded Lua VM. Everything a run reads or writes is under `<project>/.loop/`, including the rendered prompts, which go in `.loop/run/` and are gitignored.

The limits that come with the design — stage-level recovery, budgets sampled between stages, skills that scope instructions rather than capability — are written down in [design notes](docs/design-notes.md#limits-we-accept) rather than left for you to discover.
