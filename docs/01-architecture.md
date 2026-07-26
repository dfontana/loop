# 01 — Architecture

## The core inversion

The single idea that makes this tractable:

> **Deterministic control flow wrapping non-deterministic work.**

The CLI (`loop`) is a plain, boring, testable program. It reads the machine,
folds the ledger to find the current state, spawns an agent for that state,
reads back a *structured* transition proposal, checks it, and moves on. It never
"decides" anything with an LLM in a way that isn't recorded and bounded.

The agent is where the intelligence and the entropy live: it edits code, runs
tools, reasons about failures. But it does not own the flow. It ends its turn by
calling one tool — `transition(to, rationale, artifacts)` — and stops.

This split is what buys you: budgets you can actually enforce, a run you can
resume after a crash, an audit trail, and the ability to swap any single stage's
model/prompt/tools without touching the rest.

## Components

```
                    ┌──────────────────────────────────────────────┐
                    │  loop  (the CLI harness — deterministic)       │
                    │                                                │
   machine.yaml ───▶│  ┌───────────┐   fold    ┌──────────────┐     │
   (the ticket)     │  │  Machine  │◀──────────│   Ledger      │     │
                    │  │  loader   │           │  (JSONL)      │◀─┐  │
   ~/.loop/  ──────▶│  └─────┬─────┘           └──────┬───────┘  │  │
   (toolbox:        │        │ current state          │ append   │  │
    playbooks,      │        ▼                        │          │  │
    tools,          │  ┌───────────────┐              │          │  │
    templates)      │  │  Stage runner │──────────────┘          │  │
                    │  │  - context pack                          │  │
                    │  │  - spawn pi headless ──────────┐         │  │
                    │  │  - parse JSONL events          │         │  │
                    │  │  - capture artifacts           │         │  │
                    │  └───────┬───────────────┬────────┘         │  │
                    │          │ proposal      │ if invalid       │  │
                    │          ▼               ▼                  │  │
                    │  ┌──────────────┐  ┌──────────────┐         │  │
                    │  │ Guard check  │  │  Navigator    │─────────┘  │
                    │  │ struct→when  │  │  agent (cheap)│            │
                    │  │ →Judge agent │  └──────────────┘            │
                    │  └──────────────┘                              │
                    └───────────────────┬────────────────────────────┘
                                        │ spawns (subprocess)
                                        ▼
                    ┌──────────────────────────────────────────────┐
                    │  pi  (headless worker — non-deterministic)     │
                    │  --print --mode json  --model X:thinking       │
                    │  --no-skills --skill <the stage's skills>      │
                    │  builtin read/edit/write/bash, the mcp proxy,  │
                    │  and the injected `transition` tool            │
                    └──────────────────────────────────────────────┘
```

- **Machine loader** — parses `machine.yaml` (or `.fnl`) and validates the graph.
  The run stages tools and MCP configuration before it begins; each stage resolves
  its playbook immediately before it runs, so a playbook edit applies to stages
  that have not started yet.
- **Ledger** — the append-only JSONL run record. Folding it yields the current
  state, the active cycle counters, and per-state attempt counts. See
  [03-ledger.md](03-ledger.md).
- **Stage runner** — the per-state engine: assembles a deterministic *context
  pack*, spawns a `pi` worker with the right model/thinking/tools/prompt, parses
  the JSONL event stream, captures artifacts, and reads the worker's
  `transition` call.
- **Guard check** — three tiers, cheapest first: **structural** (is this edge in
  the graph?), **`when`** (a deterministic expression over structured ledger
  vars, e.g. `build.status == 'pass'`), then **`criteria`** (an LLM *Judge*).
- **Judge** — separate cheap agent that evaluates the semantic `criteria`. Kept
  independent so the worker can't approve its own output — critical for QA.
- **Navigator** — cheap agent invoked only when the worker proposes an
  out-of-graph target or signals `blocked`. It picks a reachable next state and
  synthesizes an entry-prompt addendum to get back on track.

## The control loop

```text
load machine
fold ledger → (current_state, cycle_counters, attempts)   # fresh run: entry state
loop:
    if current_state is terminal: emit run_finished; exit

    check global guardrails (wallclock, $ budget, total transitions); abort if exceeded
    cycle    = cycle_counters.for(current_state)
    attempt  = attempts.for(current_state, cycle) + 1

    append state_entered{state, cycle, attempt, model, thinking, tools, session}
    context = build_context_pack(machine, ledger_digest, current_state, cycle)
    result  = spawn_worker(current_state, context)         # pi headless, blocking
    append worker_output{summary, artifacts}

    proposal = result.transition          # {to, rationale, artifacts} or {blocked, rationale}
    append transition_proposed{...}

    if proposal is blocked OR proposal.to not in edges(current_state):
        choice = spawn_navigator(machine, ledger_digest, proposal)   # enum-constrained to reachable
        append navigator_invoked{chosen_to, entry_prompt}
        target, entry_addendum = choice
    else:
        target, entry_addendum = proposal.to, none

    # structural guard already satisfied by construction of `target`
    if edge(current_state, target).when and not eval_when(ledger_vars):
        append guard_checked{when: fail}
        handle on_fail (retry | route | abort); continue
    if edge(current_state, target).criteria:
        verdict = spawn_judge(criteria, worker_output_digest, artifacts)  # cheap agent
        append guard_checked{criteria: verdict}
        if verdict == fail: handle on_fail; continue

    append transition_committed{from: current_state, to: target}
    stash entry_addendum for target's context pack
    current_state = target
```

Everything an LLM decides — the worker's proposal, the navigator's choice, the
judge's verdict — is a single constrained call, recorded, and bounded. The loop
body itself has no LLM in it.

## Data flow between stages

Stages do **not** share chat memory by default. Continuity flows through two
deterministic channels the CLI controls:

1. **The ledger digest** — a compact rolling summary the CLI assembles: the last
   N transitions, their rationales, and pinned artifact references. Not raw
   transcripts (cost + drift).
2. **Artifacts** — outputs a stage captures to `./.loop/artifacts/` (a build id,
   a test report, a Spark job output sample), referenced by path+hash from the
   ledger and injectable into later prompts/tools as `$ARTIFACT_<NAME>`.

A stage that genuinely needs conversational continuity across cycles (the classic
implement↔fix loop, where the fixer should remember what it just tried) opts into
a **persistent session** — see [05-orchestration.md](05-orchestration.md#session-models).

## Why external CLI and not an internal coordinator agent

Your `run-plan` skill already runs a coordinator *inside* pi that spawns
subagents. That's a legitimate alternative architecture. The trade-off:

| | External CLI (this design) | Internal coordinator agent (run-plan style) |
|---|---|---|
| Control flow | Deterministic code | An LLM deciding flow |
| Budget/wallclock caps | Hard, enforced by the harness | Soft, the agent has to choose to stop |
| Crash resume | Ledger replay, trivial | Lose the coordinator's context |
| Auditability | Every decision is a ledger line | Buried in one long transcript |
| Continuity | Must be engineered (digest/artifacts) | Free — it's one conversation |
| Build effort | More plumbing | Less — it's a skill |

The external CLI matches your stated vision and is the right call when a ticket
needs gating, budgets, and resumability. The internal coordinator is the lighter
prototype if you want to feel the shape first — and the two aren't exclusive: a
stage's worker *can itself* be a coordinator that fans out subagents (e.g. the
`review` stage invoking `run-review`).
