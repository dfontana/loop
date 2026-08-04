# Customizing a loop

This is the reference for shaping a loop to your work: where configuration lives, what every key does, and what surfaces you can reach from a stage.

It defines what the keys _mean_. How they are evaluated at run time — the run loop, the three roles, the handoff protocol, guard ordering, the ledger — is [02-how-it-works.md](02-how-it-works.md). Flags, environment variables, and exit codes are [04-cli-reference.md](04-cli-reference.md). Why any of it is shaped this way is [05-design-notes.md](05-design-notes.md).

---

## Where configuration lives

One directory: `<project>/.loop/`. Everything loop reads or writes for a ticket is inside it, and there is nowhere else to look.

```
machine.fnl                  the ticket machine — the only required file
task.md                      prose, referenced by :task
plan.md                      prose, referenced by :plan
stage-prompts/<name>.md      a stage's prompt, referenced by bare name
skills/<name>/SKILL.md       a skill as a directory
skills/<name>.md             a skill as a single file
ledger.jsonl                 the append-only run log
artifacts/                   snapshots, named <state>-<cycle>-<name>
run/                         rendered prompts and handoff files — derived
.gitignore                   one line, `run/`
```

Three kinds of thing, with three lifetimes:

- **Authored** — `machine.fnl`, the prose, `stage-prompts/`, `skills/`. Yours. `loop init` writes starting copies and then never rewrites a file that exists.
- **Recorded** — `ledger.jsonl` and `artifacts/`. Written by a run, and the reason `.loop/` is worth committing alongside the branch it belongs to.
- **Derived** — `run/`, holding `<state>-<cycle>-<attempt>-system.md` (the fully-substituted prompt actually handed to pi) and `<state>-<cycle>-<attempt>-handoff.json` (the file the Worker writes back). Regenerated every stage, so `loop init` puts it in `.loop/.gitignore` and deleting it costs nothing.

The rendered prompt under `run/` is the single most useful file when a stage misbehaves — it is what the agent was actually told, not what the stage prompt says.

`loop init` creates `machine.fnl`, `task.md`, `plan.md`, four bundled stage prompts, one bundled skill, and the `.gitignore`. `ledger.jsonl`, `artifacts/`, and `run/` appear when something needs them.

### Stage prompts and skills are not the same thing

Both are markdown with YAML frontmatter, and a file that works as one will usually parse as the other. The format is where the similarity stops. What separates them is **how each reaches the model**, and it is worth getting straight before you write either, because it decides which one a given piece of text has to be.

| | Stage prompt | Skill |
| --- | --- | --- |
| Bound to | Exactly one state | Any state that names it, via `:skills` |
| Reaches pi as | `--append-system-prompt <path>` | `--skill <path>` |
| In the stage's context | **Always.** It _is_ the system prompt. | **Only if the model elects to load it**, having seen its name and description. |
| Does loop read it? | Yes — parses frontmatter, substitutes `$VAR`s | **Never.** Checks the path exists and hands it over. |
| Can carry `$TASK`, `$PLAN`, `$LEDGER_DIGEST` | Yes | No. loop never renders one, so the text arrives literally. |
| Can set the stage's model | Yes, via frontmatter — layer 2 of [model resolution](#model-resolution) | No |
| Can carry scripts | No, one file of prose | Yes — a directory, and `:check` can run the same script |

Two rules fall out of that, and they settle most authoring questions:

- **Anything the stage must be told is a stage prompt.** "Offered" is not "told". A description of the job, the definition of done, the instruction to write a handoff — none of these can be a skill, because a skill the model chooses not to open is a skill that did nothing.
- **Anything that depends on where the run has been is a stage prompt.** There is no automatically prepended context header (see [template variables](#template-variables)), so a `$VAR` is the only way the task, the plan, the digest, or the Navigator's note enters a stage — and a skill is never rendered.

What is left for skills is the good case for them: situational know-how that most runs will not need, and the scripts that carry it out. "How to tell a flaky test from a real one" is worth having available and not worth spending context on every stage. That is a skill.

### Names resolve in one place

A stage prompt or skill name resolves inside `.loop/` and nowhere else. There is no precedence order, no shadowing, and no shared copy elsewhere on the machine that a local file overrides — a name either names a file under `.loop/stage-prompts/` or `.loop/skills/`, or it is an error that prints the path it looked at. The exact candidate lists are under [Stage prompts](#stage-prompts) and [Skills](#skills).

The cost of that is real and worth stating: there is no shared library to edit once. Carrying a tuned stage prompt, skill, or machine to the next ticket is a copy — `loop init <TICKET> --from <DIR>` — so an improvement you make in one ticket does not reach the tickets already in flight. See [reuse across tickets](#reuse-across-tickets).

The project root is the only path loop takes from outside the directory, and it comes from `-C/--dir` or the cwd; see [04-cli-reference.md](04-cli-reference.md#environment-variables).

---

## `machine.fnl` — the ticket machine

`<project>/.loop/machine.fnl` evaluates to a table describing one ticket's state graph, plus every setting the run uses. This is the file you edit per ticket, and it is the only file loop reads settings from. Fennel here is a configuration surface, not a scripting hook: it evaluates once, to a plain table, and there are no callbacks into the machine at run time.

### Top-level keys

| Key | Required | Type | Notes |
| --- | --- | --- | --- |
| `:ticket` | yes | string | Identifies the run. Becomes `$TICKET_ID` and the `TICKET_ID` env var. |
| `:task` | yes | string | A path relative to `machine.fnl`, read into `$TASK` — or inline prose. See the `.md` rule below. |
| `:plan` | yes | string | Same, into `$PLAN`. |
| `:qa-cases` | no | `[{:id :desc}]` | Both fields required per entry. Renders to `$QA_CASES`. |
| `:provider` | no | string | The provider under all three roles; a role naming its own wins. Default `"anthropic"`. |
| `:defaults` | no | `{:provider :model :thinking :skills :mcp}` | Sits under every state. The model half is layer 3 of [model resolution](#model-resolution); `:skills` and `:mcp` stack under every state's own. |
| `:worker` | no | `{:provider :model :thinking}` | The Worker floor — layer 4, under `:defaults`. Default `claude-sonnet-5` / `medium`. |
| `:judge` | no | `{:provider :model :thinking}` | The Judge model. Not layered — no state can change it. Default `claude-haiku-4-5` / `low`. |
| `:navigator` | no | same, plus `:max-invocations` | The Navigator model and its cap. Default `claude-haiku-4-5` / `low`, cap `5`. |
| `:budgets` | no | `{:usd :wallclock-s :max-transitions}` | May only **tighten** loop's built-in floor — per field, the smaller value wins. Floor: `15.0` / `7200` / `60`. |
| `:digest-last-n` | no | int | How many recent committed transitions `$LEDGER_DIGEST` lists. Default `8`. |
| `:pi-extensions` | no | `[string]` | A declaration of what you have installed in pi. Drives one lint; turns nothing on. Default `["mcp" "review-model-selector"]`. |
| `:entry` | conditional | string | Must name a declared state. See below. |
| `:terminals` | yes | `[string]` | Terminal names. These are **not** states — they have no stage prompt and never spawn an agent. |
| `:escalation-state` | no | string | Must name a declared state or terminal. |
| `:states` | yes | table | At least one entry. |
| `:transitions` | no | list | Absent means no edges, which `loop validate` will reject as unreachable/terminal-less. |
| `:loops` | no | list | Cycle counting and caps. |

**What a machine does not name comes from loop's built-in floor**, which is code rather than a file — `Config::defaults` in `crates/loop/src/core/config.rs`. There is nothing to edit there and nothing to scaffold; a machine that wants a different model, budget, or digest length says so in `machine.fnl`, in the file under review. The one setting no machine can reach is the pi binary itself, which comes from `LOOP_PI_BIN` (default `pi`).

**Every struct is `deny_unknown_fields`.** A misspelled key is an error naming the field and listing the ones that exist, at any depth — `:playbok "implement"` no longer loads and then complains about a missing stage prompt, and `:max-cycels 4` no longer leaves the bound at its default while the run keeps going. Errors carry the path to the offending value:

```
error: machine: at `states.qa-staging.thinking`: deserialize error: unknown variant `hihg`,
expected one of `off`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`
```

An unknown key at the top level lists the ones that exist, the same way:

```
error: machine: at `playbok`: deserialize error: unknown field `playbok`, expected one of
`ticket`, `task`, `plan`, `qa-cases`, `defaults`, `budgets`, `judge`, `navigator`, `entry`,
`terminals`, `escalation-state`, `states`, `transitions`, `loops`, `provider`, `worker`,
`digest-last-n`, `pi-extensions`
```

Sharp edges worth knowing before you edit:

- **`:task` / `:plan` ending in `.md` that does not resolve is a hard error, not a fallback.** The value is first tried as a path relative to `machine.fnl`. If that file exists, its contents become `$TASK`/`$PLAN`. If it does not exist _and_ the value ends in `.md`, loop fails with ``could not resolve task `task.md` `` followed by the path it tried, on the assumption you meant a file and mistyped it. Any other non-resolving string is taken as inline prose, which is what makes `:task "Bump the timeout to 30s."` work for a throwaway ticket.
- **`:entry` is only inferable when there is exactly one state.** Omit it with one state and that state is the entry. Omit it with more and you get ``missing `:entry` and `:states` has N entries; ambiguous which one starts the machine``. Naming a state that does not exist gives `` `:entry` `x` is not a declared state ``.
- **Terminals are not states.** `:terminals ["done" "blocked"]` declares two names a transition may point `:to`; neither has a stage prompt. Entering one ends the run.
- **`:escalation-state` is committed to directly**, bypassing edge selection and every guard tier. If it is also a terminal, the run reports `Failed` rather than `Done`. With no escalation state configured, an escalation ends the run `Aborted`. See [escalation](02-how-it-works.md#escalation).
- **`:budgets` can only tighten.** Writing `:usd 100` against the built-in floor of `15.0` leaves you with `15.0`. Machines are for narrowing, not for raising the roof.
- **`:navigator {:max-invocations N}`** is a cap that applies **both** run-wide and per source state; hitting either escalates instead of spawning. See [the Navigator](02-how-it-works.md#navigator).

### `:pi-extensions` is a declaration, not a switch

loop never turns this list into a command-line flag, because pi has none to turn: there is no way to enable an _installed_ extension by name. The Worker is simply spawned without `--no-extensions`, so pi's own ambient discovery loads whatever you have, list or no list.

What the key does is let the linter catch a mismatch. If a state names `:mcp` servers and `"mcp"` is not in `:pi-extensions`, `loop validate` errors with _state `{id}` names MCP servers, but `mcp` is not in `:pi-extensions` — the stage would be told to call a tool it does not have_. Declare what you have installed and the lint is worth something; leave it stale and it isn't.

### Keys that were removed or renamed

These fail the load by name rather than as a generic unknown field, because each one used to do something and an author deserves to be told what replaced it.

| Key | Error says |
| --- | --- |
| `:playbook` (on a state) | **Renamed** to `:stage-prompt`, and `.loop/playbooks/` is now `.loop/stage-prompts/`. It still means the same thing: a bare name, or a path if it contains `/`. The inline-text key is still `:prompt`. |
| `:context` | Removed. It took `"digest"` or `"full"`, and `"full"` was never wired to anything. The rolling digest is the only continuity channel between stages: interpolate `$LEDGER_DIGEST` in a stage prompt and tune `:digest-last-n`. |
| `:default-skills` | Write `:defaults {:skills [..]}`. |
| `:default-mcp` | Write `:defaults {:mcp [..]}`. |
| `:transition-mode` | Removed. A Worker ends its stage by writing `$LOOP_HANDOFF`, and the harness checks the target against the graph either way; an off-graph target routes to the Navigator, which is what `open` meant and is now the only behavior. |
| `:when` (on a transition) | Removed. Express the condition as a `:check` the harness runs, or as `:criteria` for the Judge. |

`:playbook` is the only rename in that table, and it gets an error of its own precisely because it is one. A removed key can be reported generically and the author loses nothing; a renamed key reported as "unknown field `playbook`, expected one of `stage-prompt`, `prompt`, …" offers two plausible replacements, and picking `:prompt` is not a load error — it is a stage that runs the string `"qa"` as its entire prompt. The name changed because "playbook" said nothing about the one property that matters: this file is in the stage's context whether the model wants it or not. See [stage prompts and skills](#stage-prompts-and-skills-are-not-the-same-thing).

The three `config.fnl` keys are the shape of a change worth knowing about: there used to be a second authored file, `config.fnl`, in a second directory, holding `:provider`, `:worker`, `:judge`, `:navigator`, `:default-skills`, `:default-mcp`, `:pi-extensions`, `:budgets`, and `:digest-last-n`. Every one of those was a value a machine could already override, so they are machine keys now — with the two `default-*` lists folded into the `:defaults` a machine already had — and the file is gone.

### States

`:states` is a table of state id → state table.

| Key | Required | Type | Effect |
| --- | --- | --- | --- |
| `:stage-prompt` | one of the two | string | A bare name resolved in `.loop/stage-prompts/`, or a path if it contains `/`. |
| `:prompt` | one of the two | string | Inline prompt text. No filesystem access at all. |
| `:model` | no | string | Model override for this stage (layer 1). |
| `:thinking` | no | string | Thinking override for this stage (layer 1). |
| `:provider` | no | string | Provider override for this stage. |
| `:skills` | no | `[string]` | Skill names, unioned with the machine's `:defaults`. |
| `:mcp` | no | `[string]` | MCP server names, unioned the same way. |
| `:description` | no | string | One line on what the stage is for. |

A state needs `:stage-prompt` **or** `:prompt`; with neither you get ``state `{id}`: needs either `:stage-prompt` or `:prompt` ``.

`:description` is not decorative. It is what the Navigator reads in the graph summary when it has to pick a state on the Worker's behalf, and it is the label `loop diagram` draws. A state with no description shows up to the Navigator as `(no description)`, which is exactly as useful as it sounds.

### Transitions

`:transitions` is a list of edge tables. Order matters: for a given `(from, to)` pair, the **first** matching edge is the one taken, and `loop validate` errors on a duplicate rather than letting the second one sit there dead.

| Key | Required | Type | Effect |
| --- | --- | --- | --- |
| `:from` | yes | string | Must be a declared state. |
| `:to` | yes | string | A declared state or a terminal. |
| `:check` | no | string, or `{:cmd :timeout-s}` | A command the harness runs itself. Exit 0 passes. |
| `:criteria` | no | string | What an independent Judge model is asked to decide. |
| `:on-fail` | no | `"retry"` \| `"abort"` \| `{:route "id"}` | Default `"retry"`. What happens when any tier fails. |
| `:backoff-s` | no | int | Seconds to sleep **after** this edge commits. |

What these mean at run time — the tier order, the short-circuit, what the Judge can and cannot see — is [guards](02-how-it-works.md#guards). As keys:

- **`:check`** is the one signal a Worker cannot author, because it runs in the harness's own subprocess after the stage exits. Writing it is covered under [Check commands](#check-commands). An empty string is rejected: ``transitions[N]: `:check` command is empty — omit the key instead``.
- **`:criteria`** is prose, handed to the Judge as its system prompt. Use it for what no exit code decides: "every item in the plan is addressed", "this is a real fix, not a widened assertion".
- **`:on-fail`** applies to the edge, regardless of which tier failed. `"retry"` re-enters the source state at `attempt + 1` and does not consume a transition from the budget. `"abort"` ends the run `Failed`. `{:route "x"}` commits straight to `x` with no guard tiers and no backoff — the usual way to send a failed review back to `implement` with the findings on the ledger.
- **`:backoff-s`** sleeps after the commit event is written, so it survives a crash in the sense that the commit is already durable. It is a blocking sleep in the `loop run` process.
- **`:when` is removed.** The key used to hold a Fennel closure. Using it now is a hard error: ``transitions[N]: `:when` guards were removed — express the condition as a `:check` command the harness runs, or as `:criteria` for the Judge to evaluate``. See [keys that were removed or renamed](#keys-that-were-removed-or-renamed).

An edge with neither `:check` nor `:criteria` is legal and draws a `loop validate` warning: _the worker's proposal is committed unexamined_. Warnings alone still exit 0.

### Loops

`:loops` exists to count cycles and cap them. It does not create edges — the back-edges must already be in `:transitions`.

| Key | Required | Type | Effect |
| --- | --- | --- | --- |
| `:name` | yes | string | Used in the exhaustion error and the `loop diagram` note. |
| `:states` | yes | `[string]` | Non-empty. **`states[0]` is the loop head.** |
| `:max-cycles` | yes | int | Cap on cycles through the head. |
| `:on-exhausted` | no | `"escalate"` \| `"abort"` | Default `"escalate"`. |

**`states[0]` is load-bearing.** The head is the state whose _re-entry_ counts a cycle; the remaining names are documentation of the loop's extent. Two loops may share a head — `{:states ["qa-staging" "debug"]}` and `{:states ["qa-staging"]}` count different things about the same node.

The cap is enforced prospectively at commit time, and only when the target is a head. Exhaustion writes ``loop `{name}` exhausted max_cycles={n} at head `{head}` `` and then escalates or aborts. `loop validate` will also tell you if a declared head is never re-entered by any transition, which usually means you named the wrong element of `:states` first.

[`loop preview`](04-cli-reference.md#loop-preview) prints each loop with the head it resolved to, its member states, its cap, and where exhaustion sends the run — so a head you named in the wrong position is visible as a head, not inferred from a list.

### A complete machine

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

 ;; What you have installed in pi. This does not turn anything on.
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

  ;; Three-way fail routing: a transient flake retries in place with backoff
  ;; and touches no code, a real failure spawns the debugger, a pass moves on.
  ;; Each edge asserts its own branch of one script's taxonomy, so "transient"
  ;; is decided by a versioned regex set and an exit code rather than by a
  ;; tired agent that would rather retry than debug.
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

Run `loop validate` after every edit. It resolves every stage prompt and skill name, walks reachability from `:entry`, checks that each state has a path to a terminal, and catches duplicate edges — the full diagnostic list is in [04-cli-reference.md](04-cli-reference.md#loop-validate). `loop diagram` renders the same machine as a mermaid state diagram if you would rather look at it.

---

## Model resolution

The Worker's model is assembled from four layers, most specific first:

1. The state's own `:model` / `:thinking` / `:provider`
2. The stage prompt's frontmatter `model` / `thinking`
3. The machine's `:defaults`
4. The machine's `:worker`, over loop's built-in floor (`claude-sonnet-5` at `medium`)

Layers are merged **field by field**, not chosen wholesale. A state that sets only `:thinking "high"` still takes its model from whichever lower layer supplies one. A stage prompt whose frontmatter names a `model` but no `thinking` contributes exactly the model.

**Stage prompt frontmatter never supplies a provider.** That layer contributes `model` and `thinking` only; the provider comes from the state, the machine's `:defaults`, the role's own table, or the machine's top-level `:provider` under all of them.

The resolved pair becomes one pi flag:

```
--model claude-sonnet-5:high
```

`model:thinking`, joined by a colon. Thinking levels, lowercase:

`off` · `minimal` · `low` · `medium` · `high` · `xhigh` · `max`

The Judge and Navigator are resolved separately and do not participate in this chain: the machine's `:judge` / `:navigator`, over the built-in floor. No state can change them, which is the point — a stage cannot pick its own grader.

Those two overlays are what the spawn actually gets. They used to be parsed and then dropped — the stage builder read the grader's model off the built-in floor rather than off the machine, so a `:judge {:model "claude-sonnet-5"}` loaded, validated, previewed correctly, and then graded on haiku anyway. It reads the machine now. If you set either key before and never saw the model change, that is why.

Do not merge the four layers in your head. [`loop preview`](04-cli-reference.md#loop-preview) prints the resolved `provider/model:thinking` for every state, computed by the same `resolve_model` the run calls, and `loop preview <state>` adds the `--model` flag pi is handed verbatim.

---

## Stage prompts

A stage prompt is what a stage is told: a markdown file whose body, after `$VAR` substitution, is written to `.loop/run/` and handed to pi as `--append-system-prompt <path>`. One per state, always in that stage's context. The contrast with a skill is [above](#stage-prompts-and-skills-are-not-the-same-thing).

Three ways to name one:

| Form | Behavior |
| --- | --- |
| `:stage-prompt "qa"` | A bare **name**, resolved in `.loop/stage-prompts/`. |
| `:stage-prompt "stage-prompts/one-off.md"` | Contains `/`, so it is a **path** — absolute as-is, otherwise relative to `machine.fnl`'s directory. **No extension is appended**; write the `.md` yourself. |
| `:prompt "…"` | **Inline** text. No filesystem access, no frontmatter, no name resolution. |

A bare name has exactly one candidate, `.md` only:

```
<project>/.loop/stage-prompts/<name>.md
```

A miss names it:

```
could not resolve stage prompt `qa`
  searched: /proj/.loop/stage-prompts/qa.md
```

`loop validate` reports the same miss as _stage prompt for state `{id}` does not resolve in .loop/stage-prompts/_, so you find it before a run burns tokens getting there.

A hit is worth reading back too: [`loop preview`](04-cli-reference.md#loop-preview) names the file each state resolved to, and `loop preview <state>` prints the body it resolved to as well.

### Frontmatter

Optional YAML between `---` fences at the top of the file. Exactly four keys are read:

| Key | Effect |
| --- | --- |
| `name` | Display name. Defaults to the file stem. |
| `description` | Carried along; does not drive the run. |
| `model` | Model override — layer 2 of [model resolution](#model-resolution). |
| `thinking` | Thinking override — same layer. |

Parsing rules that surprise people:

- Frontmatter is recognized **only if line 1 is exactly `---`**. A blank line, a BOM, or a leading comment above it and the whole file is body.
- **Unclosed frontmatter is silently treated as body.** There is no error; you get a prompt that starts with `---` and your YAML as prose.
- Unknown keys are ignored, not rejected.
- Malformed YAML _inside_ a properly closed block does error: ``stage prompt `{name}` has malformed frontmatter: {err}``.

### Template variables

The stage prompt body is rendered with `$UPPER_SNAKE` substitution. This is the complete set of variables — there are no others.

| Variable | Value |
| --- | --- |
| `$TICKET_ID` | The machine's `:ticket`. |
| `$TASK` | Full text of `:task`. |
| `$PLAN` | Full text of `:plan`. |
| `$STATE` | Current state id. |
| `$PREV_STATE` | The `from` of the most recent committed transition. **Empty string** when there is none. |
| `$CYCLE` | Cycle number. |
| `$ATTEMPT` | Attempt number within the cycle. |
| `$CRASHED` | `1` when this entry follows a stage that died mid-flight (a resumed crash, or an in-process retry after the Worker process failed). **Empty** on a clean entry. |
| `$LEDGER_DIGEST` | The rendered rolling digest — totals, the last `:digest-last-n` committed transitions, and every artifact. |
| `$ENTRY_ADDENDUM` | The Navigator's get-back-on-track note for this state. **Empty** when the Navigator did not route here. |
| `$QA_CASES` | Markdown bullets, `- **{id}** — {desc}` per case. **Empty** when `:qa-cases` is absent. |
| `$ARTIFACT_<NAME>` | Project-relative path of a captured artifact. `<NAME>` is the artifact's claimed name, uppercased: a claim named `diff` becomes `$ARTIFACT_DIFF`. |

The same map is used for `:check` command strings — see [Check commands](#check-commands).

> **The variables only reach the agent where you interpolated them.** There is no automatically prepended context header. A stage prompt that never writes `$TASK` gives the agent no task. A stage prompt that never writes `$LEDGER_DIGEST` gives it no memory of the previous six stages. The positional message pi is spawned with contains no ticket id, task, plan, or digest — only "you are entering **X**, cycle N" and, when the stage names servers, the MCP connect instructions. Everything else is in the file you wrote.

`loop preview <state>` answers that directly: it lists the variables the body **actually writes**, split from the `$NAME`s that will pass through untouched, and then renders the body so you can read the result. The render is representative, not exact — it uses cycle 1, attempt 1, no previous state, no artifacts, and an empty digest, because everything else depends on where the run has already been. Which variables are wired in is exact; what they will contain is not.

Substitution rules:

- **Maximal munch.** At each `$` the whole following `[A-Za-z_][A-Za-z0-9_]*` run is consumed before any lookup, so `$ARTIFACT_DIFF_PATH` is looked up as one token and can never be truncated to `$ARTIFACT_DIFF` with a dangling `_PATH`.
- **Unknown names pass through untouched.** `$HOME`, `$1`, and shell snippets in a fenced block survive verbatim.
- **`$$` is a literal `$`.**
- No `${...}` braces, no conditionals, no loops. It is pure textual substitution — there is no expression language here.

`$CRASHED` is the one variable worth branching on: a stage that opens a PR, posts a comment, or kicks a deploy is re-run from the top after a crash, and this is how it knows to look for its own half-finished work first. `$ATTEMPT` does not distinguish a crash from a guard failure sending the stage back.

### Separate: the four environment variables

Four scalars are also exported as real environment variables, to both the pi spawn and every `:check` subprocess:

```
TICKET_ID   STATE   CYCLE   ATTEMPT
```

Note the absence of a `$` and of everything else in the table above. `$TASK` and `$LEDGER_DIGEST` are template variables only; nothing in the agent's environment carries them.

---

## Skills

A skill is know-how a stage is _offered_, plus whatever scripts sit beside it. loop resolves a name to a path and passes `--skill <path>` to pi; **loop does not parse skills at all**. The format is entirely pi's business — loop never reads a `SKILL.md`'s contents, only checks that the file exists.

That last sentence has two consequences worth stating before the mechanics. A skill's body is **not** rendered, so a `$TASK` written in one arrives as the literal five characters. And a skill's frontmatter cannot set a model or thinking level — those keys are read off a stage prompt, which belongs to a state and therefore has a model to influence; on a skill they are silently inert. See [above](#stage-prompts-and-skills-are-not-the-same-thing).

A name containing `/` is an exact path: absolute as-is, otherwise relative to `machine.fnl`'s directory. A bare name has two candidates, in order:

1. `<project>/.loop/skills/<name>/` — a directory, **counted only if it contains `SKILL.md`**
2. `<project>/.loop/skills/<name>.md`

The directory form wins when both exist: a `SKILL.md` with scripts beside it is the richer thing, and silently preferring the bare file would drop the scripts. The `SKILL.md` rule exists so an empty `skills/foo/` directory fails loudly at `loop validate` instead of resolving and then loading nothing at run time.

The effective skill list for a stage is the **order-preserving deduplicated union** of two lists:

```
machine :defaults :skills  +  state :skills
```

There is no exclude list and no subtraction. Withholding a skill hides know-how; it does not revoke a capability, because the Worker keeps pi's built-in tools regardless. See [what a stage can do](02-how-it-works.md#worker).

A stage prompt `.md` can double as a skill, but **only through the path form** — `:skills ["stage-prompts/review.md"]`, not `:skills ["review"]`, because a bare name is looked for under `.loop/skills/` and nowhere else. Naming the same `review.md` that a state uses as its stage prompt is a reasonable thing to do when the review procedure is worth _offering_ to a second stage; note the asymmetry that makes it work in that direction only. The file is rendered where it is the stage prompt and handed over raw where it is the skill, so a `$VAR` in it will interpolate in one stage and appear literally in the other.

> `loop validate` checks the whole union, not just the names the state writes, because the union is what a spawn actually loads. A name that came from `:defaults` says so in the diagnostic.

To read the union rather than assemble it from two places, [`loop preview`](04-cli-reference.md#loop-preview) prints each state's effective skills with the path each name resolved to; `loop preview <state>` lists them as the `--skill` arguments pi receives.

---

## MCP servers

`:mcp ["warehouse"]` names a server in **your own** `~/.pi/agent/mcp.json`. loop never reads, ships, writes, or validates that file — it only carries names. `PI_AGENT_DIR` is deliberately not set on the spawn, precisely so pi's `mcp` extension finds your config rather than something loop invented.

The effective list is the same union as skills:

```
machine :defaults :mcp  +  state :mcp
```

**How the names reach the agent.** They are not a flag. Every session starts with every server _disconnected_, and the only way in is the agent connecting one, so loop leads the stage's entry message with instructions:

> Before anything else, connect the MCP servers this stage needs — they start the session disconnected, and `mcp({connect: "…"})` is what turns one on:
>
> - `mcp({connect: "warehouse"})`
>
> If one fails to connect, say so in your handoff rationale rather than working around it.

One bullet per named server, then the ordinary `You are entering **X**, cycle N.` line. When a stage names no servers, the entry message says nothing about MCP at all.

Two consequences:

- **A stage that does not name a server cannot reach it**, because nothing told the agent to connect it.
- **A name that exists nowhere fails at connect time, not at load time.** loop has nothing to check it against, so `loop validate` cannot tell a typo from a server you have not installed on this machine.

The one MCP diagnostic validate does emit is the `:pi-extensions` mismatch: naming servers on a state while `"mcp"` is absent from the list errors with _the stage would be told to call a tool it does not have_.

Since a typo cannot be caught, read the names back: [`loop preview`](04-cli-reference.md#loop-preview) shows the effective server list per state, and `loop preview <state>` renders the connect instructions exactly as they will appear in that stage's entry message. It never connects to anything — the names are reported, not tested.

---

## Check commands

A `:check` is a command the **harness** runs, in its own subprocess, after the stage's agent has exited. It is the one signal on the ledger that never passed through the Worker's session, which is why a failed check is not appealable to the Judge — the Judge is never even spawned.

How one executes:

- **`bash -c <cmd>`.** Specifically bash, not `sh` and not your login shell. No profile is sourced.
- **cwd is the project directory** — the directory `loop run` targets, not `.loop/`.
- **stdin is null.** A check that waits for input hangs until its timeout.
- **Exit 0 passes.** A non-zero exit is an ordinary failure, not a harness error; it routes through the edge's `:on-fail`.
- **`$VAR` substitution happens before execution**, over the full [template variable](#template-variables) table. `$TICKET_ID`, `$CYCLE`, and `$STATE` are the useful ones; `$TASK` and `$LEDGER_DIGEST` will interpolate a whole document into your command line if you let them.
- **Four of those are also real environment variables** in the subprocess: `TICKET_ID`, `STATE`, `CYCLE`, `ATTEMPT`. Prefer `"$CYCLE"` inside a script you invoke; the template form is for the command string itself.
- **stdout and stderr are merged** into one capture and kept.
- **Timeout defaults to 120 seconds.** Override per edge with the table form. On timeout the process is killed, the exit code is recorded as absent, and `\n[check timed out after {N}s]` is appended to the output. A timeout is a failure.
- **Output is truncated to the last 16 KiB**, prefixed `[… N earlier bytes truncated …]`. The tail is what a human or a Judge reads, so a check may print a whole build log without consequence.
- **The output is shown to the Judge** when the same edge also has `:criteria`, under a line that tells the Judge the harness produced it and that it exited zero. Design checks so their output is worth reading, not just their exit code.

The two forms:

```fennel
;; Bare string — the common case.
{:from "implement" :to "review"
 :check "bash .loop/skills/spark-build/build.sh"}

;; Table form, when 120s is not enough.
{:from "debug" :to "qa-staging"
 :check {:cmd "bash .loop/skills/spark-run/run.sh" :timeout-s 900}}
```

A pattern worth stealing from the [worked example](../examples/): the same script appears both in a state's `:skills` and in the `:check` on the edge out of it. The agent and the harness run identical code, so an agent cannot pass a gate the harness would fail. And when one script classifies a failure, each outgoing edge can assert its own branch of that classification:

```fennel
{:from "qa-staging" :to "qa-staging"
 :check "bash .loop/skills/spark-run/classify.sh --expect transient"
 :backoff-s 30
 :on-fail "abort"}
{:from "qa-staging" :to "debug"
 :check "bash .loop/skills/spark-run/classify.sh --expect real"}
```

"Transient" is then decided by a versioned regex set and an exit code rather than by a tired agent that would rather retry than debug.

Empty is an error, not a no-op: ``transitions[N]: `:check` command is empty — omit the key instead``.

[`loop preview`](04-cli-reference.md#loop-preview) lists every edge with its check command, its effective timeout, its criteria, its `:on-fail` action, and its backoff — which is how you notice that the check on the way out of `test` is still the commented-out one the template shipped with. Preview never runs a check; it prints the command string as authored, before `$VAR` substitution.

---

## Reuse across tickets

`loop init <TICKET>` with no flags writes the bundled `standard-ticket` machine, its four stage prompts, and the one skill it names. `loop init <TICKET> --from <DIR>` **copies** a `.loop/`-shaped directory instead:

```
loop init PROJ-1502 --from ~/loop-kits/data-pipeline
```

`<DIR>` must hold a `machine.fnl`; a leading `~/` is expanded. Everything under it — stage prompts, skills, prose, whatever you keep there — is copied into the new `.loop/`, and files that already exist are never overwritten.

**A copy, not a lookup.** What you started from is recorded in the ticket rather than resolved at run time, so editing the source afterwards cannot change a run already in flight. The flip side is the one stated under [names resolve in one place](#names-resolve-in-one-place): fixing a stage prompt in your kit does not fix it in the twelve tickets already copied from it. You re-copy, or you edit the ticket in front of you.

**`$TICKET` is scaffold-time text.** `loop init` does a plain string replacement of `$TICKET` with the ticket argument in the copied `machine.fnl`, and in `task.md` / `plan.md` when it writes them itself. This is _not_ the `$VAR` render engine and _not_ the runtime `$TICKET_ID` — it happens once, at scaffold, and the resulting file contains the literal ticket. Only `$TICKET` is replaced; every other `$NAME` survives to be a real template variable later.

`loop init` refuses to overwrite an existing machine: `{path} already exists — delete .loop/ to start a new ticket`.

### Building a kit

Copy a ticket directory you have already tuned, and generalize it:

```
cp -R .loop ~/loop-kits/data-pipeline
```

Then replace the ticket id with `$TICKET`, delete `ledger.jsonl`, `artifacts/`, and `run/`, replace ticket-specific `:qa-cases` descriptions with the shape of the question rather than this ticket's answer, and mark the lines you always end up editing — an `EDIT` comment is a cheap convention worth copying.

### When a new kit is actually warranted

The bar is not "this ticket had one more step". A sequential stage is a two-line edit to `machine.fnl` in the ticket that needs it, and keeping a kit costs you a directory to maintain forever.

**The bar is a state you would have to _re-enter_** — a genuine ambiguity the graph has to resolve by looping rather than by proceeding. The canonical case is transient-vs-real failure: when a stage can fail in two ways that demand different responses, and no single guard can tell them apart in one pass, you need a head, a back-edge, a cap, and a classification check. That is a shape, not a step, and shapes are what a kit is for.

Everything else — a different build command, an extra QA case, a one-off stage prompt — belongs in the ticket's own `.loop/`, where it costs nothing when the ticket is deleted.
