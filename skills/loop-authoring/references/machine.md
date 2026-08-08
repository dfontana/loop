# `machine.fnl` — the ticket machine

`<project>/.loop/machine.fnl` evaluates to a plain table describing one ticket's state graph plus every setting the run uses. It is the only file loop reads settings from — there is no second config file anywhere.

Fennel here is a **configuration surface, not a scripting hook**. It evaluates once, to a table. There are no callbacks into the machine at run time, and no expression language in any value.

## Where everything lives

One directory, `<project>/.loop/`, and nowhere else:

```
machine.fnl                  the ticket machine — the only required file
task.md                      prose, referenced by :task
plan.md                      prose, referenced by :plan
stage-prompts/<name>.md      a stage's prompt, referenced by bare name
skills/<name>/SKILL.md       a skill as a directory
skills/<name>.md             a skill as a single file
ledger.jsonl                 the append-only run log
artifacts/                   snapshots, named <state>-<cycle>-<name>
run/                         rendered prompts and handoff files — derived, gitignored
.gitignore                   one line, `run/`
```

Three lifetimes:

- **Authored** — `machine.fnl`, the prose, `stage-prompts/`, `skills/`. `loop init` writes starting copies and then never rewrites a file that exists.
- **Recorded** — `ledger.jsonl`, `artifacts/`. Written by a run; the reason `.loop/` is worth committing alongside the branch.
- **Derived** — `run/`, holding `<state>-<cycle>-<attempt>-system.md` (the prompt actually handed to pi) and `<state>-<cycle>-<attempt>-handoff.json`. Regenerated every stage; deleting it costs nothing.

A stage prompt or skill name resolves inside `.loop/` and nowhere else. No precedence order, no shadowing, no shared library. A name either names a file under `.loop/stage-prompts/` or `.loop/skills/`, or it is an error printing the path it looked at.

The consequence: **carrying a tuned machine to the next ticket is a copy** (`loop init <TICKET> --from <DIR>`), so an improvement made in one ticket does not reach tickets already in flight.

## Top-level keys

| Key | Required | Type | Notes |
| --- | --- | --- | --- |
| `:ticket` | yes | string | Identifies the run. Becomes `$TICKET_ID` and the `TICKET_ID` env var. |
| `:task` | yes | string | Path relative to `machine.fnl`, read into `$TASK` — or inline prose. See the `.md` rule below. |
| `:plan` | yes | string | Same, into `$PLAN`. |
| `:qa-cases` | no | `[{:id :desc}]` | Both fields required per entry. Renders to `$QA_CASES`. |
| `:provider` | no | string | The provider under all three roles; a role naming its own wins. Default `"anthropic"`. |
| `:defaults` | no | `{:provider :model :thinking :skills :mcp}` | Sits under every state. `:skills`/`:mcp` stack under every state's own. |
| `:worker` | no | `{:provider :model :thinking}` | The Worker floor. Default `claude-sonnet-5` / `medium`. |
| `:judge` | no | `{:provider :model :thinking}` | The Judge model. **Not layered** — no state can change it. Default `claude-haiku-4-5` / `low`. |
| `:navigator` | no | same, plus `:max-invocations` | Default `claude-haiku-4-5` / `low`, cap `5`. |
| `:budgets` | no | `{:usd :wallclock-s :max-transitions}` | May only **tighten** the built-in floor `15.0` / `7200` / `60`. |
| `:digest-last-n` | no | int | How many recent committed transitions `$LEDGER_DIGEST` lists. Default `8`. |
| `:pi-extensions` | no | `[string]` | A declaration of what is installed in pi. Drives one lint; turns nothing on. Default `["mcp" "review-model-selector"]`. |
| `:entry` | conditional | string | Must name a declared state. Inferable only when there is exactly one state. |
| `:terminals` | yes | `[string]` | Terminal names. These are **not** states — no stage prompt, never spawn an agent. |
| `:escalation-state` | no | string | Must name a declared state or terminal. |
| `:states` | yes | table | At least one entry. |
| `:transitions` | no | list | Absent means no edges, which `loop validate` rejects as unreachable/terminal-less. |
| `:loops` | no | list | Cycle counting and caps. |

Anything a machine does not name comes from loop's built-in floor, which is code, not a file. The one setting no machine can reach is the pi binary: that comes from `LOOP_PI_BIN` (default `pi`).

### Sharp edges

- **`:task` / `:plan` ending in `.md` that does not resolve is a hard error, not a fallback.** The value is first tried as a path relative to `machine.fnl`. If the file exists, its contents become `$TASK`/`$PLAN`. If it does not exist _and_ the value ends in `.md`, loop fails with ``could not resolve task `task.md` `` followed by the path it tried. Any other non-resolving string is taken as **inline prose** — which is what makes `:task "Bump the timeout to 30s."` work for a throwaway ticket.
- **`:entry` is only inferable with exactly one state.** Omit it with more and you get `missing :entry and :states has N entries; ambiguous which one starts the machine`.
- **Terminals are not states.** `:terminals ["done" "blocked"]` declares two names a transition may point `:to`. Entering one ends the run.
- **`:escalation-state` is committed to directly**, bypassing edge selection and every guard tier. If it is also a terminal, the run reports `Failed` rather than `Done`. With no escalation state configured, an escalation ends the run `Aborted`.
- **`:budgets` can only tighten.** `:usd 100` against a floor of `15.0` leaves you with `15.0`.
- **`:navigator {:max-invocations N}`** is a cap that applies **both** run-wide and per source state; hitting either escalates instead of spawning.
- **Every struct is `deny_unknown_fields`.** A misspelled key errors at any depth, naming the path and listing the keys that exist:

```
error: machine: at `states.qa-staging.thinking`: deserialize error: unknown variant `hihg`,
expected one of `off`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`
```

### `:pi-extensions` is a declaration, not a switch

loop never turns this list into a flag — pi has none to turn, since there is no way to enable an installed extension by name. The Worker is simply spawned without `--no-extensions`, so pi's ambient discovery loads whatever is installed.

What the key buys is one lint: if a state names `:mcp` servers and `"mcp"` is not in `:pi-extensions`, `loop validate` errors with _the stage would be told to call a tool it does not have_.

## States

`:states` is a table of state id → state table.

| Key | Required | Type | Effect |
| --- | --- | --- | --- |
| `:stage-prompt` | one of the two | string | A bare name resolved in `.loop/stage-prompts/`, or a path if it contains `/`. |
| `:prompt` | one of the two | string | Inline prompt text. No filesystem access at all. |
| `:model` | no | string | Model override for this stage (highest-priority layer). |
| `:thinking` | no | string | `off`·`minimal`·`low`·`medium`·`high`·`xhigh`·`max`. |
| `:provider` | no | string | Provider override for this stage. |
| `:skills` | no | `[string]` | Skill names, unioned with `:defaults`. |
| `:mcp` | no | `[string]` | MCP server names, unioned the same way. |
| `:description` | no | string | One line on what the stage is for. |

A state needs `:stage-prompt` **or** `:prompt`; with neither you get `state {id}: needs either :stage-prompt or :prompt`.

**`:description` is not decorative.** It is what the Navigator reads in the graph summary when it has to pick a state on a stuck Worker's behalf, and it is the label `loop diagram` draws. A state with no description appears to the Navigator as `(no description)`.

## Transitions

`:transitions` is a list of edge tables. **Order matters**: for a given `(from, to)` pair the _first_ matching edge is taken, and `loop validate` errors on a duplicate rather than leaving the second one dead.

| Key | Required | Type | Effect |
| --- | --- | --- | --- |
| `:from` | yes | string | Must be a declared state. |
| `:to` | yes | string | A declared state or a terminal. |
| `:check` | no | string, or `{:cmd :timeout-s}` | A command the harness runs itself. Exit 0 passes. |
| `:criteria` | no | string | What an independent Judge model is asked to decide. |
| `:on-fail` | no | `"retry"` \| `"abort"` \| `{:route "id"}` | Default `"retry"`. |
| `:max-attempts` | no | int ≥ 1 | Default `3`. Bounds `"retry"`. |
| `:backoff-s` | no | int | Seconds to sleep **after** this edge commits. |

- **`:check`** is the one signal a Worker cannot author — it runs in the harness's own subprocess after the stage exits. An empty string is rejected: `transitions[N]: :check command is empty — omit the key instead`.
- **`:criteria`** is prose handed to the Judge as its system prompt. Use it for what no exit code decides: "every item in the plan is addressed", "this is a real fix, not a widened assertion".
- **`:on-fail`** applies to the edge regardless of which tier failed. `"retry"` re-enters the source state at `attempt + 1` and consumes no transition budget. `"abort"` ends the run `Failed`. `{:route "x"}` commits straight to `x` with **no guard tiers and no backoff** — the usual way to send a failed review back to `implement` with the findings on the ledger.
- **`:max-attempts`** is the only bound on `"retry"`. A retry commits no transition, so a loop head's cycle counter never advances and `:max-cycles` can never fire on a stage retrying itself. Reaching the cap escalates with a fatal naming the edge and the bound. Lower it to `1` to make an edge one-shot without reaching for `"abort"` (which ends the run rather than escalating). No effect under `"abort"` or `{:route ...}`. `0` is rejected.
- **`:backoff-s`** sleeps after the commit event is written, so the commit is already durable. It is a blocking sleep in the `loop run` process.

An edge with neither `:check` nor `:criteria` is legal and draws a `loop validate` warning: _the worker's proposal is committed unexamined_. Warnings alone still exit 0.

### Writing a `:check`

- Run as **`bash -c <cmd>`** — bash specifically, not `sh`, not a login shell. No profile is sourced.
- **cwd is the project directory**, not `.loop/`.
- **stdin is null.** A check that waits for input hangs until its timeout.
- **Exit 0 passes.** Non-zero is an ordinary guard failure routed through `:on-fail`, not a harness error.
- **`$VAR` substitution happens before execution**, over the full template-variable table. `$TICKET_ID`, `$CYCLE`, `$STATE` are the useful ones; `$TASK` and `$LEDGER_DIGEST` will interpolate a whole document onto your command line.
- **`TICKET_ID`, `STATE`, `CYCLE`, `ATTEMPT` are also real environment variables** in the subprocess. Prefer `"$CYCLE"` inside a script you invoke; the template form is for the command string itself.
- **stdout and stderr are merged** into one capture.
- **Timeout defaults to 120 s**, overridable per edge with the table form. On timeout the process is killed, the exit code is recorded absent, and `\n[check timed out after {N}s]` is appended. A timeout is a failure.
- **Output is truncated to the last 16 KiB.** Put the signal at the _end_ of your command's output.
- **The output is shown to the Judge** when the same edge also has `:criteria`. Design checks so their output is worth reading, not just their exit code.

```fennel
;; Bare string — the common case.
{:from "implement" :to "review"
 :check "cargo test --quiet"}

;; Table form, when 120s is not enough.
{:from "debug" :to "qa-staging"
 :check {:cmd "bash .loop/skills/spark-run/run.sh" :timeout-s 900}}
```

Two patterns worth stealing:

**Same script in `:skills` and in `:check`.** The agent and the harness run identical code, so an agent cannot pass a gate the harness would fail.

**One classifier, one branch per edge.** When a stage can fail in two ways that demand different responses, each outgoing edge asserts its own branch of one script's taxonomy — so "transient" is decided by a versioned regex set and an exit code rather than by a tired agent that would rather retry than debug:

```fennel
{:from "qa-staging" :to "qa-staging"
 :check "bash .loop/skills/spark-run/classify.sh --expect transient"
 :backoff-s 30 :on-fail "abort"}
{:from "qa-staging" :to "debug"
 :check "bash .loop/skills/spark-run/classify.sh --expect real"}
```

### Guard order at run time

Two tiers, cheapest first: `check`, then `criteria`. A tier the edge does not declare is skipped, and `skip` counts as passing; a report passes when **no tier failed**. **A failed `check` short-circuits — the Judge is never spawned**, so you never pay for a model call to evaluate a diff whose suite just went red.

## Loops

`:loops` exists to **count cycles and cap them**. It does not create edges — the back-edges must already be in `:transitions`.

| Key | Required | Type | Effect |
| --- | --- | --- | --- |
| `:name` | yes | string | Used in the exhaustion error and the diagram note. |
| `:states` | yes | `[string]` | Non-empty. **`states[0]` is the loop head.** |
| `:max-cycles` | yes | int | Cap on cycles through the head. |
| `:on-exhausted` | no | `"escalate"` \| `"abort"` | Default `"escalate"`. |

**`states[0]` is load-bearing.** The head is the state whose _re-entry_ counts a cycle; the remaining names document the loop's extent. Two loops may share a head — `{:states ["qa-staging" "debug"]}` and `{:states ["qa-staging"]}` count different things about the same node.

The cap is enforced **prospectively at commit time**, and only when the target is a head — so the run never enters the (N+1)th cycle, it fails at the boundary. Exhaustion writes `loop {name} exhausted max_cycles={n} at head {head}` and then escalates or aborts.

`loop validate` errors if a declared head is never re-entered by any transition, which usually means the wrong element of `:states` was named first.

## Escalation and budgets

`:escalation-state` is the machine's designated failure destination. Committing to it **bypasses edge selection and every guard tier** — it needs no declared transition. Landing on it reports `Failed`, not `Done`, which is the whole reason it is declared separately: a script can tell "gave up" from "succeeded" by exit code alone.

It fires on:

| Trigger                                             | Where       |
| --------------------------------------------------- | ----------- |
| Navigator invocation cap exceeded                   | routing     |
| Edge selection found no matching transition         | routing     |
| A loop exhausted `:max-cycles` with `"escalate"`    | commit      |
| An edge's `:max-attempts` exhausted under `"retry"` | routing     |
| 3 consecutive Worker process crashes                | stage entry |

Budgets, from the built-in floor and only tightenable:

| Limit              | Default | Breach when                                  |
| ------------------ | ------- | -------------------------------------------- |
| `:usd`             | 15.0    | `cost_usd > usd` (strictly greater)          |
| `:wallclock-s`     | 7200    | `elapsed_s > wallclock_s` (strictly greater) |
| `:max-transitions` | 60      | `transitions >= max_transitions`             |

Checked in that order, first wins. **A budget breach is always `Aborted`, never `Failed`.** Two honest caveats: retries are free against `max_transitions` (it counts committed transitions only), and budgets are sampled **between** stages — a single long Worker spawn can blow the wallclock and not be noticed until it finishes. Worker, Judge, and Navigator spend all count toward `:usd`.

## Keys that were removed or renamed

These fail the load **by name**, because each one used to do something.

| Key | What to write instead |
| --- | --- |
| `:playbook` (on a state) | **Renamed** to `:stage-prompt`; `.loop/playbooks/` is now `.loop/stage-prompts/`. Same meaning: a bare name, or a path if it contains `/`. The inline-text key is still `:prompt`. |
| `:context` | Removed. `$LEDGER_DIGEST` in a stage prompt is the only continuity channel; tune `:digest-last-n`. |
| `:default-skills` | `:defaults {:skills [..]}` |
| `:default-mcp` | `:defaults {:mcp [..]}` |
| `:transition-mode` | Removed. An off-graph target routes to the Navigator, which is what `open` meant and is now the only behavior. |
| `:when` (on a transition) | Removed. Express the condition as a `:check` the harness runs, or as `:criteria` for the Judge. |

`:playbook` gets its own error precisely because it is a rename: reported generically it would offer two plausible replacements, and picking `:prompt` is not a load error — it is a stage that runs the string `"qa"` as its entire prompt.

There is also no `config.fnl`. Those keys (`:provider`, `:worker`, `:judge`, `:navigator`, `:pi-extensions`, `:budgets`, `:digest-last-n`, and the two `default-*` lists) are machine keys now.

## A complete machine

```fennel
{:ticket "PROJ-1487"

 ;; Prose lives in markdown, referenced by path relative to this file.
 :task "task.md"
 :plan "plan.md"

 ;; What "done and correct" means. Rendered into prompts as $QA_CASES.
 :qa-cases [{:id "pipeline"
             :desc "Retention job populates churn_score for all active accounts, 30d backfilled."}
            {:id "contract"
             :desc "GET /accounts/:id returns churn_score as a number matching the OpenAPI schema."}]

 ;; The provider under all three roles; a role naming its own wins.
 :provider "anthropic"

 ;; Sits under every state, over the :worker floor below.
 :defaults {:model "claude-sonnet-5" :thinking "medium"
            :skills [] :mcp []}

 ;; May only tighten loop's built-in floor (15.0 / 7200 / 60).
 :budgets {:usd 8 :wallclock-s 5400 :max-transitions 40}

 ;; The three roles. Nothing but this file sets them.
 :worker    {:model "claude-sonnet-5"  :thinking "medium"}
 :judge     {:model "claude-haiku-4-5" :thinking "low"}
 :navigator {:model "claude-haiku-4-5" :thinking "low" :max-invocations 5}

 ;; What is installed in pi. This does not turn anything on.
 :pi-extensions ["mcp" "review-model-selector"]

 :digest-last-n 8

 :entry "implement"
 :terminals ["done" "blocked"]
 :escalation-state "blocked"

 :states
 {:implement {:stage-prompt "implement"          ; .loop/stage-prompts/implement.md
              :thinking "high"
              :skills ["spark-build"]
              :description "Implement the plan; keep the build green."}

  :review {:stage-prompt "review"
           :thinking "high"
           :description "Adversarial review of the diff; find real defects."}

  :qa-staging {:stage-prompt "qa"
               :thinking "high"
               :skills ["staging-deploy" "spark-run"]
               :mcp ["warehouse"]
               :description "Deploy to staging, run the pipeline, grade it."}

  :debug {:stage-prompt "debug-spark"
          :thinking "high"
          :skills ["spark-build" "debug-transient"]
          :description "Diagnose a real pipeline failure and fix it."}

  :validate-contract {:stage-prompt "validate-contract"
                      :thinking "medium"
                      :skills ["staging-deploy" "contract-check"]
                      :description "Confirm the API contract matches the OpenAPI schema."}

  :open-pr {:stage-prompt "open-pr"
            :thinking "low"
            :skills ["open-pr"]
            :description "Open or update the pull request for this branch."}}

 :transitions
 [{:from "implement" :to "review"
   :check "bash .loop/skills/spark-build/build.sh"
   :criteria "The plan's four items are all addressed in the diff, and no TODO/FIXME markers remain in changed files."
   :on-fail "retry"}

  {:from "review" :to "implement"
   :criteria "The review identified defects that require code changes."}
  {:from "review" :to "qa-staging"
   :criteria "The review found no defect requiring a code change."}

  ;; Three-way fail routing off one classifier: a transient flake retries in
  ;; place with backoff and touches no code, a real failure spawns the
  ;; debugger, a pass moves on.
  {:from "qa-staging" :to "qa-staging"
   :check "bash .loop/skills/spark-run/classify.sh --expect transient"
   :backoff-s 30
   :on-fail "abort"}
  {:from "qa-staging" :to "debug"
   :check "bash .loop/skills/spark-run/classify.sh --expect real"}
  {:from "qa-staging" :to "validate-contract"
   :check "bash .loop/skills/spark-run/classify.sh --expect pass"
   :criteria "The output sample satisfies every QA case, not just the job's exit status."}

  {:from "debug" :to "qa-staging"
   :check "bash .loop/skills/spark-build/build.sh"
   :criteria "A concrete fix to the diagnosed failure was applied — not a retry, a widened assertion, or a disabled check."
   :on-fail "retry"}

  {:from "validate-contract" :to "implement"
   :criteria "The staging response does not match the committed OpenAPI schema."}
  {:from "validate-contract" :to "open-pr"
   :check "bash .loop/skills/contract-check/check.sh /accounts/42"}

  {:from "open-pr" :to "done"
   :criteria "A pull request exists for this branch with a populated description."}]

 ;; states[0] is the loop head — the state whose re-entry counts a cycle.
 :loops
 [{:name "qa" :states ["qa-staging" "debug"] :max-cycles 4 :on-exhausted "escalate"}
  {:name "qa-transient" :states ["qa-staging"] :max-cycles 3 :on-exhausted "escalate"}]}
```

Run `loop validate` after every edit. It resolves every stage prompt and skill name, walks reachability from `:entry`, checks that each state has a path to a terminal, and catches duplicate edges. `loop diagram` renders the same machine as mermaid if you would rather look at it.

## Reuse across tickets

`loop init <TICKET> --from <DIR>` **copies** a `.loop/`-shaped directory instead of writing the bundled one. `<DIR>` must hold a `machine.fnl`; a leading `~/` is expanded; files that already exist are never overwritten; and `ledger.jsonl`, `run/`, and `artifacts/` at the top level are skipped, because they are what a _run_ leaves behind.

It is a copy, not a lookup — what you started from is recorded in the ticket, so editing the source afterwards cannot change a run already in flight. The flip side: fixing a stage prompt in a kit does not fix it in the tickets already copied from it.

**`$TICKET` is scaffold-time text.** `loop init` does a plain string replacement of `$TICKET` in the copied `machine.fnl` (and in `task.md`/`plan.md` when it writes them itself). This is _not_ the `$VAR` render engine and _not_ the runtime `$TICKET_ID`. When the source already has a real id (it came from an earlier `loop init`), the **value** of the first `:ticket` key is rewritten in place instead.

To build a kit: `cp -R .loop ~/loop-kits/<name>`, then replace the ticket id with `$TICKET`, delete `ledger.jsonl` / `artifacts/` / `run/`, generalize the `:qa-cases` descriptions into the shape of the question rather than this ticket's answer, and mark the lines you always end up editing with an `EDIT` comment.

**When a new kit is warranted:** not "this ticket had one more step" — a sequential stage is a two-line edit. The bar is **a state you would have to _re-enter_**: a genuine ambiguity the graph has to resolve by looping rather than by proceeding. The canonical case is transient-vs-real failure, which needs a head, a back-edge, a cap, and a classification check. That is a shape, and shapes are what a kit is for.
