# CLI reference

Twelve subcommands: `init`, `validate`, `preview`, `diagram`, `run`, `resume`, `status`, `recap`, `logs`, `sessions`, `session`, `doctor`.

## Global flags

| Flag                      | Default           | Meaning            |
| ------------------------- | ----------------- | ------------------ |
| `--dir <DIR>`, `-C <DIR>` | current directory | Project directory. |
| `--version`               | —                 | clap-generated.    |
| `--help`, `-h`            | —                 | clap-generated.    |

`--dir` may appear before or after the subcommand. It anchors `.loop/`, the ledger, artifacts, and the cwd of every spawned process.

## Install

```
cargo build --release -p loop     # binary lands at target/release/loop
```

Rust 1.85+ (edition 2024). `pi` must be on `PATH` to _run_ a machine — without it you can still scaffold, validate, preview, and diagram. There is no daemon, server, or database; the whole run lives in files under the project.

---

## `loop init`

```
loop init <TICKET> [--from <DIR>]
```

Scaffolds `<project>/.loop/`. Every file is **write-if-absent** — nothing existing is ever overwritten. Bails first if the machine exists:

```
<project>/.loop/machine.fnl already exists — delete .loop/ to start a new ticket
```

**Without `--from`**, the bundled templates are written out of the binary — no fetch, nothing read from outside the project:

| Path                         | From                          |
| ---------------------------- | ----------------------------- |
| `machine.fnl`                | the bundled `standard-ticket` |
| `stage-prompts/implement.md` | bundled                       |
| `stage-prompts/review.md`    | bundled                       |
| `stage-prompts/qa.md`        | bundled                       |
| `stage-prompts/open-pr.md`   | bundled                       |
| `skills/debug-transient.md`  | bundled                       |

**With `--from <DIR>`**, the tree under `<DIR>` is copied in recursively instead, never overwriting, with file modes preserved (a copied `skills/` tree keeps its `.sh` executables). A leading `~/` is expanded. Three top-level names are **skipped**, because they are what a _run_ leaves behind: `ledger.jsonl`, `run/`, `artifacts/`. Failures specific to this path:

```
--from <DIR> is not a directory
<DIR> has no machine.fnl — --from wants a directory shaped like .loop/
```

Either way, `init` then writes `task.md`, `plan.md` (bundled templates, if absent), creates `stage-prompts/` and `skills/`, and writes `.gitignore` holding one line, `run/`. Each file actually created prints `  created <path>`.

`machine.fnl`, `task.md`, and `plan.md` get the ticket id stamped on them — a literal text substitution, **not** the `$VAR` render engine. A bundled template still contains the literal `$TICKET`, which is replaced; a `--from` source produced by an earlier `loop init` does not, so the **value** of the first `:ticket` key is rewritten in place. Under `--from`, only `machine.fnl` is rewritten.

`init` does **not** create `artifacts/`, `ledger.jsonl`, or `run/` — those appear when something needs them.

## `loop validate`

```
loop validate
```

No flags. Loads the machine, resolves every stage prompt and skill inside `.loop/`, prints one line per diagnostic as `{tag}  {where}: {message}` where `tag` is `error` or `warn ` (trailing space, for alignment).

| Sev | Where | Message |
| --- | --- | --- |
| error | `machine` | entry state `{e}` is not a defined state |
| error | `{from} -> {to}` | transition `from` `{f}` names no state or terminal |
| error | `{from} -> {to}` | transition `to` `{t}` names no state or terminal |
| error | state id | state `{id}` is unreachable from entry `{e}` |
| error | state id | state `{id}` has no path to any terminal |
| error | state id | stage prompt for state `{id}` does not resolve in .loop/stage-prompts/ |
| error | state id | skill `{n}` on state `{id}` does not resolve in .loop/skills/ — with ``(from `:defaults {:skills ..}`)`` when the name came from there |
| error | state id | state `{id}` names MCP servers, but `mcp` is not in `:pi-extensions` |
| error | loop name | loop `{n}` declares no states |
| error | loop name | loop `{n}` references unknown state `{s}` |
| error | loop name | loop `{n}`'s head `{h}` is never re-entered by any transition |
| error | `machine` | escalation_state `{e}` names no state or terminal |
| error | `{from}` | duplicate transition `{f}` → `{t}`: only the first is ever taken — merge them into one edge |
| warn | `{from}` | transition `{f}` → `{t}` has neither `check` nor `criteria`: the worker's proposal is committed unexamined |

Clean run: `{ticket} — {N} states, {M} transitions, no problems found`.

- Any error → exit 1 with `error: {n} error(s)` on stderr.
- **Warnings alone exit 0**, but suppress the "no problems found" line.
- Reachability walks only edges into _defined states_, from `entry`; skipped entirely when `entry` is undefined.
- MCP server names are never checked — loop never reads `~/.pi/agent/mcp.json`.
- Skills and MCP are checked as the **effective union** (`:defaults` plus the state's own), because that union is what a spawn loads.
- **Terminal-reachability ignores `on_fail: route` edges**, so a state whose only way forward is a guard-failure route reads as having no path to a terminal.

## `loop preview`

```
loop preview [<STATE>]
```

Answers "what will this loop do?" using the run's own resolvers — the four-layer model merge, stage prompt and skill resolution, the effective unions, `$VAR` substitution — stopped short of every write.

**Performs no side effect.** It does not spawn pi, run a `:check`, connect to an MCP server, create the ledger or `artifacts/`, or write anything under `run/`. Output is deterministic.

Whole-machine sections, in order:

| Section | Contents |
| --- | --- |
| header | ticket, state / transition / loop counts |
| — | source path, entry, terminals, escalation state, effective budgets, Judge and Navigator models with the invocation cap |
| `context` | task and plan line/char counts with their first line, and the QA case ids |
| `states` | per state: description, resolved stage prompt name and path, resolved `provider/model:thinking`, effective skills with resolved paths, effective MCP names, reachable states, then each outgoing edge with its check command, timeout, criteria, `:on-fail`, `:max-attempts` (only on retry edges, where it can bite), and backoff |
| `loops` | each loop's head, member states, `:max-cycles`, exhaustion behavior |
| `validation` | the diagnostics, or `no problems found` |

An absent optional value or empty list reads `(none)`.

`loop preview <STATE>` prints that state's block, then adds: how the state names its stage prompt and the file it resolved to; the frontmatter as parsed; the `worker invocation` (the `--model` flag, provider, each `--skill` path, MCP names, reachable states, cwd, the rendered-prompt path pattern, the four exported environment variable **names**, the deterministic session id); `template variables` (the `$NAME`s the body writes that are loop variables, split from the ones that pass through untouched); the body as authored; and a **representative render**.

**The representative render is not the future prompt.** It is built with cycle 1, attempt 1, no previous state, no artifacts, and an empty digest. What it establishes exactly is _which_ variables the stage prompt interpolates.

Preview runs the full `validate` linter — same function, same diagnostics — and prints the report **first**, then the diagnostics, so problems are the last thing on screen.

- Any validation **error** → full report prints, then exit 1 with `error: {n} error(s) — this machine will not run as previewed`.
- **Warnings alone exit 0.**
- An unknown `<STATE>` exits 1 **before printing anything**, listing the states that do exist. A terminal is not a state, so `loop preview done` is this error.

## `loop diagram`

```
loop diagram > machine.mmd
```

No flags. Mermaid to stdout with no code fences and no prose. A pure function of the machine IR — it reads nothing off disk but the machine file, so a machine with a dangling `:stage-prompt` still draws.

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

- A node gets a `state "<label>" as <alias>` line only when the label differs from the alias — the alias sanitizes non-alphanumerics to `_`, which is why `open-pr` gets one and `implement` does not.
- The escalation state's label gets `" (escalation)"` appended.
- Solid edges are declared transitions, **in declaration order**. Back-edges from `:on-fail {:route …}` are grouped under the comment, one per distinct pair, sorted.
- `note right of <head>` blocks describe each loop.

**Edge-label grammar** — a comma-joined list built in this order:

| Fragment        | Appears when              |
| --------------- | ------------------------- |
| `check`         | the edge has a `:check`   |
| `judge`         | the edge has `:criteria`  |
| `unguarded`     | the edge has neither      |
| `wait {N}s`     | the edge has `:backoff-s` |
| `abort on fail` | `:on-fail` is `"abort"`   |

`retry` never appears — it is the default and would be noise on every edge. A route gets its own arrow rather than a label fragment.

## `loop run` / `loop resume`

```
loop run    [--max-transitions <N>] [-v|--verbose]
loop resume [--max-transitions <N>] [-v|--verbose]
```

`--max-transitions` **only tightens**: merged with the effective budget by per-field minimum, so a value larger than the machine's own has no effect. `--verbose` echoes each pi spawn's stderr as it runs.

Identical in every respect except the ledger precondition, which is inverted:

```
<project>/.loop/ledger.jsonl already has a run — use `loop resume`, or delete it to start over
nothing to resume: <project>/.loop/ledger.jsonl is empty
```

Missing machine (shared by every command that loads the graph):

```
no machine at <project>/.loop/machine.fnl — run `loop init <TICKET>` first
```

**Final line**, printed to stdout after a blank line for every outcome including failures:

```
{Status} — {terminal} after {n} transition(s), ${cost}, {t} token(s), {duration}
```

`{Status}` is `Done`, `Failed`, or `Aborted`; `{terminal}` falls back to `(no terminal)`; `{t}` is every token every role burned. Duration: `{s}s` under a minute, `{m}m{s}s` under an hour, else `{h}h{m}m`.

| Status | Exit | stderr |
| --- | --- | --- |
| `Done` | 0 | — |
| `Failed` | 1 | ``error: run ended at `{state}` without completing — see `loop status` `` |
| `Aborted` | 1 | ``error: run aborted — see `loop status` for the guardrail`` |

The summary line prints _before_ the error, so a failed run emits both. `wallclock-s` bounds the run, not the process — resuming does not buy a fresh time budget.

There are exactly three outcomes. **Done** — reached a terminal normally. **Failed** — reached the escalation terminal, or an edge with `:on-fail "abort"` failed its guard; the machine ran correctly, the work did not converge. **Aborted** — a guardrail stopped the run: a budget breach, or an escalation with nowhere to escalate to.

## `loop status`

```
loop status [--json]
```

Folds the ledger. Does **not** require the machine to load — it only loses the `cycles:` line. Empty ledger, both modes: ``no run yet — `loop run` starts one`` (human mode only; `--json` still emits its five keys with nulls and zeroes).

```
unfinished — last at `review`
  5 transition(s), $1.23, 38104 token(s), 12m3s
  cycles: implement#2, qa#1

recent:
  <ts>  <summary>
```

The header is `not started`, ``unfinished — last at `{state}` ``, or ``finished — {Status} at `{state}` ``. It says "unfinished" rather than "running" because status reads a ledger and nothing else: a run whose process died an hour ago is indistinguishable here from one still working.

`recent:` lists the last 12 events, oldest-first within that window. Summary grammar (shared with `loop logs`):

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

`--json` emits exactly five keys, alphabetically ordered:

```json
{
  "current": "done",
  "cycles": { "qa-staging": 3 },
  "navigator_invocations": 0,
  "status": "done",
  "totals": {
    "cost_usd": 3.58,
    "tokens": 0,
    "transitions": 10,
    "wallclock_s": 3414
  }
}
```

`status` is `null` while the run is in progress; `totals.wallclock_s` is live and accumulates across resumes. No resume point, no artifact list, no ticket id in JSON mode.

## `loop recap`

```
loop recap
loop recap > run-recap.md
```

No flags. The post-run counterpart to `preview`: that command explains the declaration, this one explains the observed execution. Markdown to stdout.

**Deterministic** — no LLM, no clock, no network, nothing written. The same ledger renders byte-identical output every time, which is the property that makes it usable as evidence.

Four sections, always in this order:

1. **Run summary** — start timestamp, the budgets recorded on `run_started`, the machine hash and whether the machine on disk still matches it, outcome, totals, cycle counts, Navigator invocations, attempt count.
2. **Attempt timeline** — one `###` per `state_entered`, in ledger order: entered timestamp and elapsed, `model:thinking`, skills, MCP, session id, then the episode's events — Worker summary, usage and artifacts; the proposal and its rationale; each guard tier's outcome with the **full** check output and Judge rationale; the Navigator's choice; the committed move; any errors and notes.
3. **Why it ended** — the terminal transition and totals, plus the last fatal `error` when the status is not `Done`. For an unfinished run, the folded resume point and last durable event instead.
4. **Inspecting further** — a `loop sessions <state>` line per state with a reopenable attempt, and the `loop logs --raw | jq` recipes.

**Evidence labels.** Claims are attributed to whoever made them, because the three sources are not equally trustworthy and the interesting runs are the ones where they disagree:

| Label | Who authored it | What it proves |
| --- | --- | --- |
| `**Worker**` | the Worker, from `worker_output.summary` | nothing on its own |
| `**Proposal**` | the Worker (or the Navigator, when it routed) | what was asked for, not what was granted |
| `**Check**` | the harness, running the edge's `:cmd` in its own process | a real signal |
| `**Judge**` | an independent tool-less spawn | a second opinion on the criteria |
| `**Committed**` | the harness | what actually happened |

Multi-line evidence is reproduced in full, inside a fence grown past any backtick run in the text. Nothing in the timeline is truncated.

**Partial runs** are reported to date; attempts that produced nothing still get their section. An **empty ledger is an error**, not an empty report.

**The machine is optional and only sometimes trusted.** It is loaded opportunistically and used only when its hash still equals the `machine_hash` on `run_started`. Otherwise the recap says so, drops state descriptions, folds machine-agnostically (so `cycles` counts re-entries of every state), and on a hash mismatch writes a warning to **stderr** — so `loop recap > run-recap.md` still produces a clean file.

A recap of a `Failed` or `Aborted` run still exits 0. Exit 1 is reserved for an empty, unreadable, or interior-corrupt ledger.

## `loop logs`

```
loop logs [-n <N>]
loop logs --raw
```

| Flag     | Default | Meaning                                     |
| -------- | ------- | ------------------------------------------- |
| `-n <N>` | `20`    | Events in the human-readable tail.          |
| `--raw`  | false   | Emit the complete repaired ledger as JSONL. |

Does not load `machine.fnl`. Human mode prints one event per line, oldest first within the tail, as `<timestamp>  <status summary>` using the same grammar as `status`.

`--raw` writes the entire repaired ledger to stdout as JSONL — no heading, reformatting, or filtering — and is the path-independent replacement for reading `.loop/ledger.jsonl` directly. An empty ledger writes zero bytes. `--raw` with an explicit `-n` is a usage error (exit 2).

Three `jq` recipes that earn their keep:

```sh
# What actually happened, in order
loop logs --raw | jq -r 'select(.type=="transition_committed")
       | "\(.ts)  cycle \(.cycle)  \(.from) -> \(.to)"'

# Why a guard failed — the failing tier, the check output, the Judge's rationale
loop logs --raw | jq -r 'select(.type=="guard_checked" and (.check=="fail" or .criteria=="fail"))
       | "=== \(.from) -> \(.to)  check=\(.check) criteria=\(.criteria)",
         (.check_output // "(no check output)"),
         (.judge_rationale // "(no judge rationale)")'

# Where the money went, per state (Workers only)
loop logs --raw | jq -s 'map(select(.type=="worker_output"))
       | group_by(.state)
       | map({state: .[0].state, spawns: length, cost: (map(.usage.cost_usd) | add)})'
```

## `loop sessions` and `loop session`

```
loop sessions                          # every Worker attempt, oldest first
loop sessions implement                # only attempts at that exact state
loop session PROJ-1-implement-1-2      # reopen that attempt
loop session --latest implement        # newest implement attempt, no id needed
loop session --latest                  # newest Worker attempt
```

`sessions` finds the transcript; `session` opens it. Both read nothing but the ledger and require **neither a loadable machine nor a resolvable stage prompt** — a mid-edit `machine.fnl` is often exactly when you want this. Judge and Navigator spawns run `--no-session` and never appear.

One candidate per `state_entered` with a non-empty `session_id`, in ledger order:

```
2026-07-26T12:01  implement        1  1  crashed     PROJ-9-implement-1-1        error: executor lost
2026-07-26T12:03  implement        1  2  finished    PROJ-9-implement-1-2        Added the retry guard and updated the tests.
2026-07-26T12:05  review-the-diff  1  1  incomplete  PROJ-9-review-the-diff-1-1
```

Timestamp, state, cycle, attempt, outcome, session id, evidence. **Fields 1–6 are single whitespace-free tokens in every row**, so `awk '{print $6}'` is always the session id; the evidence column is last because it is the only one that can contain spaces (collapsed to one line, truncated to 72 chars). No header line.

| Outcome      | Means                                              |
| ------------ | -------------------------------------------------- |
| `finished`   | a matching `worker_output` landed in the episode   |
| `crashed`    | no `worker_output`, but an `error` in the episode  |
| `incomplete` | neither — still running, or killed without a trace |

The listing is a pipeline, not a menu — the shell is the picker:

```sh
loop sessions | fzf | awk '{print $6}' | xargs loop session
loop sessions implement | grep crashed
loop sessions | awk '$5=="incomplete"'
```

`<STATE>` is an **exact** filter: `implement` never matches `implement-hotfix`. No usable candidate is an error on stderr with exit 1, not an empty success.

`loop session` resolves the attempt, prints one line naming it, then executes `pi --session <id>` in the project directory with stdin/stdout/stderr inherited. Three things it will not do quietly:

- **Guess which attempt you meant.** Without `--latest` it needs an id. There is no picker any more; `loop session implement` errors with the two commands that work.
- **Hide a missing session.** It passes `--session`, not `--session-id`, so a session pi no longer holds is an error — `--session-id` would create an empty replacement that looks identical to a Worker that did nothing.
- **Pretend an attempt finished.** An attempt with no `worker_output` still opens — that is precisely the transcript you want after a crash — but it warns on stderr first.

pi's exit status is the command's exit status. loop never reads, parses, writes, or deletes a pi session file.

## `loop doctor`

```
loop doctor
```

Two existence checks, no parsing:

```
  ok    `pi` on PATH
  ok    /home/you/src/yourrepo/.loop/machine.fnl

all good
```

| # | Label | Hint on failure |
| --- | --- | --- |
| 1 | `` `{pi_bin}` on PATH `` | `install pi, or set LOOP_PI_BIN` |
| 2 | `{project}/.loop/machine.fnl` | ``run `loop init <TICKET>` in this project`` |

Every label is the path actually tested. Both pass → blank line, `all good`, exit 0. Otherwise exit 1 with `error: {n} problem(s)`.

It **never parses `machine.fnl`** — check 2 only asks whether the file exists. Use `loop validate` for the graph.

## Environment variables

| Variable | Used for | Default / fallback |
| --- | --- | --- |
| `LOOP_PI_BIN` | the pi executable to spawn, for stages and for `loop session` | `pi` |
| `HOME` | expanding a leading `~/` in `loop init --from` | must be non-empty to count |
| `PATH` | resolving `pi_bin` in `doctor` and at spawn time | — |

That is the whole list. There is no `LOOP_CONFIG_DIR`, no `LOOP_STATE_DIR`, and no XDG lookup, because there is no root to point them at. **`LOOP_PI_BIN` is the only lever on the binary** — no machine key sets it.

## Exit codes

| Code | Meaning                                                   |
| ---- | --------------------------------------------------------- |
| 0    | Success.                                                  |
| 1    | Runtime or application error.                             |
| 2    | Command-line usage error reported by clap.                |
| 141  | Killed by `SIGPIPE` — the thing reading stdout went away. |

Runtime errors print `error: {e:#}` to stderr — the full context chain, so a failure surfaces as `error: outer: inner: root cause`.

Per command:

| Command | Exit 0 | Exit 1 |
| --- | --- | --- |
| `init` | scaffold written | machine already exists; `--from` names a non-directory or one with no `machine.fnl`; any write fails |
| `validate` | no diagnostics, **or warnings only** | one or more errors; machine missing or fails to load |
| `preview` | report printed with no errors, **or warnings only** | validation errors; unknown `<STATE>`; machine missing or fails to load |
| `diagram` | mermaid printed | machine missing or fails to load |
| `run` | outcome `Done` | `Failed` or `Aborted`; ledger already has a run; machine missing or fails to load |
| `resume` | outcome `Done` | as `run`, plus an empty ledger |
| `status` | always, including the empty-ledger message | ledger unreadable or interior-corrupt |
| `recap` | a report was printed, **whatever the outcome** | empty ledger; unreadable or interior-corrupt |
| `logs` | human tail, or complete JSONL | ledger unreadable or interior-corrupt |
| `sessions` | one line per listed attempt | no usable candidate |
| `session` | pi exited 0 | no id and no `--latest`; unknown id; `--latest` with no candidate; pi could not be spawned; pi exited non-zero |
| `doctor` | both checks pass | `{n} problem(s)` |

`run` and `resume` exit 1 on `Failed` and `Aborted` deliberately, so `loop run && gh pr merge` and CI wrappers gate correctly. To distinguish the two, read `status` from `loop status --json`.
