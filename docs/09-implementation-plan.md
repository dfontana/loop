# 09 — v1 implementation plan

The design docs (01–08) are the *what*. This is the *how* for the first working
binary, and it records the four decisions that supersede the earlier docs.

## Decisions that supersede docs 01–08

| Topic | Docs 01–08 said | v1 does |
|---|---|---|
| Harness language | Node/TS, to match pi | **Rust** (`clap` CLI, `mlua` embedding Lua 5.4) |
| Machine surface | YAML first, Fennel later ([02](02-language.md)) | **Fennel only.** No YAML machine loader is written. |
| Fennel style | Full macro DSL (`(stage … (to … :when …))`) | **Plain table** returned by the module; guards are ordinary `fn`s. Macros are a v2 layer over an unchanged IR. |
| Toolbox location | `~/.loop/` | **`~/.config/loop/`** (authored) + **`~/.local/state/loop/`** (generated) |

Cut from v1, deliberately: persistent sessions (`session: continue|fork`),
`--dry-run` cost estimation, `fork`/`join` parallelism, machine-authoring
(`loop plan`). Every stage runs in a fresh session; continuity is the ledger
digest + artifacts, exactly as [01](01-architecture.md) describes for the default
path.

Kept in v1, because they are load-bearing for correctness: the **Judge** and
**Navigator** agents ([07](07-risks.md) risk #1 — the worker must not grade its
own homework) and the **scoped-tools / mcp** wiring ([04](04-toolbox.md) — without
it a stage has no `LOOP_VARS`-emitting tools, so `when` guards have nothing to
gate on).

## Directory layout

```
~/.config/loop/                  # authored, git-able
  config.fnl                     # global defaults (was loop.config.yaml)
  playbooks/*.md
  tools/*.yaml                   # scoped-tools specs
  tools/mcp.json
  machines/*.fnl                 # machine templates
  ext/{transition,verdict,choose}-tool.ts   # materialized from the binary

~/.local/state/loop/             # generated, disposable, safe to rm -rf
  agent-dir/                     # exported as PI_AGENT_DIR for every spawn
    scoped-tools.yaml            # merged from ~/.config/loop/tools/*.yaml
    mcp.json                     # copied from tools/mcp.json
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
| `loop-core` | The IR: `Machine`, `State`, `Transition`, `Config`, `Event`, `Vars`, and the two traits (`GuardEvaluator`, `AgentRunner`) every other crate is written against. No I/O. | — |
| `loop-ledger` | JSONL append (fsync per event, tolerant of a trailing partial line), the fold to `RunState`, the artifact store (temp-file + atomic rename + sha256), the rolling digest. | core |
| `loop-fennel` | `mlua` + vendored `fennel.lua`; loads `config.fnl` / `machine.fnl` → `Machine`; owns the Lua registry of guard closures and implements `GuardEvaluator`. | core |
| `loop-toolbox` | Playbook resolution (local-first, then toolbox, then inline), `$UPPER_SNAKE` rendering over the context namespace, `tools/*.yaml` → merged `scoped-tools.yaml`, `PI_AGENT_DIR` staging, materializing the three `ext/*.ts` from `include_str!`. | core |
| `loop-runner` | Spawning `pi` (worker/judge/navigator), parsing the JSONL event stream, extracting `LOOP_TRANSITION` + `LOOP_VARS`, summing `usage.cost`; implements `AgentRunner`. | core |
| `loop-engine` | The control loop of [01](01-architecture.md): guard tiers, budgets, cycle caps, navigator caps, `on_fail` handling — plus `validate` (the static linter of [07](07-risks.md) #11). Written against the traits, so it is fully testable with fakes. | core |
| `loop-cli` | `clap`: `init`, `validate`, `run`, `status`, `resume`, `doctor`. Wires the concrete impls into the engine. | all |
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
| T4 | `loop-runner` + `mock-pi` | parse a scripted stream → `WorkerResult`; `LOOP_TRANSITION`/`LOOP_VARS` extraction; cost summing; non-zero exit + malformed line handling |
| T5 | `loop-engine` | the full control loop against fakes: three guard tiers, `on_fail` retry/route/abort, cycle + navigator caps, budget aborts, `validate` catching each authoring error in [07](07-risks.md) #11 |

**Wave 2 (me):** merge the workspaces, `loop-cli` wiring, end-to-end integration
tests driving the real binary against `mock-pi`, port `examples/local/` and
`examples/toolbox/` to the Fennel/XDG shapes, update docs 02/04/05 to match.

Every task ends green on `cargo fmt --check`, `cargo clippy -- -D warnings`, and
`cargo test`; wave 2 additionally requires the end-to-end run to reach `done`.
