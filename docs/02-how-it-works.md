# How a run works

This document explains what actually happens when you type `loop run`, and how to take a run apart afterwards to find out why it did what it did.

For the config and machine keys mentioned here, see [customizing](03-customizing.md). For flags, env vars, and exit codes, see the [CLI reference](04-cli-reference.md). For why the system is shaped this way, see the [design notes](05-design-notes.md).

## The shape of a run

A run is one ticket, one state machine, driven to a terminal.

The states of that machine are **agent stages**. Entering a state means rendering a prompt and spawning a coding agent (`pi`) as a subprocess in your project directory. The agent works, then ends its stage by calling an injected `transition` tool naming where it thinks the run should go next.

That call is a **proposal**, not a move. The harness decides whether to honour it. Between the proposal and the commit sit the guards: a deterministic shell command you wrote, and an independent cheap model that reads the evidence. Only after those pass does the harness write `transition_committed` and enter the next state.

Every decision, spawn, verdict, and commit is appended to a JSONL ledger. There is no mutable state file — the run's position is recomputed by folding the ledger from the top on every step. That is also what makes the run resumable after a crash.

```mermaid
flowchart TD
    A["enter state<br/>(render prompt, spawn Worker)"] --> B["Worker calls transition"]
    B --> C{"valid proposal?<br/>not blocked, to is a declared neighbor"}
    C -->|yes| D{"target is the<br/>escalation state?"}
    C -->|"blocked / to is null /<br/>unknown target"| N["Navigator picks a target"]
    N --> D
    D -->|yes| K["transition_committed"]
    D -->|no| G["guard tiers:<br/>structural → check → criteria"]
    G -->|pass| K
    G -->|"fail, on-fail retry"| A
    G -->|"fail, on-fail route"| K
    G -->|"fail, on-fail abort"| X["run_finished: failed"]
    K --> Y{"target is a terminal?"}
    Y -->|no| A
    Y -->|yes| Z["run_finished: done or failed"]
```

Two things in that diagram are easy to miss and matter a lot:

- A commit to the **escalation state bypasses every guard tier**, including the structural one. It does not need a declared transition.
- An `on-fail` **route also bypasses every guard tier** — and its backoff.

## One stage, start to finish

This is the exact ordered sequence the engine runs. Everything else in this document is a detail hanging off one of these steps.

The engine's outer loop reads the whole ledger, folds it into a run state, and dispatches on the resume point it derives from the ledger's tail. If the folded status is already finished, it returns immediately. If there is no `run_started` event yet, it appends one. Then:

**Entering a state**

1. **Terminal check.** If the state is a terminal, the run finishes here. The status is `Failed` if that terminal is the machine's `:escalation-state`, and `Done` otherwise.
2. **Budget check — before any process is spawned.** A breach appends a fatal `error` and `run_finished{status: aborted}` and stops.
3. **Compute `cycle` and `attempt`.** `cycle` is only meaningful for loop heads; every other state gets `1`. `attempt` is the count of prior attempts at this state in this cycle, plus one.
4. **Look up a pending Navigator addendum** — the `entry_prompt` the Navigator wrote when it redirected the run here, found by scanning back from the commit through that routing decision's own events. It reaches the playbook as `$ENTRY_ADDENDUM`.
5. **Build the stage.** Resolve the playbook, resolve the skill list, and render the prompt by substituting `$VAR` template variables into the playbook body.
6. **Append `state_entered`** — state, cycle, attempt, model, thinking, the resolved skill names, the MCP server names.
7. **Spawn the Worker** and stream-parse its stdout.
8. **If the process itself failed** (non-zero exit, spawn error), append a transient `error` and return; the fold will re-enter the state. After `MAX_CRASH_ATTEMPTS = 3` consecutive crashes the engine appends a `note` and escalates. The `error` carries the tail of pi's stderr, so a spawn failure leaves a diagnosis rather than just an exit code.
9. **Capture artifacts** claimed in the `transition` call — copy into the store. This happens _before_ the output event is written. A claim that cannot be resolved is recorded as an `error` and dropped; the rest proceed.
10. **Append `worker_output`** — the last non-empty assistant message as the summary, the captured artifact refs, and token/cost usage.
11. **If no proposal was emitted**, synthesize one: `{to: null, blocked: true, rationale: "worker ended its turn without calling transition"}`.
12. **Append `transition_proposed`.**
13. **Re-read and re-fold the ledger**, then route the proposal.

**Routing the proposal**

1. **Decide whether the Navigator is needed.** It is, if `blocked` is true, or `to` is null, or `to` is not a declared neighbor of the current state.
2. **If so, invoke the Navigator.** It picks a target or escalates; its choice may finish the run.
3. **If the target is the escalation state, commit directly** — no `select_edge`, no guard tiers.
4. **Select the edge.** The first `:transitions` entry matching `(from, to)` wins; duplicates after it are dead (`loop validate` flags them). No match means the harness writes `guard_checked{structural: fail}` and escalates.
5. **Run the guard pipeline** — structural, then check, then criteria.
6. **Append `guard_checked`** with each tier's outcome, the check's captured output, and the Judge's rationale.
7. **Fail → handle `:on-fail`. Pass → commit.**

**Committing**

If the target is a loop head and entering it would exceed that loop's `:max-cycles`, the exhaustion path runs instead. Otherwise the engine appends `transition_committed`, and _then_ sleeps the edge's `:backoff-s` — the event is durable before the sleep starts.

## The three roles

Every role is the same binary: `pi`, spawned as `pi --print --mode json` with stdin closed and stdout piped and parsed line by line. The binary comes from `LOOP_PI_BIN` (default `pi`). The working directory is always your project directory. **stderr is drained and its last 20 lines are kept** in all three cases — folded into the `error` event on a Worker crash, and into the fail-closed rationale of a Judge or Navigator that produced no marker. `loop run --verbose` echoes it live as well.

They differ in what they are allowed to do.

### Worker

The Worker is the stage. It does the actual work: reads files, edits code, runs tests, whatever the playbook tells it to. It is the only role with real capability, and the only one that can see your repository.

Its model comes from the resolution chain documented in [customizing](03-customizing.md#machinefnl--the-ticket-machine); the last resort is `config.fnl`'s `:worker`, defaulting to `claude-sonnet-5` at `medium` thinking.

```
--print --mode json
[--session-id <id>]
--provider <p> --model <model>:<thinking>
--no-skills
--skill <path>            (repeated, one per resolved skill)
-e <ext>                  (transition-tool.ts only)
--append-system-prompt <path-to-rendered-playbook>
<entry message>           (positional, last)
```

Environment it receives: `LOOP_REACHABLE` (the comma-joined list of declared neighbors), `LOOP_TRANSITION_MODE`, and the four scalars `TICKET_ID`, `STATE`, `CYCLE`, `ATTEMPT`. `PI_AGENT_DIR` is deliberately _not_ set, so pi's `mcp` extension reads your own `~/.pi/agent/mcp.json`.

Note what is **absent** from that flag list: no `--no-builtin-tools`, no `--no-extensions`. The Worker keeps bash, file editing, and pi's ambient extension discovery. Only skills are pinned shut, via `--no-skills` plus an explicit `--skill` per resolved skill.

> Withholding a skill bounds **instructions, not capability**. A stage that isn't given the `deploy` skill has no idea how you deploy — but it still has bash. Skills shape what an agent knows to do, not what it is able to do.

### Judge

The Judge evaluates one edge's `:criteria` against the evidence the harness hands it. It is a second opinion on whether the Worker actually did what it claims, and it is deliberately cheap: `claude-haiku-4-5` at `low` thinking by default.

```
--print --mode json --no-session
--provider <p> --model <m>:<t>
--no-builtin-tools --no-extensions --no-skills
-e verdict-tool.ts
--append-system-prompt <the criteria TEXT, not a path>
<judge message>
```

The only environment variable it gets is `LOOP_MOCK_ROLE`. No `LOOP_REACHABLE`, no `TICKET_ID`.

`--no-builtin-tools --no-extensions --no-skills` is the whole point. The Judge has exactly one tool — `verdict` — and no others. It cannot read a file, run bash, or reach an MCP server. **It judges only what it is handed**, which is:

- the Worker's summary, trimmed
- each artifact as `- <name> (<absolute path>)`
- when a `:check` ran, its output under a literally-prefixed block: `Output of the harness's own check for this transition (the worker did not produce this, and it exited zero):`

That block is the one piece of evidence on the line the Worker did not author.

### Navigator

The Navigator is the recovery path. It fires only when the Worker's proposal is unusable, reads the ledger digest, and picks a target from the declared neighbors — or escalates. Same cheap default as the Judge: `claude-haiku-4-5` at `low`.

Same isolation flags as the Judge, with `-e choose-tool.ts` instead. Its `--append-system-prompt` receives the **graph summary** as text, built to this grammar:

```
## States
- `<id>` — <description>          (or `(no description)`)
- `<terminal>` — terminal

## Edges out of `<from>`
- `<from>` → `<to>` (check: <first line of check>; criteria: <first line of criteria>)
```

Each edge shows only the **first line** of its check and criteria. This is why writing `:description` on your states is worth the keystrokes — it is what the Navigator reads when it decides where a stuck run should go.

Its positional message is the ledger digest plus, when available, `Worker rationale: …`, ` (worker reported blocked)`, and ` (worker proposed: <to>)`.

`LOOP_REACHABLE` for the Navigator is exactly `machine.neighbors(from)`. The literal `"escalate"` option is appended by `choose-tool.ts` itself, not by the harness.

## The three injected tools

Each role gets exactly one tool injected via `-e`. The three `.ts` files are compiled into the `loop` binary and written to `<config_dir>/ext/` on `loop init`. **`materialize_ext` overwrites them whenever the on-disk sha256 differs** — hand-editing one gets it reverted on the next `init`.

Each tool returns a marker line: a name, a space, then a JSON payload. The stream parser scans every `tool_execution_end` result for those prefixes after `trim_start()`. Surrounding prose is tolerated, and the **last** payload for a given name wins.

### `transition`

The Worker's only injected tool. Calling it ends the stage.

| Parameter   | Type             | Required  |
| ----------- | ---------------- | --------- |
| `to`        | state id         | optional* |
| `blocked`   | bool             | optional* |
| `rationale` | string           | **yes**   |
| `artifacts` | `[{name, path}]` | optional  |

\* The tool throws if neither `to` nor `blocked` is given.

Returns the text `LOOP_TRANSITION {json}`. Calling it causes: the artifacts to be captured, `worker_output` and `transition_proposed` to be appended, and the routing pass to begin.

**Constrained vs. open.** The schema of `to` depends on `LOOP_TRANSITION_MODE`, set from `:transition-mode` (machine key wins over config; default `"constrained"`):

- **`constrained`** — when `LOOP_REACHABLE` is non-empty, `to` is a union of string literals, one per reachable neighbor. The model _cannot_ name an invalid edge; the tool schema rejects it before the harness ever sees it.
- **`open`** — `to` is a free string. Unknown targets are legal to emit and get routed to the Navigator, which decides what the Worker actually meant.

Constrained mode is the reason the Navigator rarely fires in a healthy run: the only way to produce an unusable proposal is to be genuinely blocked, or to end the turn without calling the tool at all.

### `verdict`

The Judge's only tool.

| Parameter   | Type                                                        |
| ----------- | ----------------------------------------------------------- |
| `pass`      | bool                                                        |
| `rationale` | string — the schema asks it to "cite the specific evidence" |

Returns `LOOP_VERDICT {json}`. `pass` becomes the `criteria` tier's outcome; `rationale` is stored on the `guard_checked` event as `judge_rationale`.

### `choose`

The Navigator's only tool.

| Parameter      | Type                                                   |
| -------------- | ------------------------------------------------------ |
| `to`           | enum of `LOOP_REACHABLE` plus the literal `"escalate"` |
| `entry_prompt` | string — a get-back-on-track note for the next stage   |

Returns `LOOP_CHOICE {json}`. `entry_prompt` becomes `$ENTRY_ADDENDUM` in the next stage's prompt: the harness finds it by scanning back from the commit through the events of that one routing decision, so it survives the `guard_checked` a guarded route puts in between. The scan stops at the previous commit or state entry, which is what keeps a note from an earlier cycle out of a later stage's prompt.

**Fail-closed on a missing marker.** If a role's process finishes without emitting a usable marker, the harness does not guess:

| Role | Missing marker becomes |
| --- | --- |
| Worker | no proposal → the engine synthesizes `blocked: true` → the Navigator fires |
| Judge | `{pass: false, rationale: "judge returned no usable verdict"}`, plus the stderr tail |
| Navigator | `{to: "escalate"}` |

A **non-zero exit invalidates a Judge or Navigator marker even if one was present**. The Worker's non-zero exit is handled by the crash path instead (step 8 above).

## Guards

Three tiers, evaluated in order, cheapest first:

| # | Tier | What it is | Skipped when |
| --- | --- | --- | --- |
| 1 | `structural` | the edge must exist in `:transitions` | never — but it is really enforced by edge selection; in the guard function itself this tier is a hardcoded pass |
| 2 | `check` | the edge's `:check` shell command | the edge has no `:check` |
| 3 | `criteria` | the Judge, against the edge's `:criteria` | the edge has no `:criteria` |

`skip` counts as passing. A report passes when **no tier failed**.

**A failed `check` short-circuits: the Judge is never spawned.** That ordering is deliberate — you don't pay for a model call to evaluate a diff whose test suite just went red.

An edge with neither `:check` nor `:criteria` commits the Worker's proposal unexamined. `loop validate` emits a warning for exactly this, and warnings alone still exit 0.

**How a check command is executed:**

- The command string goes through the same `$VAR` substitution as a playbook body, so `$TICKET_ID`, `$STATE`, `$CYCLE`, `$ATTEMPT`, `$ARTIFACT_<NAME>` and the rest are all available. The four scalars `TICKET_ID`, `STATE`, `CYCLE`, `ATTEMPT` are _also_ exported as real environment variables to the subprocess.
- It is run as **`bash -c <cmd>`** — bash specifically, not `sh`, not your login shell. No profile is sourced.
- Working directory is the project directory. Stdin is null.
- **stdout and stderr are merged** into one temp file, so a failing command's error message is captured alongside its output.
- The engine polls for exit every 50 ms until the deadline.
- Default timeout is **120 seconds**; override per-edge with `:check {:cmd "…" :timeout-s N}`. On timeout the process is killed, the exit code is recorded as absent, and `\n[check timed out after {N}s]` is appended to the output.
- `passed = !timed_out && exit_code == 0`. **A non-zero exit is a normal guard failure, not a harness error.**
- The captured output is truncated to the **last 16 KiB**, prefixed with `[… N earlier bytes truncated …]\n`, then trimmed. Put the signal at the end of your command's output, not the beginning.

That truncated output is stored on `guard_checked.check_output` and, when the check passed, forwarded to the Judge.

## When a guard fails

Whichever tier failed, the edge's `:on-fail` decides what happens. It is a property of the edge, not of the tier.

| `:on-fail` | Behavior |
| --- | --- |
| `"retry"` (default) | Re-enter the **source** state at `attempt + 1`, same cycle. |
| `"abort"` | Finish the run immediately as `Failed`. |
| `{:route "x"}` | Commit straight to `x`. |

Two consequences worth internalizing:

- **A retry does not consume `max_transitions`.** No `transition_committed` is written, so the transition budget is untouched. Retries are free against that budget — and only against that budget: they still cost dollars and wallclock. A tight `:max-cycles` is what actually bounds a thrashing stage, not `max-transitions`.
- **A route skips every guard tier and the backoff.** It is an unconditional jump. The edge you route _to_ is not consulted, is not required to exist in `:transitions`, and nothing evaluates whether the destination is a sensible place to be. Routes are also **not counted** by `loop validate`'s terminal-reachability analysis, so a machine that only reaches its terminal via routes will still be flagged as having no path to a terminal.

On a retry, the whole stage runs again: fresh prompt render, fresh spawn, fresh session. Nothing from the failed attempt is carried into the next one except what is visible in the ledger digest.

## Loops and cycles

A `:loops` entry names a set of states and a cycle budget:

```fennel
:loops [{:name "fix"
         :states ["implement" "review" "test"]
         :max-cycles 4
         :on-exhausted "escalate"}]
```

**`states[0]` is the loop head.** Only the head counts cycles: entering `implement` for the third time makes it cycle 3, and every other state in the loop reports the head's current cycle. States outside any loop are always cycle 1.

`:max-cycles` is enforced **prospectively, in `commit`** — the check runs when the target of a commit is a loop head, _before_ the commit is appended. So the run never actually enters the (N+1)th cycle; it fails at the boundary.

Exhaustion appends a fatal `error`:

```
loop `fix` exhausted max_cycles=4 at head `implement`
```

and then does what `:on-exhausted` says — `"escalate"` (the default) or `"abort"`.

`loop validate` checks that each loop declares states, that every named state exists, and that the head is actually re-entered by some transition. A head nothing points back to is a loop that can never cycle, and is an error.

## Escalation

`:escalation-state` is the machine's designated failure destination. Committing to it bypasses edge selection and every guard tier — it does not need a declared transition from anywhere.

It fires on:

| Trigger                                                        | Where       |
| -------------------------------------------------------------- | ----------- |
| Navigator invocation cap exceeded                              | routing     |
| Edge selection found no matching transition                    | routing     |
| A loop exhausted `:max-cycles` with `:on-exhausted "escalate"` | commit      |
| 3 consecutive Worker process crashes                           | stage entry |

The Navigator's cap (`:max-invocations`, default 5) applies **both run-wide and per source state**. Hitting either limit means no Navigator spawn at all — the run escalates immediately. A Navigator spawn that returns no usable `LOOP_CHOICE` is coerced to `{to: "escalate"}`, with the spawn's stderr tail as the addendum so the escalation says why.

**Landing on the escalation state reports `Failed`, not `Done`** — even though it is a terminal like any other. That is the whole reason it is declared separately: it lets a machine have a "give up here" terminal that a script can distinguish from success by exit code alone.

**With no `:escalation-state` configured, an escalation ends the run as `Aborted`.**

## Budgets

Three limits, from `config.fnl`'s `:budgets` and optionally tightened by the machine's `:budgets` (per-field minimum — a machine can never raise a limit) and further by `--max-transitions` on the command line (also tightening only).

| Limit | Default | Comparison |
| --- | --- | --- |
| `:usd` | 15.0 | breach when `cost_usd > usd` (strictly greater) |
| `:wallclock-s` | 7200 | breach when `elapsed_s > wallclock_s` (strictly greater) |
| `:max-transitions` | 60 | breach when `transitions >= max_transitions` (greater-or-equal) |

They are checked in that order, first-wins. A breach appends a fatal `error` with a detail string plus `run_finished{status: aborted}`. **A budget breach is always `Aborted`, never `Failed`.**

Budgets are sampled at exactly two points: the top of stage entry — before any process is spawned — and on a crash-resumed guard check. Which produces two honest caveats (Worker, Judge, and Navigator spend all count toward `:usd`, and `:wallclock-s` accumulates across resumes):

- **Retries are free against `max_transitions`,** which counts committed transitions only.
- **Budgets are sampled between stages.** A single long Worker spawn can blow through the wallclock limit and will not be noticed until it finishes.

## The ledger

`<project>/.loop/ledger.jsonl`. Newline-delimited JSON, one event per line, append-only. This is the entire durable state of a run.

Note that `LOOP_STATE_DIR` does **not** move it. The ledger always lives beside the machine in the project's `.loop/`.

**The envelope is minimal:** every line is `ts` — RFC-3339 UTC with millisecond precision and a `Z` suffix — plus `type`, with all payload fields **flattened onto the same object**. There is no `seq`, no run id, no schema version. Ordering is file order.

```json
{
  "ts": "2026-07-26T14:03:11.482Z",
  "type": "transition_committed",
  "from": "implement",
  "to": "review",
  "cycle": 1
}
```

| `type` | Fields | Written when |
| --- | --- | --- |
| `run_started` | `ticket`, `machine_hash`, `budgets{usd,wallclock_s,max_transitions}` | first step of a run |
| `state_entered` | `state`, `cycle`, `attempt`, `session_id`, `model`, `thinking`, `skills[]`, `mcp[]` | immediately before spawning the Worker |
| `worker_output` | `state`, `cycle`, `summary`, `artifacts[{name,path}]`, `usage{tokens,cost_usd}` | after a clean Worker exit, after artifact capture |
| `transition_proposed` | `from`, `to` (nullable), `blocked`, `rationale`, `by` (`worker`\|`navigator`\|`harness`) | right after `worker_output` |
| `guard_checked` | `from`, `to`, `structural`, `check`, `criteria` (each `pass`\|`fail`\|`skip`), `check_output`, `judge_rationale`, `usage{tokens,cost_usd}` (the Judge's, zero when no criteria ran) | after the guard tiers run |
| `navigator_invoked` | `from`, `proposal`, `chosen_to`, `entry_prompt`, `usage` | when the Navigator fires |
| `transition_committed` | `from`, `to`, `cycle` | after guards pass, or on a route or escalation |
| `error` | `state`, `kind` (`transient`\|`fatal`), `detail` | budget breach, loop exhausted, Worker crash, dropped artifact claim |
| `note` | `text` | 3 crashes → escalating; otherwise a human annotation slot |
| `run_finished` | `status` (`done`\|`failed`\|`aborted`), `terminal_state`, `totals{cost_usd,wallclock_s,transitions}` | terminal |

### The ledger is a record, not a transcript

Two different things are being kept, and it matters which one you are reading.

The **ledger** is loop's own concise account of the run: decisions, evidence, spend. `worker_output.summary` is the Worker's digest of its own turn, not a log of it — the fields above are the whole schema, and there is deliberately no room in them for messages or tool calls.

The **transcript** is pi's. Every Worker spawn gets `--session-id <ticket>-<state>-<cycle>-<attempt>` (see [Worker](#worker)), and pi persists that session with the assistant messages, tool calls, tool results, commands, usage, and branching in it. `state_entered.session_id` is the only link between the two, and it is the entire reason it is on the line: loop stores the id so it never has to store the history.

Judge and Navigator spawns are sessionless on purpose (`--no-session`) — an independent verdict that leaves a resumable session behind is one more thing to accidentally continue — so they have no `session_id` and cannot be reopened.

`loop session` is how you cross from one to the other.

Every line also carries `elapsed_s`: the run's accumulated wallclock at the moment it was written, summed across every process that has driven this ledger. That is what lets `loop resume` keep counting instead of handing the run a fresh time budget — `ts` alone cannot, since the gap between an interrupted run and its resume is time during which nothing was running.

**Durability.** The file is opened once with append mode and held open. Each append stamps `ts` and `elapsed_s`, writes the line and its newline in a single `write_all`, then `sync_data()` — an fsync per event. Lines are never rewritten. A power cut loses at most the event currently being written.

**Torn-tail repair.** Opening the ledger truncates the file back to the end of its last whole line, then fsyncs the truncation. The operation is idempotent. On read, an unparseable **last** line is silently discarded; an unparseable **interior** line is a hard error:

```
corrupt ledger line 47 of /proj/.loop/ledger.jsonl: …
```

**State is folded, never stored.** Where the run is, how many cycles a loop has burned, how many attempts a state has had, what it has cost, which artifacts exist, where to resume — all of it is recomputed by replaying the ledger from the top on every engine step. Deleting the ledger starts over; there is nothing else to clean up. The fold `break`s at `run_finished`, so nothing written after a terminal event is ever read.

`totals.wallclock_s` folds out of the last event's `elapsed_s`, so it is meaningful for a run in progress and not only after `run_finished`.

## Inspecting a run

### `loop status`

```
loop status
loop status --json
```

Human mode:

```
running — at `review`
  5 transitions, $1.23, 12m3s
  cycles: implement#2, qa#1

recent:
  <ts>  <summary>
```

The header is `not started`, ``running — at `{state}` ``, or `finished — {status}` (rendered as `Done`, `Failed`, or `Aborted`). The `cycles:` line appears only when the machine loaded and at least one cycle has been counted. `recent:` shows the last 12 events, oldest-first within that window, each rendered as a one-line summary in one of these forms:

```
run_started <ticket>
→ <state> (cycle N, attempt M)
<state> done ($X.XX)
<from> proposes → <to>: <rationale, 60 chars>
<from> blocked: <rationale, 60 chars>
guard <from>→<to>: check=Pass criteria=Skip
navigator <from> → <chosen_to>
committed <from> → <to>
error (Transient): <detail, 60 chars>
note: <text, 70 chars>
run_finished Done
```

`--json` emits exactly five keys:

```json
{
  "current": "done",
  "cycles": {
    "qa-staging": 3
  },
  "navigator_invocations": 0,
  "status": "done",
  "totals": {
    "cost_usd": 3.58,
    "transitions": 10,
    "wallclock_s": 3414
  }
}
```

(Real output from [the worked example](../examples/), a finished run. Keys come out alphabetically ordered.)

`status` is `null` while the run is in progress; `totals.wallclock_s` is live. There is no resume point, no artifact list, and no ticket id in JSON mode; for those, read the ledger.

On a ledger with zero events, `--json` emits the same five keys with nulls and zeroes — the plain-text ``no run yet — `loop run` starts one`` belongs to the human mode only, so JSON output is always parseable.

### `loop logs`

For an event-by-event view without the `recent:` wrapper:

```
loop logs
loop logs -n 50
```

Human mode prints the last 20 events by default, oldest first within the selected tail. Each line contains the event timestamp and the same summary grammar as `loop status`. It prints ``no run yet — `loop run` starts one`` for an empty ledger and does not require `machine.fnl` to load.

Use `--raw` when a complete, machine-readable ledger is needed:

```
loop logs --raw | jq '…'
```

Raw mode emits the entire repaired ledger as JSONL, byte-for-byte, with no heading or status text. It is the path-independent replacement for reading `.loop/ledger.jsonl` directly; an empty ledger emits zero bytes. `--raw` and an explicit `-n` cannot be combined.

### `loop session`

```
loop session                          # picker over every Worker attempt
loop session implement                # picker, filtered to that exact state
loop session implement --latest       # newest implement attempt, no picker
loop session --latest                 # newest Worker attempt, no picker
```

Reopens a Worker's pi session. `status` and the ledger tell you what was decided; this is how you read what was actually said.

It reads nothing but the ledger. Every candidate is a `state_entered` with a non-empty `session_id`; the command then runs `pi --session <id>` in the project directory and hands over stdin, stdout, and stderr untouched. No machine, no toolbox, and no staged prompts are required — a mid-edit `machine.fnl` is often exactly when you want this.

**The picker is the normal path.** The ids are `<ticket>-<state>-<cycle>-<attempt>`: findable by a program, not memorable by a person. So you pick a row instead, newest-first:

```
implement — cycle 2, attempt 1 — 2026-07-26 12:04 — finished
  Added the retry guard and updated the tests. · claude-sonnet-5:high · $0.11 · 1 artifact
```

The first line is the identity — state, cycle, attempt, timestamp, outcome. When a machine happens to load, its state description is appended; the state id stays regardless, so rows never collapse into each other. The second line is the evidence: the Worker's own summary, or the recorded error, or a note that neither exists, then what ran the attempt and what it cost.

Controls: type to fuzzy-filter, `↑`/`↓` to move, `Ctrl+O` to change mode, `Enter` to open, `Esc`/`Ctrl+C` to cancel without launching anything. `Ctrl+O` cycles three candidate sets, named in the header:

| Mode               | Shows                                     |
| ------------------ | ----------------------------------------- |
| `All attempts`     | every `state_entered` with a session id   |
| `Latest per state` | the newest attempt for each exact state   |
| `Incomplete`       | attempts with no matching `worker_output` |

An optional positional state is an **exact prefilter** — `implement` never matches `implement-hotfix` — and it stays in force in all three modes. Search covers the visible row text and deliberately _not_ the session id: an opaque key should never be something a query has to name.

Three things it will not do quietly:

- **Choose without a terminal.** Without `--latest`, both stdin and stdout must be TTYs. A piped invocation fails with a hint rather than picking for you.
- **Hide a missing session.** It passes `--session`, not `--session-id`, so a session pi no longer holds is an error. `--session-id` would create an empty replacement under the same id, which looks identical to a Worker that did nothing.
- **Pretend an attempt finished.** An attempt with no `worker_output` still opens — that is precisely the transcript you want after a crash — but it warns on stderr first, because the session may also still be running.

The three outcome labels are evidence, read off the attempt's ledger episode (its own `state_entered` up to the next one, since `worker_output` carries no attempt field):

| Label        | Means                                              |
| ------------ | -------------------------------------------------- |
| `finished`   | a matching `worker_output` landed                  |
| `crashed`    | no `worker_output`, but an `error` in the episode  |
| `incomplete` | neither — still running, or killed without a trace |

Nothing here is written: loop neither parses nor mutates pi's session files, and never copies a transcript into `.loop/`. For what the sessions themselves contain and how to navigate one once it is open, see pi's own session documentation.

`--latest` selects the last usable candidate in reverse ledger order after the prefilter. There is no `--cycle` or `--attempt`: reaching further back is what the picker is for.

### `loop diagram`

```
loop diagram
```

Renders the machine as a mermaid state diagram on stdout, with no fences and no prose, so `loop diagram > machine.mmd` gives you a bare `.mmd` file. It is a pure function of the machine — no filesystem, no toolbox — so a machine with a dangling playbook reference still draws. Output is deterministic.

This is the fastest way to check that the machine you _wrote_ is the machine you _meant_. From the shipped `standard-ticket` template:

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

Reading it:

- Nodes get a `state "<label>" as <alias>` line only when the label differs from the alias — because the alias sanitizes non-alphanumerics to `_`. That is why `open-pr` gets an alias line and `implement` does not.
- The escalation state's label gets `" (escalation)"` appended.
- Solid edges are declared transitions, **in declaration order**. Back-edges from `:on-fail {:route …}` are grouped separately under the comment, one per distinct pair, sorted.
- `note right of <head>` blocks describe each loop.

**Edge-label grammar.** The label is a comma-joined list, built in this order:

| Fragment        | Appears when              |
| --------------- | ------------------------- |
| `check`         | the edge has a `:check`   |
| `judge`         | the edge has `:criteria`  |
| `unguarded`     | the edge has neither      |
| `wait {N}s`     | the edge has `:backoff-s` |
| `abort on fail` | `:on-fail` is `"abort"`   |

`retry` never appears — it is the default and would be noise on every edge — and a route gets its own arrow rather than a label fragment. So an edge labelled `check, judge, wait 30s` runs a command, then the Judge, then sleeps 30 seconds after committing.

### Reading the ledger directly

`loop logs --raw` is the path-independent way to reach the complete ledger. The flat envelope makes `jq` the right tool. Three that earn their keep:

**What actually happened, in order:**

```sh
loop logs --raw | jq -r 'select(.type=="transition_committed")
       | "\(.ts)  cycle \(.cycle)  \(.from) -> \(.to)"'
```

**Why a guard failed** — dumps the failing tier, the check's captured output, and the Judge's rationale together:

```sh
loop logs --raw | jq -r 'select(.type=="guard_checked" and (.check=="fail" or .criteria=="fail"))
       | "=== \(.from) -> \(.to)  check=\(.check) criteria=\(.criteria)",
         (.check_output // "(no check output)"),
         (.judge_rationale // "(no judge rationale)")'
```

This is usually the first thing to run on a run that thrashed: it tells you in one screen whether the deterministic check or the Judge is the thing rejecting your Worker.

**Where the money went, per state:**

```sh
loop logs --raw | jq -s 'map(select(.type=="worker_output"))
       | group_by(.state)
       | map({state: .[0].state,
              spawns: length,
              cost: (map(.usage.cost_usd) | add)})'
```

That total covers the Workers only. `guard_checked` carries the Judge's `usage` too, so a full accounting sums `worker_output`, `guard_checked`, and `navigator_invoked` — which is exactly what the fold and the digest do.

### Artifacts

A Worker declares artifacts in its `transition` call as `[{name, path}]`. The harness captures them **before** `worker_output` is appended.

1. A relative path joins the project root; an absolute one is used as-is. Both the root and the source are canonicalized, and the source must start with the root. That single comparison rejects absolute escapes, `..` walks, and out-of-tree symlinks.
2. The destination is `<project>/.loop/artifacts/<state>-<cycle>-<name>`, with every component sanitized (non-`[A-Za-z0-9._-]` → `-`).
3. It is written atomically — temp file, fsync, rename.
4. The ledger records `{name (unsanitized), path (project-relative)}`.

The copy is the point: it is a **snapshot**, so cycle two rewriting `diff.patch` does not change what cycle one handed off. Later stages see each artifact as `$ARTIFACT_<NAME>` (the name uppercased) and in the digest's `## Artifacts` section. The Judge is handed the absolute paths — though, having no tools, it cannot open them. Nothing is hashed: no consumer ever checked a hash, and recording one would assert an integrity guarantee loop does not make.

Two sharp edges:

- **The extension is not preserved.** A claim named `diff` pointing at `changes.patch` lands at `.loop/artifacts/implement-1-diff` — no `.patch`. If you need the extension, put it in the claim's name.
- **An unusable claim is dropped, not fatal.** A path that was never written, or one that escapes the project root, produces an `error` event naming the claim; that one artifact is omitted and the stage's others are captured normally. The run continues, and the missing evidence surfaces at whichever guard wanted it.

### The rendered prompt

The `--append-system-prompt` file the Worker actually received is kept on disk:

```
~/.local/state/loop/render/<sanitized-ticket>/<state>-<cycle>-<attempt>-system.md
```

(`sanitize` maps any character outside `[A-Za-z0-9_-]` to `-`. The root moves with `LOOP_STATE_DIR`.)

**This is the way to see what the agent was actually told.** Not the playbook source — the rendered result, with every `$VAR` already substituted. When a stage behaves as though it doesn't know the plan, open the render for that attempt and check whether `$PLAN` is in there at all.

That question comes up more than you would expect, because there is **no automatically-prepended context header**. Template variables reach the agent only where the playbook author interpolated them. A playbook that never writes `$TASK` gets no task; a playbook that never writes `$LEDGER_DIGEST` gets no digest. The render file is the proof.

The other half of the prompt is the positional entry message, which is not written to disk. It is short and contains **no ticket id, task, plan, or digest** — only the MCP connect instructions when the stage names servers, then:

```
You are entering **review**, cycle 2. (previous state: implement)
```

with the Navigator's addendum appended after a blank line when it routed here.

## Resuming an interrupted run

`loop resume` continues from the point the fold derives from the ledger's tail. It is the same code path as `loop run` with a resume flag; the only difference is which command refuses to act. `run` refuses on a non-empty ledger; `resume` refuses on an empty one.

The resume point comes entirely from the **last significant event**:

| Last significant event | Resumes at |
| --- | --- |
| a `run_finished` (status set) | nothing — the run is over |
| nothing, or only `run_started` | fresh, at the machine's `:entry` |
| `state_entered` | re-enter that state — **the whole stage re-runs** |
| `worker_output` | re-enter that state — **the whole stage re-runs** |
| `transition_proposed` | re-run the guards on that proposal |
| `transition_committed` | enter the committed target |

`guard_checked` and `navigator_invoked` **do not advance the tail**. A `navigator_invoked` sitting after a proposal still resumes at the guard check and re-invokes the Navigator from scratch.

**The consequence: an interrupted stage re-runs from scratch, at `attempt + 1`.** There is no partial-stage recovery. If the Worker spent twenty minutes editing files and the process died before `worker_output` was appended, those edits are still on disk — but the stage runs again from its rendered prompt with no memory of having run.

**So stages must be idempotent.** A stage that appends to a file, posts a comment, or opens a PR unconditionally will do it twice. Write stages that check before they act: "if the PR already exists, update it"; "if the migration is already applied, skip it". The ledger digest is available to the playbook via `$LEDGER_DIGEST`, and `$ATTEMPT` tells the agent which try this is.

A re-entry after a crash is marked: the playbook sees `$CRASHED` as `1` (empty on a clean entry), which is the hook for "check whether I already opened that PR" without having to infer it from `$ATTEMPT`.
