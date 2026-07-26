# 08 — Prior art: what exists, and what to steal

You're not the first to put a control layer around agents. Almost every piece of
this exists somewhere; the novel combination is **user-authored, per-ticket state
machines + a durable ledger + a portable toolbox, driven by a thin CLI over
headless coding agents.** Here's the landscape and the specific things worth
lifting.

## The direct inspiration

**karpathy/autoresearch** — a single objective, an experiment/eval loop that
iterates toward a metric. Great demonstration of "agent on a loop with a
feedback signal", but there's no user-authored graph, no gating structure, no
QA-against-staging notion, no reusable toolbox. You're generalizing its inner
loop into an arbitrary, gated, multi-stage graph. *Steal:* the discipline of a
crisp feedback signal per iteration — our transition `:check` commands are that signal
made explicit and machine-checkable.

## Closest conceptual match

**LangGraph** — the state-of-the-art for "agents as a graph": nodes, a shared
state object threaded through, conditional edges, cycles, checkpointing, and
human-in-the-loop interrupts. If you squint, `loop` is LangGraph with three
opinions bolted on: (1) it's a **CLI over subprocess agents**, not a Python
library embedding them; (2) the graph is **authored per ticket in YAML/Fennel**,
not built in code; (3) the checkpointer is a **greppable JSONL ledger**, not a DB.
*Steal:* the checkpointer/durable-state concept, conditional-edge functions
(your `when`/`branch`), and interrupt-for-human as a first-class terminal. *Read
their docs before building the loop reducer* — they've hit every edge case.

## Durable execution — the ledger's ancestry

**Temporal / Restate / DBOS / AWS Step Functions** — event-sourced workflows with
deterministic replay, activity retries with backoff, idempotency keys, and
timers. Your ledger *is* an event-sourced workflow log; your crash-resume *is*
replay. *Steal:* fold-to-current-state as the only state representation, activity
retry/backoff (your transient self-loop), idempotency keys on side-effecting
"activities" (your cycle-scoped namespaces), and the discipline that side effects
must be idempotent because replay can re-run them. The difference: their
activities are deterministic functions; yours are LLM agents, and your guards are
partly fuzzy — which is exactly why the Judge/`criteria` layer exists on top of
the deterministic core.

## Statecharts — the vocabulary

**XState / SCXML / Harel statecharts** — guards, entry/exit actions, hierarchical
and parallel states, the whole formal grammar of "when may this transition fire".
*Steal:* the vocabulary (guard, action, entry/exit), and — if you ever need it —
hierarchical states (a `qa` superstate containing `deploy`/`run`/`validate`) and
the parallel-region model for your fork/join future. Don't adopt SCXML's XML; do
adopt its precision about guard semantics.

## Declarative CI/CD DAGs — the reuse model

**GitHub Actions / Argo Workflows / Dagger / Nextflow** — declarative YAML DAGs of
reusable steps, with `uses:` references, templating, matrices, and artifacts
passed between steps. Your **playbook/tool references** are their `uses:`; your
**toolbox** is their marketplace/library; your **artifacts** are theirs.
*Steal:* the `uses: name@version` reference-and-pin model (directly informs
toolbox versioning), artifact-passing between steps, and matrix as the mental
model for fork/join. The gap they leave: they're **acyclic** and have **no agent**
— you need loops and a reasoning executor, which is the whole reason this project
exists rather than a GH Actions workflow.

## Academic: LLMs as explicit state machines

**StateFlow** (Wu et al., 2024) — formalizes LLM task-solving as a state machine
with states and transitions, and shows it beats free-form ReAct on control and
cost for multi-step tasks. **AutoGen**'s `GroupChat`/`StateFlow` patterns
implement it. *Steal:* the empirical argument for why the state-machine framing
wins — tighter control, lower cost, better reliability than open-ended autonomy —
and cite it when you wonder whether the structure is worth it. It is.

## Agentic SWE loops — the executor

**OpenHands (ex-OpenDevin) / SWE-agent / Devin / Aider's architect mode** — agents
that edit-run-observe against a codebase and a test suite. This is what happens
*inside* your Worker stages. *Steal:* their test-driven gating (a failing test is
an objective signal — feed it to a `when` guard), the edit-run-observe inner loop
as the Worker's natural rhythm, and their scaffolding for reading tracebacks.
*Differ:* they're monolithic — the control flow lives inside one agent's head.
You externalize it so it's inspectable, budgetable, and hackable per ticket. Your
`review` stage can literally *be* a run-review-style adversarial sub-loop; you're
composing these executors, not replacing them.

## Cautionary tales — open-loop autonomy

**AutoGPT / BabyAGI** — the "give it a goal and let it run" era. The lesson is
the anti-pattern: without structural gates and budgets, agents wander, loop
uselessly, and burn money with nothing to show. Your entire design — declared
graph, bounded loops, objective gates, hard budgets — is the correction. Keep
them in mind as the failure mode you're engineering against.

## Your own precedent

**pi `run-plan` / `run-review` skills** — a coordinator that plans, delegates to a
persistent implementer subagent, and runs a bounded adversarial review/fix loop
with an independent reviewer model. This is `loop` *in miniature, in one session*.
The generalization: lift the hard-coded plan→implement→review→fix loop into a
**user-authored machine**, lift the coordinator out of pi into a **CLI + ledger**,
and let any stage's Worker itself be a run-review coordinator. You already have
the sub-primitives (`select_review_model`, persistent subagents via
`@tintinweb/pi-subagents`, `steer_subagent`); `loop` is the framework that lets
you re-wire them per ticket instead of in skill code.

## Summary: the steal list

| From | Steal |
|---|---|
| autoresearch | crisp per-iteration feedback signal |
| LangGraph | checkpointer, conditional edges, human interrupts, edge-case-hardened reducer |
| Temporal/Restate/DBOS | fold-to-state, replay-resume, retry/backoff, idempotency keys |
| XState/statecharts | guard/action/entry-exit vocabulary, hierarchical + parallel states |
| GH Actions/Argo/Dagger | `uses:@version` reference+pin, artifact passing, matrix→fork/join |
| StateFlow/AutoGen | the empirical case for the state-machine framing |
| OpenHands/SWE-agent | test-driven gating, edit-run-observe inner loop |
| AutoGPT/BabyAGI | what *not* to do — the open-loop failure mode |
| pi run-plan/run-review | the coordinator+subagent+bounded-review pattern, generalized |

Nothing here is a reason not to build it — the combination is genuinely
unoccupied. It's a map of where the hard parts are already solved so you don't
re-solve them.
