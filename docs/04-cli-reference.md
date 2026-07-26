# CLI reference

`loop` — "A local, ticket-level agent orchestrator".

Eight subcommands: [`init`](#loop-init), [`validate`](#loop-validate), [`diagram`](#loop-diagram), [`run`](#loop-run), [`resume`](#loop-resume), [`status`](#loop-status), [`logs`](#loop-logs), [`doctor`](#loop-doctor).

For what the runtime actually does with the machine, see [02-how-it-works.md](02-how-it-works.md). For the keys inside `config.fnl` and `machine.fnl`, see [03-customizing.md](03-customizing.md).

## Global flags

| Flag | Type | Default | Meaning |
| --- | --- | --- | --- |
| `--dir <DIR>`, `-C <DIR>` | path | current directory | "Project directory (default: the current directory)." |
| `--version` | — | — | clap-generated version. |
| `--help`, `-h` | — | — | clap-generated help. |

`--dir` is global: it may appear before or after the subcommand. It sets `project_dir`, which anchors `.loop/`, the ledger, artifacts, and the cwd of every spawned process.

## `loop init`

```
loop init <TICKET> [--template <TEMPLATE>]
```

> Scaffold ./.loop/ from a machine template, and ~/.config/loop/ on first use.

| Flag / arg | Type | Default | Meaning |
| --- | --- | --- | --- |
| `<TICKET>` | string, required | — | "Ticket id, e.g. PROJ-1487." |
| `--template <TEMPLATE>` | string | `standard-ticket` | "Machine template from ~/.config/loop/machines/." |

Runs two scaffold phases: the global toolbox (idempotent, safe to re-run), then the project's `.loop/`. Every file is written with a _write-if-absent_ rule — **nothing existing is ever overwritten**, with one exception noted below.

**Phase 1 — toolbox** (`<config_dir>/`), skipped file by file if already present:

| Path                           | Note                 |
| ------------------------------ | -------------------- |
| `config.fnl`                   | global defaults      |
| `machines/standard-ticket.fnl` | the default template |
| `playbooks/implement.md`       |                      |
| `playbooks/review.md`          |                      |
| `playbooks/qa.md`              |                      |
| `playbooks/open-pr.md`         |                      |
| `playbooks/debug-transient.md` |                      |
| `skills/`                      | created empty        |
| `ext/transition-tool.ts`       | see exception        |
| `ext/verdict-tool.ts`          | see exception        |
| `ext/choose-tool.ts`           | see exception        |

Each file actually created prints `  created <path>`.

**Exception to write-if-absent:** the three `ext/*.ts` are written by `materialize_ext()`, which compares the on-disk sha256 against the copy compiled into the binary and **rewrites the file on mismatch**. Hand edits to `ext/transition-tool.ts`, `ext/verdict-tool.ts`, or `ext/choose-tool.ts` are silently reverted — by `loop init` and also by every `loop run` / `loop resume`.

**Phase 2 — project** (`<project>/.loop/`). Bails first if the machine already exists:

```
<project>/.loop/machine.fnl already exists — delete .loop/ to start a new ticket
```

Otherwise reads `<config_dir>/machines/<template>.fnl` (missing → `no machine template at <path>`) and writes:

| Path          | From                      |
| ------------- | ------------------------- |
| `machine.fnl` | the template              |
| `task.md`     | the bundled task template |
| `plan.md`     | the bundled plan template |
| `playbooks/`  | created empty             |

All three files get a plain `str::replace("$TICKET", <ticket>)` — a literal text substitution, **not** the `$VAR` render engine used at runtime. No other placeholder is expanded at init time.

`init` does **not** create `skills/`, `artifacts/`, or `ledger.jsonl`.

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

Loads `config.fnl` and `machine.fnl`, resolves every playbook and skill against the toolbox, and prints one line per diagnostic in the form:

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
| error | state id | playbook for state `{id}` does not resolve in the toolbox |
| error | state id | skill `{n}` on state `{id}` does not resolve in the toolbox |
| error | state id | state `{id}` names MCP servers, but `mcp` is not in `:pi-extensions` — the stage would be told to call a tool it does not have |
| error | loop name | loop `{n}` declares no states |
| error | loop name | loop `{n}` references unknown state `{s}` |
| error | loop name | loop `{n}`'s head `{h}` is never re-entered by any transition |
| error | `machine` | escalation_state `{e}` is not a declared terminal |
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

Skills and MCP servers are checked as the **effective union** — `config.fnl`'s `:default-skills` / `:default-mcp`, the machine's `:defaults`, and the state's own — because that union is what a spawn loads. A diagnostic for a name that came from the global config says so.

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

Output is deterministic and a pure function of the machine IR — it touches no toolbox files, so a machine with a dangling `:playbook` or an unresolvable skill still draws. It still requires the machine to load: a missing or unparseable `machine.fnl` is an error.

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

Loads config + machine, materializes `ext/*.ts`, opens the ledger, and steps the engine until the run reaches a terminal or trips a guardrail.

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

{Status} — {terminal} after {n} transitions, ${cost}, {duration}
```

`{Status}` is `Done`, `Failed`, or `Aborted`. `{terminal}` falls back to `(no terminal)`. Cost is two decimal places. Duration formatting:

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
running — at `review`
  5 transitions, $1.23, 12m3s
  cycles: implement#2, qa#1

recent:
  <ts>  <summary>
```

Header is one of `not started`, ``running — at `{state}` ``, or `finished — {Status}`. The `cycles:` line appears only when the machine loaded and at least one cycle was counted. `recent:` lists the last 12 ledger events, oldest-first within that window. Per-event summary forms:

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

`logs` opens the ledger through the normal `Ledger` reader, so a torn trailing write is repaired before anything is printed. It does not load `machine.fnl` or `config.fnl`; the ledger is its only source, so it works while the machine is missing or invalid.

Human mode prints one event per line, oldest first within the selected tail, as `<timestamp>  <status summary>`. It has no `recent:` wrapper and defaults to the last 20 events. If fewer than `N` events exist, it prints all of them. An empty ledger prints:

```
no run yet — `loop run` starts one
```

`--raw` ignores the human tail and writes the entire repaired ledger to stdout exactly as JSONL: no heading, status message, reformatting, or filtering. An empty ledger writes zero bytes. `--raw` conflicts with an explicitly supplied `-n`, so a command such as `loop logs --raw -n 50` is a command-line usage error and exits 2 without output.

Valid ledger content goes only to stdout. A ledger read or parse failure is reported to stderr and exits 1; successful commands exit 0.

## `loop doctor`

```
loop doctor
```

> Check the environment: pi on PATH, toolbox staged, machine present.

No flags.

Four existence checks, no parsing. Output per check:

```
  ok    {label}
  FAIL  {label} — {hint}
```

| # | Label | Hint on failure |
| --- | --- | --- |
| 1 | `` `{pi_bin}` on PATH `` | `install pi, or set LOOP_PI_BIN` |
| 2 | `{config_dir}/config.fnl` | ``run `loop init <TICKET>` to scaffold the toolbox`` |
| 3 | `{config_dir}/ext/transition-tool.ts` | ``run `loop init` to write the vendored ext`` |
| 4 | `{project}/.loop/machine.fnl` | ``run `loop init <TICKET>` in this project`` |

Every label is the path actually tested, so under `LOOP_CONFIG_DIR` or `-C` you can read off where loop is looking.

`{pi_bin}` is `LOOP_PI_BIN` or `pi`. A `pi_bin` containing `/` is tested as a path; otherwise each `PATH` entry is probed for a file of that name.

All four pass → blank line, then `all good`, exit 0. Otherwise exit 1 with `error: {n} problem(s)` on stderr.

### What doctor does not check

- It never loads or evaluates `config.fnl` — check 2 is a bare file-existence test. A file that exists but does not parse passes doctor and fails everything else; `loop validate` is what reads it.
- It never parses `machine.fnl` — check 4 only asks whether the file exists. Use [`loop validate`](#loop-validate) for the graph.
- It never resolves playbooks or skills.
- Check 3 probes **only `ext/transition-tool.ts`**. A missing or corrupt `verdict-tool.ts` or `choose-tool.ts` still reports `ok`.

## Environment variables

| Variable | Used for | Default / fallback chain |
| --- | --- | --- |
| `LOOP_CONFIG_DIR` | toolbox root (`config.fnl`, `playbooks/`, `skills/`, `machines/`, `ext/`) | `$XDG_CONFIG_HOME/loop` → `$HOME/.config/loop` → relative `.config/loop` |
| `LOOP_STATE_DIR` | generated state root (`render/`) | `$XDG_STATE_HOME/loop` → `$HOME/.local/state/loop` → relative `.local/state/loop` |
| `LOOP_PI_BIN` | the pi executable to spawn | `pi` |
| `HOME` | fallback base for both roots | must be non-empty to count |
| `XDG_CONFIG_HOME` | config-root fallback | **ignored unless absolute** |
| `XDG_STATE_HOME` | state-root fallback | **ignored unless absolute** |
| `PATH` | resolving `pi_bin` in `loop doctor` and at spawn time | — |

Precedence for each root, first match wins:

1. `LOOP_CONFIG_DIR` / `LOOP_STATE_DIR`, taken verbatim (no expansion, no absoluteness requirement).
2. `$XDG_CONFIG_HOME/loop` / `$XDG_STATE_HOME/loop`, **only if the XDG value is an absolute path**; a relative value is ignored entirely.
3. `$HOME/.config/loop` / `$HOME/.local/state/loop`, only if `$HOME` is set and non-empty.
4. The relative paths `.config/loop` / `.local/state/loop`, resolved against the process cwd.

The project root is not environment-driven: it is `--dir`/`-C`, else the cwd.

Two things to know:

- **`LOOP_PI_BIN` cannot be set from `config.fnl`.** Config loading passes the binary through untouched, so the environment variable is the only lever.
- **`LOOP_STATE_DIR` does not move the ledger.** The ledger is always `<project>/.loop/ledger.jsonl`. `LOOP_STATE_DIR` only relocates the rendered system prompts under `render/`.

## Exit codes

| Code | Meaning                                    |
| ---- | ------------------------------------------ |
| 0    | Success.                                   |
| 1    | Runtime or application error.              |
| 2    | Command-line usage error reported by clap. |

Runtime errors are caught by `main`, which prints `error: {e:#}` to stderr and exits 1. Clap handles invalid commands, flags, and arguments before `main` and exits 2. The `{e:#}` form prints the full context chain, so a runtime failure surfaces as `error: outer: inner: root cause`.

Per command:

| Command | Exit 0 | Exit 1 |
| --- | --- | --- |
| `init` | scaffold written | `.loop/machine.fnl` already exists; template missing; any write fails |
| `validate` | no diagnostics, **or warnings only** | one or more errors (`{n} error(s)`); machine missing or fails to load |
| `diagram` | mermaid printed | machine missing or fails to load |
| `run` | outcome `Done` | outcome `Failed` or `Aborted`; ledger already has a run; machine missing or fails to load |
| `resume` | outcome `Done` | as `run`, plus an empty ledger |
| `status` | always, including the empty-ledger message | ledger unreadable or has a corrupt interior line |
| `logs` | human tail, or complete JSONL with `--raw` | ledger unreadable or has a corrupt interior line |
| `doctor` | all four checks pass | `{n} problem(s)` |

`loop run` and `loop resume` exit 1 on `Failed` and `Aborted` deliberately, so `loop run && gh pr merge` and CI wrappers gate correctly. To distinguish the two, read `status` from [`loop status --json`](#loop-status).
