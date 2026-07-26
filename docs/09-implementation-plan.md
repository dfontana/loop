# 09 — v1 implementation plan

The design docs (01–08) are the *what*. This is the *how* for the first working
binary, and it records the four decisions that supersede the earlier docs.

## Decisions that supersede docs 01–08

| Topic | Docs 01–08 said | v1 does |
|---|---|---|
| Harness language | Node/TS, to match pi | **Rust** (`clap` CLI, `mlua` embedding Lua 5.4) |
| Machine surface | YAML first, Fennel later ([02](02-language.md)) | **Fennel only.** No YAML machine loader is written. |
| Fennel style | Full macro DSL (`(stage … (to … :when …))`) | **Plain table** returned by the module. Macros are a v2 layer over an unchanged IR. |
| Gating | `when` guards over "trusted" ledger vars scraped from tool stdout, plus `criteria` | **`:check`** — a command the harness runs itself, exit code decides — plus `criteria`. See below. |
| Stage tooling | `scoped-tools` YAML, bound per stage by a `--tools` allowlist | **pi skills** (`--no-skills` + `--skill <path>`), resolved local-first. No per-stage tool filtering. |
| Toolbox location | `~/.loop/` | **`~/.config/loop/`** (authored) + **`~/.local/state/loop/`** (generated) |

Cut from v1, deliberately: persistent sessions (`session: continue|fork`),
`--dry-run` cost estimation, `fork`/`join` parallelism, machine-authoring
(`loop plan`). Every stage runs in a fresh session; continuity is the ledger
digest + artifacts, exactly as [01](01-architecture.md) describes for the default
path.

Kept in v1, because they are load-bearing for correctness: the **Judge** and
**Navigator** agents ([07](07-risks.md) risk #1 — the worker must not grade its
own homework) and the **`:check`** tier ([03](03-ledger.md) — the only signal in
the system a worker cannot author).

### Why `when` guards and scoped-tools were cut

They were one idea: a tool asserts a fact from a real exit code, prints
`LOOP_VARS {…}`, the harness scrapes it into the ledger as *trusted*, and a
Fennel `when` closure gates on it. Wrapping the command in scoped-tools YAML was
what made the assertion trustworthy.

It did not hold up. The scrape ran over **every** tool result, with no filter on
which tool produced it — so any stage with `bash` could print the marker itself
and open its own gate. Claimed artifacts had the same problem: the worker names
the paths, the harness merely hashes them. Every signal reaching a guard had
passed through the worker's session.

The fix is not a better scrape. It is to have the **harness run the command
itself**, out of process, after the stage exits — which is what `:check` is.
Once gating no longer reads tool stdout, a scoped-tool's `validationCmd` and
hidden parameters only constrain blast radius, and only on a stage with no
`bash`. Every stage had `bash`, via the machine-wide default. So the wrapper was
paying real complexity for a guarantee it never delivered, and skills — which
the agent can read, and which carry their guardrails in a testable script —
cover the same ground more honestly.

### Why loop stopped modeling MCP

An earlier draft had loop own an `mcp.json`: a file in the toolbox, copied into
a generated agent dir, with `PI_AGENT_DIR` pointed at it per spawn.

That is a worse version of a file the user already has. The `mcp` extension
reads `~/.pi/agent/mcp.json` and `./.mcp.json`, and it holds OAuth state, bearer
tokens, and env-expanded headers — everything you would least like to maintain a
second copy of. Worse, `PI_AGENT_DIR` is a *redirect*, not an overlay: pointing
it at loop's directory doesn't add loop's servers to the user's, it replaces
them. A machine that named no server would silently take away every server the
user had configured.

So loop models the one thing that is genuinely its business — **which of your
servers a given stage should reach** — and nothing else. A state lists names in
`:mcp`; the harness sets no `PI_AGENT_DIR` and stages no file.

The mechanism for turning one on is dictated by the extension: servers default
to **off** every session, and the only way in is the `/mcp` panel (which does
not exist headless) or the agent calling `mcp({connect: "<name>"})`, which
enables and connects in one step. So the names go into the stage's entry
message as exactly those calls, ahead of the work.

This does mean the agent performs its own setup, and could in principle skip it
or connect something it wasn't given. That is the same bargain skills already
make: it bounds *reach*, not trust. The tier a worker cannot talk past is still
the edge's `:check`.

## Directory layout

```
~/.config/loop/                  # authored, git-able
  config.fnl                     # global defaults (was loop.config.yaml)
  playbooks/*.md                 # a stage's prompt
  skills/<name>/SKILL.md         # situational know-how + the scripts beside it
  machines/*.fnl                 # machine templates
  ext/{transition,verdict,choose}-tool.ts   # materialized from the binary

~/.local/state/loop/             # generated, disposable, safe to rm -rf
  render/<ticket>/               # rendered playbooks + entry messages per spawn

./.loop/                         # per-ticket, in the project repo
  machine.fnl  task.md  plan.md
  playbooks/*.md                 # local overrides (resolved local-first)
  ledger.jsonl                   # gitignored
  artifacts/
```

## Crate layout

A cargo workspace. The seams are chosen so five agents can build in parallel
against `loop-core` alone — no crate in wave 1 depends on another.

| Crate | Owns | Depends on |
|---|---|---|
| `loop-core` | The IR: `Machine`, `State`, `Transition`, `Check`, `Config`, `Event`, and the traits (`AgentRunner`, `CheckRunner`, `LedgerSink`, …) every other crate is written against. No I/O. | — |
| `loop-ledger` | JSONL append (fsync per event, tolerant of a trailing partial line), the fold to `RunState`, the artifact store (temp-file + atomic rename + sha256), the rolling digest. | core |
| `loop-fennel` | `mlua` + vendored `fennel.lua`; loads `config.fnl` / `machine.fnl` → `Machine`. The IR is plain data, so the VM is dropped once loading is done. | core |
| `loop-toolbox` | Playbook and skill resolution (local-first, then toolbox), `$UPPER_SNAKE` rendering over the context namespace, the entry message (including the per-stage `mcp({connect})` preamble), materializing the three `ext/*.ts` from `include_str!`. | core |
| `loop-runner` | Spawning `pi` (worker/judge/navigator), parsing the JSONL event stream, extracting the `transition` call, summing `usage.cost`; implements `AgentRunner`. Also `exec_check`, the harness's own bounded subprocess for a transition `:check`. | core |
| `loop-engine` | The control loop of [01](01-architecture.md): guard tiers, budgets, cycle caps, navigator caps, `on_fail` handling — plus `validate` (the static linter of [07](07-risks.md) #11) and `mermaid` (the same graph, drawn). Written against the traits, so it is fully testable with fakes. | core |
| `loop-cli` | `clap`: `init`, `validate`, `diagram`, `run`, `status`, `resume`, `doctor`. Wires the concrete impls into the engine. | all |
| `mock-pi` | Test fixture binary: replays a scripted JSONL stream (including crash-mid-stage) so the whole loop is testable deterministically, offline, for $0. | — |

## The two traits that make parallelism work

```rust
// loop-core
pub trait GuardEvaluator {
    fn eval(&self, guard: GuardRef, vars: &Vars) -> Result<bool, CoreError>;
}

pub trait AgentRunner {
    fn run_worker(&self, spec: &WorkerSpec) -> Result<WorkerResult, CoreError>;
    fn run_judge(&self, spec: &JudgeSpec) -> Result<Verdict, CoreError>;
    fn run_navigator(&self, spec: &NavigatorSpec) -> Result<Choice, CoreError>;
}
```

`GuardRef` is an opaque handle into `loop-fennel`'s Lua registry, so the engine
evaluates a Fennel guard without linking `mlua`. `AgentRunner` is what lets the
engine be tested against `mock-pi` and against pure in-process fakes.

## Task waves

**Wave 0 (me, base commit):** workspace scaffold, `loop-core` fully written (the
IR is the contract), every other crate stubbed with its public signatures and
`todo!()`, all dependencies pinned in `Cargo.lock`, vendored `fennel.lua`.

**Wave 1 (five agents in parallel, one jj workspace each):**

| Task | Crate | Verification gate |
|---|---|---|
| T1 | `loop-ledger` | round-trip every event type; fold fixtures incl. crash-resume cases; truncated-final-line tolerance; artifact hash/atomicity |
| T2 | `loop-fennel` | load the ported `machine.fnl`; guard closures evaluate over `vars`; error messages point at Fennel source; malformed-table rejection |
| T3 | `loop-toolbox` | local-over-toolbox resolution; `$UPPER_SNAKE` render incl. unknown-`$NAME` passthrough; YAML merge with project-over-global precedence; ext materialization |
| T4 | `loop-runner` + `mock-pi` | parse a scripted stream → `WorkerResult`; `LOOP_TRANSITION` extraction; cost summing; non-zero exit + malformed line handling; `exec_check` exit codes, timeout, output truncation |
| T5 | `loop-engine` | the full control loop against fakes: three guard tiers, `on_fail` retry/route/abort, cycle + navigator caps, budget aborts, `validate` catching each authoring error in [07](07-risks.md) #11 |

**Wave 2 (me):** merge the workspaces, `loop-cli` wiring, end-to-end integration
tests driving the real binary against `mock-pi`, port `examples/local/` and
`examples/toolbox/` to the Fennel/XDG shapes, update docs 02/04/05 to match.

Every task ends green on `cargo fmt --check`, `cargo clippy -- -D warnings`, and
`cargo test`; wave 2 additionally requires the end-to-end run to reach `done`.

## What integration cost — worth reading before the next wave

All five wave-1 crates passed their own suites. Wiring them behind one binary
still surfaced six defects, and **every one lived at a seam** no single crate's
tests could see:

| Defect | The seam |
|---|---|
| `when_src` was a `file:line` label; `validate` parses it for var scopes | loop-fennel wrote it, loop-engine read it, neither was wrong alone |
| Scope extraction reported the guard's parameter `v`, not the scope `qa` | the guard's shape (`(fn [v] …)`) lives in one crate, the parser in another |
| "can fix what it's judging" warned on `implement` | the heuristic was written against a rule, not against the template we ship |
| A loop head re-entered only via `on_fail: route` read as never re-entered | `validate` and the engine disagreed on what re-entry means |
| A crashed worker escalated instead of retrying | the proposal layer renders "crashed" and "stuck" identically |
| A torn ledger line was skipped but never truncated | only shows up on the *second* read, after an append |

The lesson for wave 3: parallel tasks against a fixed IR works well — the crates
merged without a single conflict — but the IR's *doc comments* are the real
contract, and every ambiguous one ("human-readable form of the guard") became a
bug. Spend the extra sentence.

`crates/loop-cli/tests/e2e.rs` now drives the real binary against `mock-pi` and
is the gate that would have caught all six.
