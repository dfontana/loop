# loop

A local, **ticket-level agent orchestrator**. You co-design a small state machine
for one ticket — the task, the plan, the QA criteria, the stages and how they
connect — and a CLI harness drives one or more headless [pi](https://github.com/earendil-works/pi-mono)
agents around that machine until the ticket is done: implement → test → review →
fix → QA against staging → debug → re-test → validate → open PR.

The harness is deterministic and cheap. The agents are non-deterministic and
expensive. **The CLI owns the control flow and the ledger; the agent owns the
work inside a stage.** An agent finishes a stage by *proposing* where to go next;
the CLI *disposes* — it checks the proposal against the declared machine, and if
the proposal is invalid or the agent is stuck, a cheap "navigator" agent
reconciles and picks a valid next stage. Every step is appended to a JSONL
**ledger** so the run is auditable, resumable, and greppable.

The whole thing is meant to be **hacked together fast per ticket, then thrown
away**. The reusable parts — stage playbooks, pre-canned tools, machine
templates — live in a portable **toolbox** *outside* the project, so a new ticket
is a five-minute assembly job, not a build.

## The one-sentence framing

> `loop` is your `run-plan`/`run-review` skills generalized: the graph is
> **user-authored per ticket** instead of hard-coded, and the driver is an
> **external CLI + durable ledger** instead of one coordinating pi session — so
> control flow is inspectable, budgetable, and resumable across crashes.

## Documents

| Doc | What's in it |
|---|---|
| [docs/01-architecture.md](docs/01-architecture.md) | Components, the control loop, inversion of control, data flow |
| [docs/02-language.md](docs/02-language.md) | **YAML vs Fennel** for the machine definition — the central authoring decision, with samples |
| [docs/03-ledger.md](docs/03-ledger.md) | JSONL event schema, folding to current state, crash-resume, artifacts |
| [docs/04-toolbox.md](docs/04-toolbox.md) | Playbooks (a stage's prompt) vs skills (situational know-how + scripts), templating, per-stage binding, versioning |
| [docs/05-orchestration.md](docs/05-orchestration.md) | Spawning `pi` headless, the transition/judge/navigator agents, session models |
| [docs/06-example-walkthrough.md](docs/06-example-walkthrough.md) | A full Spark-pipeline ticket end to end: machine, spawns, and the ledger trace it produces |
| [docs/07-risks.md](docs/07-risks.md) | Failure modes and the mitigations the design must carry |
| [docs/08-prior-art.md](docs/08-prior-art.md) | LangGraph, Temporal/Restate, XState, Argo/GH Actions, StateFlow, OpenHands, autoresearch — what to steal |
| [docs/09-implementation-plan.md](docs/09-implementation-plan.md) | **The v1 build**: crate layout, task waves, and the four decisions that supersede 01–08 |
| [examples/](examples/) | Runnable-shaped config, split into `local/` (per-ticket) and `toolbox/` (the reusable globals) — machine, playbooks, tool YAML, machine templates, the ledger, and loop's own harness extensions |

Start with **01**, then **02** (the language decision is the one I most want your
read on), then **06** for the concrete feel. **09** records what the v1 build
settled and where it departs from 01–08.

## Status

v1 is under construction: a Rust workspace (`crates/`) driving `pi`, with the
machine authored in Fennel and evaluated in an embedded Lua VM. The toolbox
lives in `~/.config/loop/`; generated files go to `~/.local/state/loop/`.
See [docs/09](docs/09-implementation-plan.md).

## Relationship to `pi-extensions`

`loop` is built *on top of* the existing [`pi-extensions`](../pi-extensions)
monorepo, not a from-scratch stack. It reuses those packages rather than
reimplementing them:

- Skills are pi's own (`--skill`), so the toolbox is compatible with pi's skill
  format; MCP surfaces are the [`mcp`](../pi-extensions/extensions/mcp)
  extension; the `review` stage uses `select_review_model` from
  [`review-model-selector`](../pi-extensions/extensions/review-model-selector).
- The `implement`/`review` playbooks mirror the
  [`run-plan`](../pi-extensions/skills/run-plan) /
  [`run-review`](../pi-extensions/skills/run-review) skills.
- Only the three harness tools — `transition`, `verdict`, `choose` — are loop's
  own code, vendored under [`examples/toolbox/ext/`](examples/toolbox/ext).

## Glossary

- **Machine** — the per-ticket definition: states, transitions, per-state model/thinking/tools, and QA cases. One Fennel file (`machine.fnl`) that **references** the task and plan prose (`task.md`, `plan.md`) and each stage's playbook by name/path.
- **State / Stage** — a node in the machine, bound to a **playbook** that supplies its prompt.
- **Playbook** — a stage's prompt (a markdown file) plus default model/thinking (essentially a pi *skill*). Resolved **local-first** (`./.loop/playbooks/`, bespoke per ticket) then **toolbox** (`~/.config/loop/playbooks/`, reusable): "how to implement", "how to review", "how to debug Spark errors".
- **Skill** — situational know-how bound into a stage: a `SKILL.md` plus the scripts beside it, loaded via `pi --skill`. From the toolbox, resolved local-first.
- **Check** — a command the *harness* runs itself after a stage exits, whose exit code gates a transition. The one signal a worker cannot author, because it never touches the worker's session.
- **Toolbox** — the portable library of playbooks + tools + machine templates, stored outside any project (`~/.config/loop/`).
- **Ledger** — append-only JSONL, the event-sourced record of one run. The source of truth for "where are we".
- **Cycle** — one traversal of a loop (e.g. `qa#3`), with a unique id injected into prompts and tools.
- **Worker** — the pi agent spawned to execute a stage.
- **Judge** — a cheap, independent agent that evaluates a transition's semantic `criteria` (so the worker never grades its own homework).
- **Navigator** — a cheap agent that picks a valid next stage and writes an entry prompt when the worker's proposal is invalid or blocked.
- **Harness / `loop`** — the CLI that owns the loop, the ledger, and the guardrails.
