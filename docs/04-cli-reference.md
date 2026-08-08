# CLI reference

`loop` — "A local, ticket-level agent orchestrator".

Twelve subcommands: [`init`](#loop-init), [`validate`](#loop-validate), [`preview`](#loop-preview), [`diagram`](#loop-diagram), [`run`](#loop-run), [`resume`](#loop-resume), [`status`](#loop-status), [`recap`](#loop-recap), [`logs`](#loop-logs), [`sessions`](#loop-sessions), [`session`](#loop-session), [`doctor`](#loop-doctor).

For what the runtime actually does with the machine, see [02-how-it-works.md](02-how-it-works.md). For the keys inside `machine.fnl`, see [03-customizing.md](03-customizing.md).

## Global flags

| Flag | Type | Default | Meaning |
| --- | --- | --- | --- |
| `--dir <DIR>`, `-C <DIR>` | path | current directory | "Project directory (default: the current directory)." |
| `--version` | — | — | clap-generated version. |
| `--help`, `-h` | — | — | clap-generated help. |

`--dir` is global: it may appear before or after the subcommand. It sets `project_dir`, which anchors `.loop/`, the ledger, artifacts, and the cwd of every spawned process.

## `loop init`

```
loop init <TICKET> [--from <DIR>]
```

> Scaffold ./.loop/ — the machine, its prose, and its stage prompts.

| Flag / arg | Type | Default | Meaning |
| --- | --- | --- | --- |
| `<TICKET>` | string, required | — | "Ticket id, e.g. PROJ-1487." |
| `--from <DIR>` | path | — | "Copy an existing `.loop/`-shaped directory instead of the built-in template." |

One scaffold phase, into `<project>/.loop/`. Every file is written with a _write-if-absent_ rule — **nothing existing is ever overwritten**. Bails first if the machine already exists:

```
<project>/.loop/machine.fnl already exists — delete .loop/ to start a new ticket
```

**Without `--from`**, the bundled templates are written out of the binary — no fetch, no network, nothing read from outside the project:

| Path                               | From                          |
| ---------------------------------- | ----------------------------- |
| `machine.fnl`                      | the bundled `standard-ticket` |
| `stage-prompts/implement.md`       | bundled                       |
| `stage-prompts/review.md`          | bundled                       |
| `stage-prompts/qa.md`              | bundled                       |
| `stage-prompts/open-pr.md`         | bundled                       |
| `stage-prompts/debug-transient.md` | bundled                       |

**With `--from <DIR>`**, the tree under `<DIR>` is copied in instead, recursively, never overwriting. A leading `~/` is expanded. Three names at the top level of `<DIR>` are **skipped**, because they are what a _run_ leaves behind rather than part of the ticket: `ledger.jsonl`, `run/`, and `artifacts/`. The directory you reuse is normally one whose ticket you finished, so it has all three; carrying the ledger over would hand the new ticket a completed run, and `loop run` would refuse to start on it. Two failures are specific to this path:

```
--from <DIR> is not a directory
<DIR> has no machine.fnl — --from wants a directory shaped like .loop/
```

Either way, `init` then writes:

| Path             | Note                                    |
| ---------------- | --------------------------------------- |
| `task.md`        | the bundled task template, if absent    |
| `plan.md`        | the bundled plan template, if absent    |
| `stage-prompts/` | created empty if `--from` supplied none |
| `skills/`        | created empty                           |
| `.gitignore`     | one line, `run/`, if absent             |

Each file actually created prints `  created <path>`.

`machine.fnl`, `task.md`, and `plan.md` get this ticket's id stamped on them — a literal text substitution, **not** the `$VAR` render engine used at runtime. No other placeholder is expanded at init time. Under `--from`, only `machine.fnl` is rewritten, since a `task.md` copied from the source is left exactly as it was.

The stamp handles both shapes a source comes in. A bundled template still contains the literal `$TICKET`, which is replaced. A `--from` source produced by an earlier `loop init` does not — its own id was substituted in when it was created — so the **value** of the first `:ticket` key is rewritten in place instead. Without that, `loop init PROJ-99 --from` a finished PROJ-1 ticket would leave `:ticket "PROJ-1"` in the machine, and PROJ-1 would then name every session id, status line, and recap header the new run wrote. The match is line-oriented and takes the first line whose code opens with `:ticket`, so the key cannot be confused with a comment that mentions it; a machine with no `:ticket` at all is left untouched and fails to load downstream, which is the better error.

`init` does **not** create `artifacts/`, `ledger.jsonl`, or `run/` — those appear when something needs them. That holds under `--from` too, which is why those three are skipped by the copy.

Closing output:

```

initialized <project>/.loop for <TICKET>
  1. write .loop/task.md and .loop/plan.md
  2. hack .loop/machine.fnl into the shape this ticket needs
  3. loop validate
  4. loop run
```

## `loop validate`

```
loop validate
```

> Lint the machine: reachability, dangling references, guard sanity.

No flags.

Loads `machine.fnl`, resolves every stage prompt and skill inside `.loop/`, and prints one line per diagnostic in the form:

```
{tag}  {where}: {message}
```

`tag` is `error` or `warn ` (trailing space, for column alignment).

### Diagnostics

| Sev | Where | Message |
| --- | --- | --- |
| error | `machine` | entry state `{e}` is not a defined state |
| error | `{from} -> {to}` | transition `from` `{f}` names no state or terminal |
| error | `{from} -> {to}` | transition `to` `{t}` names no state or terminal |
| error | state id | state `{id}` is unreachable from entry `{e}` |
| error | state id | state `{id}` has no path to any terminal |
| error | state id | stage prompt for state `{id}` does not resolve in .loop/stage-prompts/ |
| error | state id | skill `{n}` on state `{id}` does not resolve in .loop/skills/ — with ``(from `:defaults {:skills ..}`)`` after the state id when the name came from there rather than from the state |
| error | state id | state `{id}` names MCP servers, but `mcp` is not in `:pi-extensions` — the stage would be told to call a tool it does not have |
| error | loop name | loop `{n}` declares no states |
| error | loop name | loop `{n}` references unknown state `{s}` |
| error | loop name | loop `{n}`'s head `{h}` is never re-entered by any transition |
| error | `machine` | escalation_state `{e}` names no state or terminal |
| error | `{from}` | duplicate transition `{f}` → `{t}`: only the first is ever taken — merge them into one edge |
| warn | `{from}` | transition `{f}` → `{t}` has neither `check` nor `criteria`: the worker's proposal is committed unexamined |

Clean run (zero diagnostics of any severity):

```
{ticket} — {N} states, {M} transitions, no problems found
```

### Exit and edge cases

- Any error → exit 1 with `error: {n} error(s)` on stderr, after the diagnostic lines.
- **Warnings alone exit 0**, but suppress the "no problems found" line — a warning-only run prints only the `warn ` lines.
- Reachability walks only edges into _defined states_, starting at `entry`; it is skipped entirely when `entry` is undefined.
- MCP server names are never checked — loop never reads `~/.pi/agent/mcp.json`.

### One semantics gap

- **Terminal-reachability ignores `on_fail: route` edges.** The reverse BFS covers declared `:transitions` only, so a state whose only way forward is a guard-failure route is reported as having no path to a terminal.

Skills and MCP servers are checked as the **effective union** — the machine's `:defaults` plus the state's own — because that union is what a spawn loads. A diagnostic for a name that came from `:defaults` says so.

## `loop preview`

```
loop preview [<STATE>]
```

> Show what a run would resolve to, without spawning anything.

| Flag / arg | Type | Default | Meaning |
| --- | --- | --- | --- |
| `<STATE>` | string, optional | — | "Detail one state instead of summarizing the whole machine." |

Answers "what will this loop do?" using the run's own resolvers. Every value in the report comes from the same code path `loop run` uses to build a stage — the four-layer model merge, stage prompt and skill resolution, the effective skill/MCP unions, `$VAR` substitution — stopped short of every write that stage building does.

### Read-only guarantees

Preview performs **no** side effect. Specifically it does not:

- spawn `pi`, or any Worker, Judge, or Navigator;
- run a `:check` command, connect to an MCP server, or test a credential;
- create `.loop/ledger.jsonl` or `.loop/artifacts/`;
- write anything under `.loop/run/` — the representative render is built in memory, and the rendered-prompt path it reports is where a run _would_ write.

It reads `machine.fnl`, the task/plan prose, and whatever stage prompts and skills resolve. Output is deterministic: the same inputs produce byte-identical output, since every collection is printed in the machine's own order (states alphabetically by id from the IR's `BTreeMap`, transitions and loops in declaration order).

### Whole-machine form

Sections, in order:

| Section | Contents |
| --- | --- |
| header | ticket, state / transition / loop counts |
| — | source path, entry, terminals, escalation state, effective budgets, Judge and Navigator models with the invocation cap |
| `context` | task and plan line/char counts with their first line, and the QA case ids |
| `states` | every state: description, resolved stage prompt name and path, resolved `provider/model:thinking`, effective skills with resolved paths, effective MCP names, reachable states, then each outgoing edge with its check command, timeout, criteria, `:on-fail` action, `:max-attempts` (on retry edges only, where it can bite), and backoff |
| `loops` | each loop's head, member states, `:max-cycles`, and exhaustion behavior |
| `validation` | the diagnostics, or `no problems found` |

Values are printed in one column; an absent optional value or an empty list reads `(none)`. Budget durations use the same formatting as [`loop run`](#loop-run).

### State form

`loop preview <STATE>` prints that state's block from the whole-machine form, then adds:

| Section | Contents |
| --- | --- |
| `stage prompt` | how the state names it (`name` / `path` / inline `:prompt`) and the file it resolved to |
| `stage prompt frontmatter` | `name`, `description`, `model`, `thinking` as parsed |
| `worker invocation` | the `--model` flag, provider, each skill's `--skill` path, MCP names, reachable states, cwd, the rendered-prompt path pattern, the four exported environment variable names, and the deterministic session id |
| `template variables` | the `$NAME`s the body writes that are loop variables, and the ones that will pass through untouched |
| `stage prompt body` | the body as authored, unrendered |
| `representative render` | the substituted system prompt and the entry message, under the limitation notice below |

Only environment **names** are listed, never inherited process environment or credentials.

### The representative render is not the future prompt

It is built with **cycle 1, attempt 1, no previous state, no artifacts, and an empty ledger digest**, and the report says so beside it. `$PREV_STATE`, `$LEDGER_DIGEST`, `$CYCLE`, `$ATTEMPT`, `$CRASHED`, `$ENTRY_ADDENDUM`, the `$ARTIFACT_*` paths, and the Navigator's addendum all depend on where a run has already been, so none of them can be known before the run exists. What the render does establish exactly is **which** variables the stage prompt interpolates — the thing that is wrong often enough to be worth checking.

The digest is empty rather than the header block `$LEDGER_DIGEST` renders to on a fresh ledger, so the render is not exact even for the entry state on a first run.

### Validation

Preview runs the full [`loop validate`](#loop-validate) linter — the same function, the same diagnostics, the same `{tag}  {where}: {message}` wording. There is no weaker preview-only check.

The report is printed **first**, then the diagnostics, so problems are the last thing on screen. A state whose stage prompt or skills do not resolve still gets a block; the fields it cannot compute read `unresolved` with the searched paths.

### Exit and edge cases

- Any validation **error** → the full report and diagnostics print, then exit 1 with `error: {n} error(s) — this machine will not run as previewed` on stderr.
- **Warnings alone exit 0**, with the warning lines under `validation`.
- An unknown `<STATE>` exits 1 **before printing anything**, listing the states that do exist: ``no state `{id}` in {path} — states: {a, b, c}``. A terminal is not a state, so `loop preview done` is this error.
- A missing or unparseable `machine.fnl` errors the same way it does for every command that loads the graph.

## `loop diagram`

```
loop diagram
```

> Render the machine as a mermaid state diagram, on stdout.

No flags.

Writes mermaid to stdout with no code fences and no surrounding prose, so it pipes directly:

```
loop diagram > machine.mmd
```

Output is deterministic and a pure function of the machine IR — it reads nothing off disk but the machine file, so a machine with a dangling `:stage-prompt` or an unresolvable skill still draws. It still requires the machine to load: a missing or unparseable `machine.fnl` is an error.

For the node/edge/label grammar (aliasing, edge labels, guard-fail back-edges, loop notes), see [02-how-it-works.md](02-how-it-works.md).

## `loop run`

```
loop run [--max-transitions <N>]
```

> Drive the machine to a terminal.

| Flag | Type | Default | Meaning |
| --- | --- | --- | --- |
| `--max-transitions <N>` | u32 | machine budget | "Stop after this many transitions, on top of the machine's budget." |
| `-v`, `--verbose` | bool | false | Echo each pi spawn's stderr as it runs. |

Loads config + machine, opens the ledger, and steps the engine until the run reaches a terminal or trips a guardrail.

`--max-transitions` **only tightens**: it is merged with the effective budget by per-field minimum, so a value larger than the machine's own `max-transitions` has no effect. It cannot relax `usd` or `wallclock-s` either way.

Refuses to start over an existing run:

```
<project>/.loop/ledger.jsonl already has a run — use `loop resume`, or delete it to start over
```

Missing machine (shared by every command that loads the graph):

```
no machine at <project>/.loop/machine.fnl — run `loop init <TICKET>` first
```

### Final line

Printed to stdout after a blank line, for every outcome including failures:

```

{Status} — {terminal} after {n} transition(s), ${cost}, {t} token(s), {duration}
```

`{Status}` is `Done`, `Failed`, or `Aborted`. `{terminal}` falls back to `(no terminal)`. Cost is two decimal places, and `{t}` is every token every role burned — Worker, Judge, and Navigator together, the same figure the `:usd` budget is charged against. This line, `loop status`'s totals, and the digest's `totals:` all render through one function, so the four fields and their order are the same in all three. Duration formatting:

| Seconds   | Format     | Example |
| --------- | ---------- | ------- |
| `< 60`    | `{s}s`     | `47s`   |
| `< 3600`  | `{m}m{s}s` | `12m3s` |
| otherwise | `{h}h{m}m` | `2h5m`  |

### Exit behavior

| Status | Exit | stderr |
| --- | --- | --- |
| `Done` | 0 | — |
| `Failed` | 1 | ``error: run ended at `{state}` without completing — see `loop status` `` |
| `Aborted` | 1 | ``error: run aborted — see `loop status` for the guardrail`` |

The final summary line is printed _before_ the error, so a failed run emits both.

## `loop resume`

```
loop resume [--max-transitions <N>]
```

> Continue an interrupted run from the folded resume point.

| Flag | Type | Default | Meaning |
| --- | --- | --- | --- |
| `--max-transitions <N>` | u32 | machine budget | Same tightening-only semantics as [`loop run`](#loop-run). |
| `-v`, `--verbose` | bool | false | Same as [`loop run`](#loop-run). |

Identical to `loop run` in every respect except the resume flag — same loading, same budget merge, same final line, same exit behavior. The only difference is the ledger precondition, which is inverted:

```
nothing to resume: <project>/.loop/ledger.jsonl is empty
```

Where the run picks back up is derived by folding the ledger tail; see [resume points](02-how-it-works.md#resuming-an-interrupted-run).

`wallclock-s` bounds the run, not the process: every ledger line carries the run's accumulated `elapsed_s`, and a resume picks the clock up from the last one. Resuming does not buy a fresh time budget.

## `loop status`

```
loop status [--json]
```

> Pretty-print the folded ledger: where the run is and how it got there.

| Flag | Type | Default | Meaning |
| --- | --- | --- | --- |
| `--json` | bool | false | Emit the folded summary as JSON instead of the human view. |

Reads the ledger and folds it. Does not require the machine to load — if `machine.fnl` is missing or mid-edit, status still works; it only loses the `cycles:` line (and cycle counting falls back to treating every state as a loop head).

**Empty ledger, both modes:**

```
no run yet — `loop run` starts one
```

### `--json`

Exactly five keys:

| Key | Type |
| --- | --- |
| `current` | string or null |
| `status` | `"done"` \| `"failed"` \| `"aborted"`, or null while running |
| `cycles` | object, state id → integer |
| `totals` | object: `cost_usd` float, `wallclock_s` integer, `transitions` integer |
| `navigator_invocations` | integer |

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

Keys are emitted in alphabetical order, not the order listed in the table above. No resume point, no artifacts, no ticket id in JSON mode.

On a ledger with zero events the same five keys come out with nulls and zeroes — the human-readable ``no run yet — `loop run` starts one`` belongs to the default mode only, so `--json` is always parseable.

`totals.wallclock_s` folds out of the last event's `elapsed_s`, so it is meaningful mid-run and accumulates across resumes rather than restarting.

### Human mode

```
unfinished — last at `review`
  5 transition(s), $1.23, 38104 token(s), 12m3s
  cycles: implement#2, qa#1

recent:
  <ts>  <summary>
```

Header is one of `not started`, ``unfinished — last at `{state}` ``, or ``finished — {Status} at `{state}` `` — the identical string `loop recap` prints as its `outcome:`, since both render it through one function. "Unfinished" rather than "running": this command folds a ledger, so it cannot tell a live run from one whose process died. The `cycles:` line appears only when the machine loaded and at least one cycle was counted. `recent:` lists the last 12 ledger events, oldest-first within that window. Per-event summary forms:

| Event | Summary |
| --- | --- |
| `run_started` | `run_started <ticket>` |
| `state_entered` | `→ <state> (cycle N, attempt M)` |
| `worker_output` | `<state> done ($X.XX)` |
| `transition_proposed` | `<from> proposes → <to>: <rationale…60>` |
| `transition_proposed` (blocked) | `<from> blocked: <rationale…60>` |
| `guard_checked` | `guard <from>→<to>: check=Pass criteria=Skip` |
| `navigator_invoked` | `navigator <from> → <chosen_to>` |
| `transition_committed` | `committed <from> → <to>` |
| `error` | `error (Transient): <detail…60>` |
| `note` | `note: <text…70>` |
| `run_finished` | `run_finished Done` |

## `loop recap`

```
loop recap
loop recap > run-recap.md
```

> Explain the recorded run: every attempt, the evidence behind it, and why it ended.

No flags. The post-run counterpart to [`preview`](#loop-preview): that command explains the declaration, this one explains the observed execution. Markdown to stdout, so it redirects into a ticket write-up unchanged.

**Deterministic.** No LLM, no clock, no network, and nothing written. The report is a pure function of the ledger, so the same ledger renders byte-identical output on every invocation.

### Output sections

Always these four, always in this order:

| Section | Contents |
| --- | --- |
| `# <ticket> — recap` | Heading and a one-line provenance note. |
| `## Run summary` | Start timestamp, the budgets recorded on `run_started`, the machine hash and whether the machine on disk still matches it, outcome, totals, cycle counts, Navigator invocation count, attempt count. Any events recorded before the first `state_entered` are appended here. |
| `## Attempt timeline` | One `###` section per `state_entered`, in ledger order. Header bullets: entered timestamp and elapsed, `model:thinking`, skills, MCP, session id. Then the episode's events in ledger order — Worker summary, usage and artifacts; the proposal and its rationale; each guard tier's outcome with the full check output and the full Judge rationale; the Navigator's choice; the committed move; any errors and notes before the next attempt. |
| `## Why it ended` | For a finished run: status, terminal state, the timestamp, and `run_finished`'s totals — plus the last fatal `error` when the status is not `Done`. For an unfinished one: the folded resume point and the last durable event. |
| `## Inspecting further` | A `loop sessions <state>` line per state with a reopenable attempt, and the `loop logs --raw \| jq` recipes for the complete stream. |

### Evidence labels

Claims in the timeline are attributed to whoever made them. `**Worker**` is the Worker's own account and is not treated as proof of anything; `**Check**` is output from a command the harness ran in its own process; `**Judge**` is an independent verdict; `**Committed**` is the harness's decision. Artifact lines are Worker claims — the harness captured the file, not its contents' truth.

Multi-line evidence (Worker summaries, check output, Judge rationales, error details) is reproduced **in full**, inside a fence grown past any backtick run in the text so arbitrary build output cannot escape its block. Nothing in the timeline is truncated; the 60-character summaries belong to [`status`](#loop-status) and [`logs`](#loop-logs).

### Partial runs

Completion is not required. A run in flight or interrupted is reported to date, with the resume point and last durable event in place of a terminal transition. Attempts with no `worker_output`, no session id, no artifacts, or no commit still get their section — a failed attempt that produced nothing is not omitted.

### The machine, and the hash warning

`machine.fnl` is loaded opportunistically and used **only** when `Machine::source_hash` equals the `machine_hash` recorded on `run_started`. On a match the report adds state descriptions to attempt headings and folds cycles against the machine's declared loop heads.

Otherwise — the machine is missing, fails to load, the ledger has no `run_started`, or the hashes differ — the recap:

- prints `- machine on disk: CHANGED — now <hash>` (or `not loaded`) in the run summary;
- omits state descriptions entirely;
- folds machine-agnostically, so `cycles` counts re-entries of every state and says so on the line;
- and, on a hash mismatch specifically, writes a warning to **stderr**:

```
warning: <path>/machine.fnl has changed since this run started (ledger <a>, on disk <b>) — the recap reports only what the ledger recorded
```

stderr rather than stdout, so `loop recap > run-recap.md` still produces a clean file; the report carries the same fact in its own summary.

### Exit and edge cases

The report goes to stdout in full; only the hash warning goes to stderr. **An empty ledger is an error**, not an empty report:

```
error: no run to recap: <path>/.loop/ledger.jsonl is empty — `loop run` starts one
```

Opening the ledger repairs a torn trailing line first, exactly as [`status`](#loop-status) does. A recap of a `Failed` or `Aborted` run still exits 0 — it is a report, and [`loop run`](#loop-run) owns the exit code a CI wrapper gates on. Exit 1 is reserved for an empty, unreadable, or interior-corrupt ledger.

## `loop logs`

```
loop logs [-n <N>]
loop logs --raw
```

> Show recent ledger events, or the complete ledger as JSONL.

| Flag     | Type  | Default | Meaning                                      |
| -------- | ----- | ------- | -------------------------------------------- |
| `-n <N>` | usize | `20`    | Number of events in the human-readable tail. |
| `--raw`  | bool  | false   | Emit the complete repaired ledger as JSONL.  |

`logs` opens the ledger through the normal `Ledger` reader, so a torn trailing write is repaired before anything is printed. It does not load `machine.fnl`; the ledger is its only source, so it works while the machine is missing or invalid.

Human mode prints one event per line, oldest first within the selected tail, as `<timestamp>  <status summary>`. It has no `recent:` wrapper and defaults to the last 20 events. If fewer than `N` events exist, it prints all of them. An empty ledger prints:

```
no run yet — `loop run` starts one
```

`--raw` ignores the human tail and writes the entire repaired ledger to stdout exactly as JSONL: no heading, status message, reformatting, or filtering. An empty ledger writes zero bytes. `--raw` conflicts with an explicitly supplied `-n`, so a command such as `loop logs --raw -n 50` is a command-line usage error and exits 2 without output.

Valid ledger content goes only to stdout. A ledger read or parse failure is reported to stderr and exits 1; successful commands exit 0.

## `loop sessions`

```
loop sessions [<STATE>]
```

> List every recorded Worker attempt: time, state, cycle, attempt, outcome, session id, evidence.

Reads the ledger, builds one candidate per `state_entered` that carries a non-empty `session_id`, and prints one line per attempt on stdout in **ledger order, oldest first**. It launches nothing. Its job is to give you the id that [`loop session`](#loop-session) wants.

Requires **neither a loadable machine nor a resolvable stage prompt**. Only the ledger and the project path are needed. Opening the ledger repairs a torn trailing line, exactly as [`loop status`](#loop-status) does, so an attempt interrupted mid-write is still listed.

Judge and Navigator spawns run with `--no-session` and never appear here.

### Candidates

One per `state_entered` with a session id. Ledger position is the order; `ts` is display metadata only, so a skewed clock cannot reorder history.

Each candidate's evidence comes from its **ledger episode** — from its own `state_entered` up to the next one. Within that window a `worker_output` with the same `state` and `cycle` supplies the summary, and any `error` supplies the failure detail. `worker_output` has no `attempt` field, so the episode boundary is what keeps a retry's summary off the crashed attempt's row.

| Outcome label | Condition                                         |
| ------------- | ------------------------------------------------- |
| `finished`    | a matching `worker_output` in the episode         |
| `crashed`     | no `worker_output`, but an `error` in the episode |
| `incomplete`  | neither                                           |

`<STATE>` is an **exact** filter, not a prefix or fuzzy one: `implement` never lists `implement-hotfix`.

### Columns

```
2026-07-26T12:01  implement        1  1  crashed     PROJ-9-implement-1-1        error: executor lost
2026-07-26T12:03  implement        1  2  finished    PROJ-9-implement-1-2        Added the retry guard and updated the tests.
2026-07-26T12:05  review-the-diff  1  1  incomplete  PROJ-9-review-the-diff-1-1
```

| # | Column | Notes |
| --- | --- | --- |
| 1 | timestamp | `state_entered`, local zone, to the minute. Date and time are joined with `T` so the field never splits in two. |
| 2 | state | exactly as the ledger recorded it |
| 3 | cycle | right-aligned |
| 4 | attempt | right-aligned |
| 5 | outcome | `finished`, `crashed`, or `incomplete` |
| 6 | session id | what `loop session <ID>` takes |
| 7 | evidence | the Worker's summary, else `error: <detail>`, else nothing at all |

Columns are padded to the widest row actually printed, and there is no header line — the output is a pipeline's input before it is a screen's. Fields 1–6 are single whitespace-free tokens in every row, so `awk '{print $6}'` is the session id whatever the state names in this ledger look like. The evidence column is last precisely because it is the only one that can contain spaces; it is collapsed to a single line and truncated to 72 characters, so one attempt is always one row.

That is what makes the shell the picker:

```sh
loop sessions | fzf | awk '{print $6}' | xargs loop session
loop sessions implement | grep crashed
loop sessions | awk '$5=="incomplete"'
```

### Exit and edge cases

No usable candidate is an error on stderr with exit 1, not an empty success — a pipeline that prints nothing has to say why:

```
error: no Worker session in /proj/.loop/ledger.jsonl matching any state — sessions come from `state_entered.session_id`; `loop status` shows what the ledger holds
```

The message names the requested filter (`any state`, or ``state `deploy` ``), so it distinguishes "that state never ran" from "nothing in this ledger has a session id at all" — which is what a ledger written before session ids looks like. Valid listing content goes only to stdout.

## `loop session`

```
loop session <ID>
loop session --latest [<STATE>]
```

> Reopen a Worker's pi session by id — `loop sessions` prints the ids.

| Flag / arg | Type | Default | Meaning |
| --- | --- | --- | --- |
| `<ID>` | string | — | The session id to reopen, from [`loop sessions`](#loop-sessions). With `--latest` this is a state filter instead. |
| `--latest` | bool | false | Reopen the newest recorded attempt rather than naming an id. |

Resolves the attempt, prints one line naming it, and executes `pi --session <id>` in the project directory. Like `loop sessions` it needs neither a loadable machine nor a resolvable stage prompt, and repairs a torn trailing ledger line on open.

### Selection

| Invocation                      | Behavior                                 |
| ------------------------------- | ---------------------------------------- |
| `loop session <ID>`             | the attempt with exactly that session id |
| `loop session --latest`         | the last candidate in ledger order       |
| `loop session --latest <STATE>` | the last candidate at that exact state   |
| `loop session`                  | error — see below                        |

An id is matched exactly and searched from the newest end, so a ledger that records the same id twice (a resume re-enters the same state, cycle and attempt, and the id is derived from exactly those four) opens the later episode — the one that describes the session as it now stands.

`--latest` is the deterministic path for scripts and CI: one input, one answer, no terminal required. There is no `--cycle` or `--attempt`; reaching a particular older attempt is what the listing's ids are for.

### The picker is gone

`loop session` with no argument used to open a full-screen fuzzy picker. It does not:

```
error: `loop session` no longer opens a picker — run `loop sessions` to list every recorded attempt, then `loop session <ID>` with an id from that listing. `loop session --latest [STATE]` still opens the newest attempt without naming one.
```

The old positional was a _state_, so `loop session implement` is the likeliest stale invocation. It fails with the two commands that do work:

```
error: no attempt in /proj/.loop/ledger.jsonl has session id `implement` — `loop sessions` lists every recorded id
`implement` is a state, not a session id: `loop sessions implement` lists its attempts, and `loop session --latest implement` opens the newest
```

### Launch

```
opening <TICKET>  <state> — cycle N, attempt M — <ts> — <outcome> — <summary, 72 chars>
```

One line on stdout, before the terminal is handed over. You typed an opaque id; this is loop telling you which attempt it was.

If the chosen attempt is not `finished`, stderr gets:

```
warning: no worker_output for this attempt — the session may still be active, or the spawn crashed
warning: the ledger recorded: <error detail, 120 chars>
```

The second line only when the episode recorded an error. Both on stderr, so a piped stdout stays clean, and the attempt still opens — a crashed attempt's transcript is exactly what you came for.

Then, with stdin/stdout/stderr inherited unchanged and the cwd set to the project directory:

```
<pi_bin> --session <id>
```

`--session`, not `--session-id`. This command exists to read history, so a session pi no longer has must fail loudly; `--session-id` would create an empty replacement under the same id, which is indistinguishable from a Worker that did nothing. The pi binary comes from `core::pi_bin()`, so `LOOP_PI_BIN` is the only thing that can change it.

loop never reads, parses, writes, or deletes a pi session file, and never copies a transcript into `.loop/`. For the session format and pi's own navigation controls, see pi's upstream session documentation.

### Errors

| Condition | Message |
| --- | --- |
| no id and no `--latest` | `` `loop session` no longer opens a picker — run `loop sessions` … `` |
| unknown id | ``no attempt in <ledger> has session id `X` — `loop sessions` lists every recorded id`` (plus the state hint when `X` names a state) |
| `--latest` with no candidate | ``no Worker session in <ledger> matching <state `X`\|any state> — sessions come from `state_entered.session_id`; `loop status` shows what the ledger holds`` |
| pi could not be spawned | ``launching `<pi_bin> --session <id>` — install pi, or set LOOP_PI_BIN`` |
| pi exited non-zero | `` `<pi_bin> --session <id>` exited <code\|on a signal> `` |

pi's exit status is the command's exit status: a non-zero pi is a non-zero `loop session`.

## `loop doctor`

```
loop doctor
```

> Check the environment: pi on PATH, machine present.

No flags.

Two existence checks, no parsing. Output per check:

```
  ok    {label}
  FAIL  {label} — {hint}
```

| # | Label | Hint on failure |
| --- | --- | --- |
| 1 | `` `{pi_bin}` on PATH `` | `install pi, or set LOOP_PI_BIN` |
| 2 | `{project}/.loop/machine.fnl` | ``run `loop init <TICKET>` in this project`` |

Every label is the path actually tested, so under `-C` you can read off where loop is looking.

`{pi_bin}` is `LOOP_PI_BIN` or `pi`. A `pi_bin` containing `/` is tested as a path; otherwise each `PATH` entry is probed for a file of that name.

Both pass → blank line, then `all good`, exit 0. Otherwise exit 1 with `error: {n} problem(s)` on stderr.

### What doctor does not check

- It never parses `machine.fnl` — check 2 only asks whether the file exists. Use [`loop validate`](#loop-validate) for the graph.
- It never resolves stage prompts or skills, and there is no second directory for it to look in.

## Environment variables

| Variable | Used for | Default / fallback |
| --- | --- | --- |
| `LOOP_PI_BIN` | the pi executable to spawn, for stages and for [`loop session`](#loop-session) | `pi` |
| `HOME` | expanding a leading `~/` in [`loop init --from`](#loop-init) | must be non-empty to count; otherwise the `~/` is left literal |
| `PATH` | resolving `pi_bin` in `loop doctor` and at spawn time | — |

That is the whole list. There is no `LOOP_CONFIG_DIR`, no `LOOP_STATE_DIR`, and no XDG lookup, because there is no root to point them at: everything loop reads or writes is under `<project>/.loop/`, and the project root comes from `--dir`/`-C`, else the cwd.

**`LOOP_PI_BIN` is the only lever on the binary.** No machine key sets it — a machine describes a ticket, not the harness running it.

## Exit codes

| Code | Meaning                                                       |
| ---- | ------------------------------------------------------------- |
| 0    | Success.                                                      |
| 1    | Runtime or application error.                                 |
| 2    | Command-line usage error reported by clap.                    |
| 141  | Killed by `SIGPIPE` — the thing reading its stdout went away. |

Runtime errors are caught by `main`, which prints `error: {e:#}` to stderr and exits 1. Clap handles invalid commands, flags, and arguments before `main` and exits 2. The `{e:#}` form prints the full context chain, so a runtime failure surfaces as `error: outer: inner: root cause`.

**A closed pipe is not an error.** `main` restores `SIGPIPE` to its default disposition, which Rust's runtime otherwise ignores, so `loop sessions | head -3` ends the way `cat` or `grep` would: the writer is killed by the signal, silently, and a shell reports `128 + 13 = 141`. Nothing is printed to stderr, and the code is deliberately not 0 — `loop run` reserves that for a `Done` outcome, and a run whose stdout disappeared has not finished a ticket. Such a run stays resumable from its ledger, the same as any other death mid-stage.

Per command:

| Command | Exit 0 | Exit 1 |
| --- | --- | --- |
| `init` | scaffold written | `.loop/machine.fnl` already exists; `--from` names a non-directory or one with no `machine.fnl`; any write fails |
| `validate` | no diagnostics, **or warnings only** | one or more errors (`{n} error(s)`); machine missing or fails to load |
| `preview` | report printed with no errors, **or warnings only** | one or more validation errors; unknown `<STATE>`; machine missing or fails to load |
| `diagram` | mermaid printed | machine missing or fails to load |
| `run` | outcome `Done` | outcome `Failed` or `Aborted`; ledger already has a run; machine missing or fails to load |
| `resume` | outcome `Done` | as `run`, plus an empty ledger |
| `status` | always, including the empty-ledger message | ledger unreadable or has a corrupt interior line |
| `recap` | a report was printed, **whatever the run's outcome was** | empty ledger; ledger unreadable or has a corrupt interior line |
| `logs` | human tail, or complete JSONL with `--raw` | ledger unreadable or has a corrupt interior line |
| `sessions` | one line per listed attempt | no usable candidate |
| `session` | pi exited 0 | no id and no `--latest`; unknown id; `--latest` with no usable candidate; pi could not be spawned; pi exited non-zero |
| `doctor` | both checks pass | `{n} problem(s)` |

`loop run` and `loop resume` exit 1 on `Failed` and `Aborted` deliberately, so `loop run && gh pr merge` and CI wrappers gate correctly. To distinguish the two, read `status` from [`loop status --json`](#loop-status).
