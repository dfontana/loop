# 05 — Orchestration: spawning pi, and the three agent roles

> **Superseded in part by [09-implementation-plan.md](09-implementation-plan.md).**
> The harness is **Rust**, not Node/TS, and the vendored `ext/*.ts` are written
> into `~/.config/loop/ext/` from the binary itself. **Session models are cut
> from v1**: every stage runs fresh, and continuity is the ledger digest plus
> artifacts. The three roles, the spawn flags, and the constrained `transition`
> schema are all as described here.

## How a stage is spawned

`loop` drives pi in **headless JSON mode**. Confirmed against the installed pi:
`pi -p --mode json …` emits a newline-delimited event stream whose first line is
a `session` record carrying the session id; subsequent lines carry messages, tool
calls, and the final result. The harness parses that stream line by line.

A stage spawn is assembled deterministically from the state's config:

```bash
pi --print --mode json \
   --session-id "${TICKET}-${STATE}-${CYCLE}" \       # deterministic id → resumable, forkable
   --provider  "${PROVIDER}" \
   --model     "${MODEL}:${THINKING}" \               # model + thinking in one token, e.g. claude-sonnet-5:high
   --no-skills --skill "${SKILL_PATH}" ... \          # exactly the skills this state named
   -e ~/.config/loop/ext/transition-tool.ts \         # loop's OWN vendored ext (see below)
   --append-system-prompt "${RENDERED_PLAYBOOK}" \    # playbook rendered with the context namespace
   "${RENDERED_ENTRY_MESSAGE}"                         # short "you are entering STATE, cycle N" kickoff
```

Two different extension mechanisms feed a spawn, and they're easy to conflate:

- **loop's own tools** — `transition-tool.ts` (Worker), `verdict-tool.ts`
  (Judge), `choose-tool.ts` (Navigator) — are vendored in `~/.loop/ext/` and
  `-e`-injected per spawn. They don't exist outside loop.
- **existing pi-extensions** — [`mcp`](../../pi-extensions/extensions/mcp) and
  [`review-model-selector`](../../pi-extensions/extensions/review-model-selector)
  — are *installed packages*, not files loop ships. The harness activates them
  and otherwise leaves them alone: `PI_AGENT_DIR` is deliberately unset so
  `mcp` keeps reading the user's own `~/.pi/agent/mcp.json`. A state's `:mcp`
  list names servers from that file, and rides into the entry message as
  `mcp({connect: …})` instructions — the extension starts every session with
  every server off, and headless there is no `/mcp` panel to turn one on.
- **skills** are pi's own mechanism, loaded by path: `--no-skills` turns off
  ambient discovery, then one `--skill <path>` per skill the state named. So a
  stage loads exactly what its machine declared and nothing a stray
  `~/.pi/skills/` happens to hold.

Notes:

- **`--model X:thinking`** is how pi takes model + thinking level together, so
  per-state model/thinking config is a one-line render.
- **`--no-skills` + `--skill <path>`** pins a stage's skill set to what the
  machine declared. Note this bounds *instructions*, not capability: a skill is
  a prompt plus a script the agent runs through bash, so withholding one hides
  know-how rather than revoking access. What a stage may *do* is bounded by its
  tools; what it may *transition past* is bounded by the edge's `:check`.
- **`-e`** loads loop's own vendored tools per spawn.
- **`--append-system-prompt` takes a bare path, not `@path`.** pi's
  `resolvePromptInput` does `existsSync` on the argument and reads it as a file
  when it resolves, else treats it as literal text — so an `@` prefix silently
  turns the whole playbook into the literal string `@/path/…`. (An earlier draft
  of this doc got that wrong; verified against the installed pi's
  `dist/core/resource-loader.js`.)
- The **rendered playbook** goes in as an appended system prompt; the **entry
  message** is the short positional kickoff. Keeping the "how" in the system
  prompt and the "now do this instance" in the message mirrors how skills read.
- The harness reads the stream, captures the final assistant summary and any
  artifacts, and extracts the **`transition` tool call arguments** as the
  proposal. Full transcript stays in the pi session file, referenced by id.

## The injected `transition` tool

Every worker gets one harness-injected tool. Calling it ends the stage.

```
transition(
  to: enum<reachable neighbors> | null,  # default: constrained to this state's neighbors
  blocked: boolean = false,    # "I can't get where I should; route me" — the escape hatch
  rationale: string,           # why this is the right next step / why stuck
  artifacts: [{name, path}] = []
)
```

Two things to note about the schema:

- **`to` is a constrained enum — this is the default and the recommended mode.**
  Its allowed values are the *reachable neighbors* of the current state (the tool
  schema's enum is rebuilt per spawn), so the worker *cannot* propose an
  invalid edge; the Navigator only fires on an explicit `blocked=true`. This is
  cheaper (fewer navigator calls), less error-prone, and keeps the worker's
  choice inside the declared graph by construction. `blocked=true` is always
  available as the escape hatch when none of the neighbors is right.

  The **open** mode — `to` as a free string, with the harness routing any
  unknown target to the Navigator — remains available as a per-machine opt-in
  (`transition_mode: open`). It's more faithful to the original "the CLI decides
  validity, else an agent reconciles" framing and earns its keep only when the
  graph is large enough that "where should I even go?" is a genuine question the
  worker should get to answer freely. Reach for it deliberately; the default
  stays constrained.
- **Nothing in this call is a gate.** Everything the worker passes here is a
  *proposal* plus evidence for the Judge. The gate that cannot be argued with is
  the edge's `:check`, which the harness runs after this call, in its own
  subprocess (see [03-ledger.md](03-ledger.md)).

## Three agent roles, three cost profiles

| Role | When | Model/thinking | Job |
|---|---|---|---|
| **Worker** | Every stage | Per-state (often the strong model, high thinking) | Do the stage's work; end with `transition`. |
| **Judge** | A transition with a `criteria` prompt | Cheap model, low thinking | Independently decide if `criteria` is met, given the worker output digest + artifacts. Returns `{pass\|fail, rationale}` via a constrained tool. |
| **Navigator** | Worker proposed an out-of-graph target or `blocked` | Cheap model, low thinking | Pick a reachable next state and write an entry-prompt addendum. Returns `{to (enum), entry_prompt}`. |

Keeping Judge and Navigator cheap and separate matters:

- **Judge independence** is what stops the worker grading its own homework. The
  worker wants to move on; the judge has no stake. For QA in particular, the
  judge sees only *outputs and artifacts*, never the worker's self-assessment.
- **Navigator boundedness** — cap navigator invocations per run and per state to
  avoid A→B→A ping-pong. On exceeding the cap, escalate to a `blocked`/`human`
  terminal and notify.

### Judge spawn (sketch)

```bash
pi --print --mode json --no-session \
   --provider anthropic --model "claude-haiku-4-5:low" \
   --no-builtin-tools --no-extensions \                  # see note below
   -e ~/.config/loop/ext/verdict-tool.ts \               # only a `verdict(pass, rationale)` tool
   --append-system-prompt "@${RENDERED_CRITERIA_PROMPT}" \
   "${WORKER_OUTPUT_DIGEST_AND_ARTIFACT_REFS}"
```

The judge has **no code tools** — it reads artifacts (paths passed in) and
returns a verdict. It cannot edit, deploy, or otherwise act.

**`--no-builtin-tools` is not enough on its own**: without `--no-extensions`, an
installed pi-extension (`mcp`) still auto-discovers into the spawn, handing the
Judge exactly the deploy-and-mutate surface its independence depends on not
having. Skill discovery is a *third* switch — `--no-skills` — and needs turning
off too. All three flags, on both the Judge and the Navigator.

### Navigator spawn (sketch)

```bash
pi --print --mode json --no-session \
   --provider anthropic --model "claude-haiku-4-5:low" \
   --no-builtin-tools -e ~/.loop/ext/choose-tool.ts \    # `choose(to<enum>, entry_prompt)`
   --append-system-prompt "@${RENDERED_NAVIGATOR_PROMPT}" \  # graph + each state's purpose
   "${LEDGER_DIGEST_AND_WORKER_RATIONALE}"
```

The navigator prompt includes the machine graph (states, their purposes, and the
edges out of the current state) so it routes within the declared structure. Its
`choose` tool's `to` is an enum of reachable states — it *cannot* pick an invalid
one; it can pick `escalate`.

## Session models

How much chat context carries between stages is per-state config:

- **`session: fresh`** (default) — a new session per stage. Clean, deterministic,
  cheapest. Continuity comes from the ledger digest + artifacts. Matches "spawn
  the next stage's agent".
- **`session: continue`** — reuse a session keyed by `sessionKey`. The classic use
  is a `implement ↔ debug` loop where the fixer should remember what it just
  tried; both states share `sessionKey: impl`, so cycle 2 of `implement`
  continues cycle 1's conversation via `pi --continue --session-id …`. This is the
  external-CLI analogue of `run-review`'s persistent implementer resumed with
  findings.
- **`session: fork`** — branch a session (`pi --fork`) so a stage explores from a
  prior context without polluting it. Useful for a speculative fix you might
  discard.

Trade-off: persistent sessions improve continuity and reduce re-explanation cost
but reintroduce context drift and make a stage less idempotent to re-run. Default
to `fresh`; opt into `continue` only for tight fix loops.

## Cost & concurrency controls

- **Budgets** are enforced by the harness from the `usage` in `worker_output`:
  `budget_usd`, `wallclock_s`, `max_transitions`, per-loop `max_cycles`. Exceeding
  any → `run_finished{status: aborted}` with a clear reason.
- **`loop run --dry-run`** walks the graph, renders every prompt, and prints an
  estimated token/cost envelope without spawning workers — a sanity check before
  you spend.
- **Concurrency:** v1 is sequential (a state machine has one active state).
  "Multiple different QA tests" fan-out is modeled as either sequential sub-stages
  or a future `fork/join` construct (see [07-risks.md](07-risks.md#parallelism)).
