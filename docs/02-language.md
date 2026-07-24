# 02 — The machine language: YAML vs Fennel

This is the authoring surface you'll touch every ticket, so it's the decision
that most shapes the day-to-day feel. The machine has to express five things:

1. **Static data** — task text, plan, QA cases, per-state model/thinking.
2. **A graph** — states and the edges between them (including cycles/loops).
3. **Tool/playbook bindings** — which toolbox pieces each state gets.
4. **Guards** — when is a transition allowed: cheap deterministic checks (`when`)
   and fuzzy semantic checks (`criteria` prompts).
5. **Occasionally, logic** — computed edges, generated states, a guard that's
   more than a comparison ("route to `debug_transient` if the error matches this
   set, else `debug_real`").

Items 1–4 are data. Item 5 is where a pure-data format strains and a real
language shines. That tension is the whole comparison.

## Option A — YAML (declarative, with an expression escape hatch)

The machine is data. Guards that need computation are expressed as small
embedded expressions over structured ledger variables (a CEL / jsonlogic-style
mini-language), and anything genuinely fuzzy is punted to a `criteria` prompt
judged by an agent. See [`examples/machine.yaml`](../examples/local/machine.yaml) for
the full version; the shape:

```yaml
ticket: PROJ-1487
task: |
  Add a `churn_score` column to the retention pipeline and expose it on
  GET /accounts/:id. Backfill 30 days.
plan: playbooks://plan-output   # or inline

defaults:
  provider: anthropic
  model: claude-sonnet-5
  thinking: medium

states:
  implement:
    playbook: implement          # resolved from ~/.loop/playbooks/implement.md
    model: claude-sonnet-5
    thinking: high
    tools: [read, edit, write, bash, spark_build]
  qa_staging:
    playbook: qa
    thinking: high
    tools: [read, bash, staging_deploy, spark_run, fetch_job_output]  # no edit — read-only QA

transitions:
  - from: implement
    to: review
    criteria: |
      The plan's checklist is fully addressed, the diff builds clean, and no
      TODO/FIXME markers remain in changed files.
    on_fail: retry
  - from: qa_staging
    to: debug
    when: "qa.result == 'fail' && qa.error_class != 'transient'"
  - from: qa_staging
    to: qa_staging               # self-loop: retry transient flakes
    when: "qa.result == 'fail' && qa.error_class == 'transient'"
    on_fail: abort
  - from: qa_staging
    to: validate_contract
    when: "qa.result == 'pass'"

loops:
  qa:            { states: [qa_staging, debug], max_cycles: 4, on_exhausted: escalate }
```

**Strengths**

- **Legible to anyone.** A reviewer skims the graph in seconds. No language to learn.
- **Diffs beautifully.** A one-line prompt tweak is a one-line diff. Great for the "hack per ticket" workflow.
- **Statically analyzable.** `loop validate` can walk the graph, check reachability, find dangling playbook/tool refs, and confirm a path to a terminal — all without executing anything.
- **Safe.** No arbitrary code runs at load time.
- **Templating is a solved problem** — reuse the `scoped-tools` `$UPPER_SNAKE` substitution you already have, over the context namespace.

**Weaknesses**

- **Item 5 gets ugly.** The moment a guard needs real logic, you're writing code
  inside a YAML string (`when: "..."`), inventing/​importing an expression
  language, and losing the editor tooling that would catch a typo. Complex
  routing becomes a wall of near-duplicate transition entries.
- **No abstraction.** Two tickets that share a "QA-with-retry-on-transient" loop
  must copy the same three transition blocks. You can factor that into a machine
  *template* fragment, but composition is textual, not semantic.
- **Stringly-typed.** `when` expressions and variable names are strings the
  loader parses; mistakes surface at run time, not author time.

## Option B — Fennel (a Lisp that compiles to Lua)

The machine is a program that *returns* a machine table. The trap here — and the
first version of the example fell into it — is to write Fennel that just passes
Lua tables to constructor functions:

```fennel
;; DON'T: this is the worst of Lisp — full parens tax, zero DSL payoff.
;; It's YAML with extra punctuation, and it reads like code because it *is* code.
(state :qa-staging {:playbook :qa :thinking "high"
                    :tools [:read :bash :staging_deploy :spark_run :fetch_job_output]})
(transition :implement :review {:criteria "…" :on-fail :retry})
```

Fennel shines at DSLs specifically because **macros let you change the surface
syntax**, not because you can pass tables around. Done right, the macros drop the
quotes and braces, co-locate each stage with its own outgoing edges, let guards
be real expressions, and — the thing YAML fundamentally can't do — let you factor
shared config as ordinary bindings. See [`examples/machine.fnl`](../examples/local/machine.fnl)
for the full, YAML-equivalent version; the shape:

```fennel
(local coder [read edit write bash spark_build])   ; shared, because it's code

(machine PROJ-1487
  (defaults :model claude-sonnet-5 :thinking medium :session fresh :tools [read bash])
  (cheap-agents claude-haiku-4-5)                   ; judge + navigator in one line

  ;; a stage co-locates its edges; `:when` is a real guard expr, `:judge` the LLM tier
  (stage implement :thinking high :session (continue impl) :tools coder
    (to review :judge "Plan's four items done; build green; no TODO/FIXME."))

  (stage qa-staging :thinking high :tools [read bash staging_deploy spark_run fetch_job_output]
    (to validate-contract :when (= qa.result :pass))
    (to qa-staging        :when (and (= qa.result :fail) (transient? qa)) :backoff (secs 30))
    (to debug             :when (and (= qa.result :fail) (real? qa))))

  (retry-loop qa [qa-staging debug] :max 4 :on-exhausted escalate))
```

Bare symbols (`implement`, `high`, `qa-staging`) are quoted by the macros; a
`:tools` *symbol* like `coder` is evaluated so bundles are reusable; guard
expressions run with the ledger's scope tables (`qa`, `review`, `contract`)
bound. The three-clause `qa-staging` block is the whole transient-vs-real routing
that took **three separate near-duplicate `when:` rows** in YAML — and the full
`.fnl` comes out to ~40 content lines against the YAML's ~110. That density and
the `coder`/`cheap-agents` factoring are the DSL payoff; the naive table-passing
version above buys you none of it.

**Strengths**

- **Item 5 is free.** Computed branches, error-class routing, generated states
  (`(for [i 1 shards] (state (.. :migrate- i) ...))`) are ordinary code.
- **Real abstraction.** `retry-loop`, `qa-gate`, `deploy-then-verify` become
  macros/functions in your toolbox; a machine composes them. This is the deepest
  answer to "minimize unique work per ticket" — the primitives are *callable*,
  not copy-pasted.
- **Homoiconic → DSL-friendly.** Lisp is the natural host for exactly this kind
  of "data that's occasionally code" definition. Done right (co-located edges,
  quoted symbols, factored bundles) the macro layer makes the 90% case *denser*
  than the YAML, not just as clean — but only if you invest in the macros; the
  naive table-passing style is strictly worse than YAML.
- **One language for guards and structure.** No separate embedded expression
  mini-language to design, document, and debug.

**Weaknesses**

- **Unfamiliarity tax.** Lisp + Lua semantics is a real barrier for you-in-six-
  months and for anyone else. Parens fatigue is real for infrequent editors.
- **A Lua runtime in the CLI.** `loop` is Node/TS (to match pi). To *execute* a
  `.fnl` machine you must embed Lua: [`wasmoon`](https://github.com/ceifa/wasmoon)
  (Lua 5.4 in WASM) or [`fengari`](https://github.com/fengari-lua/fengari) (Lua
  in JS), compile Fennel→Lua (the Fennel compiler is itself Lua, or shell out to
  the `fennel` binary), then marshal the returned table across the VM boundary.
  That's a genuine dependency and a marshalling surface to get right.
- **Weaker static analysis.** `loop validate` can't fully reason about a graph
  built by arbitrary code without running it. You can validate the *resolved*
  table after one evaluation, but "unreachable state" analysis over dynamic
  construction is harder, and errors can point into *compiled Lua*, not your
  Fennel source, unless you wire up source maps.
- **Safety.** Evaluating a machine now runs arbitrary code at load time. Fine for
  files you author; a sandbox (wasmoon is already isolated) matters if machines
  are ever shared.
- **Noisier diffs for trivial edits.** A one-word prompt change can sit inside a
  macro call, and reviewers must read code, not scan data.

## Why not plain Lua?

Fennel dominates plain Lua for this use case: same runtime and ecosystem, but
macros are what let you hide the boilerplate behind a clean DSL. Plain Lua
tables-as-config would read like verbose YAML *without* YAML's legibility. If you
go the VM route, go Fennel.

## Recommendation: YAML surface now, Fennel as an opt-in backend later

Make the **loader pluggable** and ship YAML first:

- `machine.yaml` → parsed to the canonical machine object.
- `machine.fnl` → evaluated in an embedded Lua VM, returning the *same* canonical
  object, so everything downstream (validator, runner, ledger) is identical.

Both compile to one internal representation. This lets you:

1. Get the whole system working against the safe, legible, analyzable format that
   handles ~90% of tickets — and matches the "read it in five seconds, hack it,
   throw it away" ethos.
2. Reach for Fennel exactly on the tickets that need item 5 — dynamic graphs,
   real routing logic, heavy reuse — without a second engine.
3. Keep the **toolbox** the true home of reuse regardless of surface: playbooks
   are Markdown, tools are `scoped-tools` YAML, and machine *fragments/templates*
   can be YAML anchors today and Fennel macros later. Most reuse lives there, not
   in the machine language.

A pragmatic middle path if you want *some* logic without the VM: keep YAML but let
a transition's target be a **playbook-authored decision** — i.e. a `branch:`
whose routing is itself a cheap agent call constrained to reachable states (the
Navigator, invoked deliberately rather than only on error). You get data-driven
structure with an LLM handling the fuzzy routing, and you never leave YAML. Fennel
then earns its keep only when the routing is *deterministic and complex enough*
that you'd rather write `case` than pay for a model call.

**My read:** start YAML, keep the IR boundary clean, and add the Fennel backend
the first time you catch yourself copy-pasting a third near-identical transition
block or writing a five-clause `when` string. That's the signal that item 5 has
outgrown data.
