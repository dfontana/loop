# Customizing a loop

This is the reference for shaping a loop to your work: where configuration lives, what every key does, and what surfaces you can reach from a stage.

It defines what the keys _mean_. How they are evaluated at run time — the run loop, the three roles, the injected tools, guard ordering, the ledger — is [02-how-it-works.md](02-how-it-works.md). Flags, environment variables, and exit codes are [04-cli-reference.md](04-cli-reference.md). Why any of it is shaped this way is [05-design-notes.md](05-design-notes.md).

---

## Where configuration lives

Two locations, with different lifetimes.

**The toolbox — `~/.config/loop/`.** Portable, hand-authored, reused across every ticket. Nothing in it is ticket-specific.

```
config.fnl                 global defaults (models, budgets, skills, MCP)
playbooks/<name>.md        stage prompts, referenced by bare name
skills/<name>/SKILL.md     a skill as a directory
skills/<name>.md           a skill as a single file
machines/<name>.fnl        machine templates for `loop init --template`
ext/                       transition-tool.ts, verdict-tool.ts, choose-tool.ts
```

`ext/` is written for you. The three `.ts` files are compiled into the `loop` binary and materialized on `loop init`; loop rewrites any of them whose on-disk sha256 does not match the vendored content, so a hand-edited copy gets reverted.

**The ticket — `<project>/.loop/`.** Created by `loop init`, thrown away when the ticket is done.

```
machine.fnl                the ticket machine — the only required file
task.md                    prose, referenced by :task
plan.md                    prose, referenced by :plan
playbooks/                 local playbooks; win over the toolbox on a name clash
skills/                    local skills; win over the toolbox on a name clash
ledger.jsonl               the append-only run log
artifacts/                 snapshots, named <state>-<cycle>-<name>
```

`loop init` creates `machine.fnl`, `task.md`, `plan.md`, and an empty `playbooks/`. It does **not** create `skills/`, `artifacts/`, or `ledger.jsonl` — those appear when something needs them.

**The state directory — `~/.local/state/loop/`.** Generated and disposable. Deleting it loses nothing you authored.

```
render/<sanitized-ticket>/<state>-<cycle>-<attempt>-system.md
```

Each file is the fully-substituted playbook body actually handed to pi as `--append-system-prompt` — the single most useful artifact when a stage misbehaves. `<sanitized-ticket>` maps every character outside `[A-Za-z0-9_-]` to `-`.

All three roots have environment overrides (`LOOP_CONFIG_DIR`, `LOOP_STATE_DIR`, `-C/--dir`); see [04-cli-reference.md](04-cli-reference.md#environment-variables).

> **The ledger is not in the state directory.** `LOOP_STATE_DIR` moves rendered prompts and nothing else. `ledger.jsonl` is always `<project>/.loop/ledger.jsonl`, because it belongs to the ticket, not to the machine you happen to be running on.

### Local-first resolution

Playbooks and skills are named, not pathed, and a name resolves **local first**: `<project>/.loop/` is searched before `~/.config/loop/`. That is the whole override mechanism. To specialize a generic `qa` playbook for one ticket, drop `.loop/playbooks/qa.md` next to the machine; the toolbox copy is untouched and every other ticket still gets it. The exact candidate lists are under [Playbooks](#playbooks) and [Skills](#skills).

---

## `config.fnl` — global defaults

`~/.config/loop/config.fnl` evaluates to a Fennel table. Every key is optional; an absent key keeps its built-in default. Kebab-case keys map to snake_case internally (`:max-invocations` → `navigator_max_invocations`).

| Key | Type | Default | Effect |
| --- | --- | --- | --- |
| `:provider` | string | `"anthropic"` | The provider every role falls back to. A role table naming its own wins; otherwise this is what pi receives. |
| `:worker` | `{:model :thinking :provider}` | `claude-sonnet-5` / `medium` / `anthropic` | Base of the Worker model chain — the last layer, filled in only where nothing more specific spoke. |
| `:judge` | same | `claude-haiku-4-5` / `low` / `anthropic` | The Judge model. Not layered — a machine may overlay it, a state may not. |
| `:navigator` | same, plus `:max-invocations` | `claude-haiku-4-5` / `low` / `anthropic`, cap `5` | The Navigator model, and how many times it may fire. |
| `:default-skills` | `[string]` | `[]` | Skills loaded into **every** stage, ahead of the machine's and the state's. |
| `:default-mcp` | `[string]` | `[]` | MCP servers named in every stage. |
| `:pi-extensions` | `[string]` | `["mcp" "review-model-selector"]` | A declaration of what you have installed. Drives one `loop validate` diagnostic — see below. |
| `:budgets` | `{:usd :wallclock-s :max-transitions}` | `15.0` / `7200` / `60` | Hard stops the harness enforces between stages. |
| `:digest-last-n` | int | `8` | How many recent committed transitions the digest lists. |
| `:transition-mode` | `"constrained"` \| `"open"` | `"constrained"` | The schema of the injected `transition` tool's `to` parameter. |

`:navigator {:max-invocations N}` is a cap that applies **both** run-wide and per source state; hitting either escalates instead of spawning. See [the Navigator](02-how-it-works.md#navigator).

The pi binary is **not** configurable here. It comes from `LOOP_PI_BIN` (default `pi`) and `config.fnl` cannot set it.

A realistic file:

```fennel
;; ~/.config/loop/config.fnl — global toolbox defaults.

{:provider "anthropic"

 ;; The Worker does the stage work; a machine or state overrides this.
 :worker {:model "claude-sonnet-5" :thinking "medium"}

 ;; The two cheap agents that guard and reroute. Deliberately small.
 :judge     {:model "claude-haiku-4-5" :thinking "low"}
 :navigator {:model "claude-haiku-4-5" :thinking "low"
             ;; Cap reconciliations so a stuck run escalates instead of
             ;; ping-ponging between two states.
             :max-invocations 5}

 ;; Usually empty: skills and servers are situational, so they belong on the
 ;; states that need them rather than on everything.
 :default-skills []
 :default-mcp []

 ;; What you have installed. This does not turn anything on.
 :pi-extensions ["mcp" "review-model-selector"]

 ;; Hard stops. Not suggestions to the agent.
 :budgets {:usd 15 :wallclock-s 7200 :max-transitions 60}

 :digest-last-n 8

 :transition-mode "constrained"}
```

### `:pi-extensions` is a declaration, not a switch

loop never turns this list into a command-line flag, because pi has none to turn: there is no way to enable an _installed_ extension by name. The Worker is simply spawned without `--no-extensions`, so pi's own ambient discovery loads whatever you have, list or no list.

What the key does is let the linter catch a mismatch. If a state names `:mcp` servers and `"mcp"` is not in `:pi-extensions`, `loop validate` errors with _state `{id}` names MCP servers, but `mcp` is not in `:pi-extensions` — the stage would be told to call a tool it does not have_. Declare what you have installed and the lint is worth something; leave it stale and it isn't.

**`:context` was removed.** It took `"digest"` or `"full"`, and `"full"` was never wired to anything. A config that still sets it fails to load, with a pointer to `$LEDGER_DIGEST` and `:digest-last-n` — the rolling digest is the only continuity channel between stages, and it only reaches an agent where a playbook interpolates it.

---

## `machine.fnl` — the ticket machine

`<project>/.loop/machine.fnl` evaluates to a table describing one ticket's state graph. This is the file you edit per ticket. Fennel here is a configuration surface, not a scripting hook: it evaluates once, to a plain table, and there are no callbacks into the machine at run time.

### Top-level keys

| Key | Required | Type | Notes |
| --- | --- | --- | --- |
| `:ticket` | yes | string | Identifies the run. Becomes `$TICKET_ID` and the `TICKET_ID` env var. |
| `:task` | yes | string | A path relative to `machine.fnl`, read into `$TASK` — or inline prose. See the `.md` rule below. |
| `:plan` | yes | string | Same, into `$PLAN`. |
| `:qa-cases` | no | `[{:id :desc}]` | Both fields required per entry. Renders to `$QA_CASES`. |
| `:defaults` | no | `{:provider :model :thinking :skills :mcp}` | Sits under every state, over `config.fnl`. |
| `:budgets` | no | `{:usd :wallclock-s :max-transitions}` | May only **tighten** the config's — per field, the smaller value wins. |
| `:judge` | no | `{:model :thinking :provider}` | Overlays the config's Judge. |
| `:navigator` | no | same, plus `:max-invocations` | Overlays the config's Navigator. |
| `:entry` | conditional | string | Must name a declared state. See below. |
| `:terminals` | yes | `[string]` | Terminal names. These are **not** states — they have no playbook and never spawn an agent. |
| `:escalation-state` | no | string | Must name a declared state or terminal. |
| `:transition-mode` | no | `"constrained"` \| `"open"` | Overrides the config's. |
| `:states` | yes | table | At least one entry. |
| `:transitions` | no | list | Absent means no edges, which `loop validate` will reject as unreachable/terminal-less. |
| `:loops` | no | list | Cycle counting and caps. |

Sharp edges worth knowing before you edit:

- **`:task` / `:plan` ending in `.md` that does not resolve is a hard error, not a fallback.** The value is first tried as a path relative to `machine.fnl`. If that file exists, its contents become `$TASK`/`$PLAN`. If it does not exist _and_ the value ends in `.md`, loop fails with ``could not resolve task `task.md` `` followed by the path it tried, on the assumption you meant a file and mistyped it. Any other non-resolving string is taken as inline prose, which is what makes `:task "Bump the timeout to 30s."` work for a throwaway ticket.
- **`:entry` is only inferable when there is exactly one state.** Omit it with one state and that state is the entry. Omit it with more and you get ``missing `:entry` and `:states` has N entries; ambiguous which one starts the machine``. Naming a state that does not exist gives `` `:entry` `x` is not a declared state ``.
- **Terminals are not states.** `:terminals ["done" "blocked"]` declares two names a transition may point `:to`; neither has a playbook. Entering one ends the run.
- **`:escalation-state` is committed to directly**, bypassing edge selection and every guard tier. If it is also a terminal, the run reports `Failed` rather than `Done`. With no escalation state configured, an escalation ends the run `Aborted`. See [escalation](02-how-it-works.md#escalation).
- **`:budgets` can only tighten.** Writing `:usd 100` under a config of `15.0` leaves you with `15.0`. Machines are for narrowing, not for raising the roof.

### States

`:states` is a table of state id → state table.

| Key | Required | Type | Effect |
| --- | --- | --- | --- |
| `:playbook` | one of the two | string | A bare name resolved through the toolbox, or a path if it contains `/`. |
| `:prompt` | one of the two | string | Inline prompt text. No filesystem access at all. |
| `:model` | no | string | Model override for this stage (layer 1). |
| `:thinking` | no | string | Thinking override for this stage (layer 1). |
| `:provider` | no | string | Provider override for this stage. |
| `:skills` | no | `[string]` | Skill names, unioned with the machine's and the config's. |
| `:mcp` | no | `[string]` | MCP server names, unioned the same way. |
| `:description` | no | string | One line on what the stage is for. |

A state needs `:playbook` **or** `:prompt`; with neither you get ``state `{id}`: needs either `:playbook` or `:prompt` ``.

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
- **`:when` is removed.** The key used to hold a Fennel closure. Using it now is a hard error: ``transitions[N]: `:when` guards were removed — express the condition as a `:check` command the harness runs, or as `:criteria` for the Judge to evaluate``.

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

 ;; Sits under every state and over ~/.config/loop/config.fnl.
 :defaults {:provider "anthropic" :model "claude-sonnet-5" :thinking "medium"
            :skills [] :mcp []}

 ;; May only tighten the global budgets.
 :budgets {:usd 8 :wallclock-s 5400 :max-transitions 40}

 :judge {:model "claude-haiku-4-5" :thinking "low"}
 :navigator {:model "claude-haiku-4-5" :thinking "low" :max-invocations 5}

 :entry "implement"
 :terminals ["done" "blocked"]
 :escalation-state "blocked"
 :transition-mode "constrained"

 :states
 {:implement {:playbook "implement"          ; toolbox
              :thinking "high"
              :skills ["spark-build"]
              :description "Implement the plan; keep the build green."}

  :review {:playbook "review"
           :thinking "high"
           :description "Adversarial review of the diff; find real defects."}

  :qa-staging {:playbook "qa"
               :thinking "high"
               :skills ["staging-deploy" "spark-run"]
               :mcp ["warehouse"]
               :description "Deploy to staging, run the pipeline, grade it."}

  :debug {:playbook "debug-spark"
          :thinking "high"
          :skills ["spark-build" "debug-transient"]
          :description "Diagnose a real pipeline failure and fix it."}

  :validate-contract {:playbook "validate-contract"   ; LOCAL: ./.loop/playbooks/
                      :thinking "medium"
                      :skills ["staging-deploy" "contract-check"]
                      :description "Confirm the API contract matches the OpenAPI schema."}

  :open-pr {:playbook "open-pr"
            :thinking "low"
            :skills ["open-pr"]
            :description "Open or update the pull request for this branch."}}

 :transitions
 [{:from "implement" :to "review"
   :check "bash ~/.config/loop/skills/spark-build/build.sh"
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
   :check "bash ~/.config/loop/skills/spark-run/classify.sh --expect transient"
   :backoff-s 30
   :on-fail "abort"}
  {:from "qa-staging" :to "debug"
   :check "bash ~/.config/loop/skills/spark-run/classify.sh --expect real"}
  {:from "qa-staging" :to "validate-contract"
   :check "bash ~/.config/loop/skills/spark-run/classify.sh --expect pass"
   :criteria "The output sample satisfies every QA case, not just the job's exit status."}

  {:from "debug" :to "qa-staging"
   :check "bash ~/.config/loop/skills/spark-build/build.sh"
   :criteria "A concrete fix to the diagnosed failure was applied — not a retry, a widened assertion, or a disabled check."
   :on-fail "retry"}

  {:from "validate-contract" :to "implement"
   :criteria "The staging response does not match the committed OpenAPI schema."}
  {:from "validate-contract" :to "open-pr"
   :check "bash ~/.config/loop/skills/contract-check/check.sh /accounts/42"}

  {:from "open-pr" :to "done"
   :criteria "A pull request exists for this branch with a populated description."}]

 ;; states[0] is the loop head — the state whose re-entry counts a cycle.
 :loops
 [{:name "qa" :states ["qa-staging" "debug"] :max-cycles 4 :on-exhausted "escalate"}
  {:name "qa-transient" :states ["qa-staging"] :max-cycles 3 :on-exhausted "escalate"}]}
```

Run `loop validate` after every edit. It resolves every playbook and skill name, walks reachability from `:entry`, checks that each state has a path to a terminal, and catches duplicate edges — the full diagnostic list is in [04-cli-reference.md](04-cli-reference.md#loop-validate). `loop diagram` renders the same machine as a mermaid state diagram if you would rather look at it.

---

## Model resolution

The Worker's model is assembled from four layers, most specific first:

1. The state's own `:model` / `:thinking` / `:provider`
2. The playbook's frontmatter `model` / `thinking`
3. The machine's `:defaults`
4. `config.fnl`'s `:worker`

Layers are merged **field by field**, not chosen wholesale. A state that sets only `:thinking "high"` still takes its model from whichever lower layer supplies one. A playbook whose frontmatter names a `model` but no `thinking` contributes exactly the model.

**Playbook frontmatter never supplies a provider.** That layer contributes `model` and `thinking` only; the provider comes from the state, the machine defaults, or the config.

The resolved pair becomes one pi flag:

```
--model claude-sonnet-5:high
```

`model:thinking`, joined by a colon. Thinking levels, lowercase:

`off` · `minimal` · `low` · `medium` · `high` · `xhigh` · `max`

The Judge and Navigator are resolved separately and do not participate in this chain: `config.fnl`'s `:judge` / `:navigator`, optionally overlaid by the machine's. No state can change them, which is the point — a stage cannot pick its own grader.

---

## Playbooks

A playbook is a stage's prompt: a markdown file whose body, after `$VAR` substitution, is written to the state directory and handed to pi as `--append-system-prompt <path>`. One playbook per state.

Three ways to name one:

| Form | Behavior |
| --- | --- |
| `:playbook "qa"` | A bare **name**, resolved through the toolbox. |
| `:playbook "playbooks/one-off.md"` | Contains `/`, so it is a **path** — absolute as-is, otherwise relative to `machine.fnl`'s directory. **No extension is appended**; write the `.md` yourself. |
| `:prompt "…"` | **Inline** text. No filesystem access, no frontmatter, no name resolution. |

A bare name has exactly two candidates, in order, `.md` only:

1. `<project>/.loop/playbooks/<name>.md` — local wins
2. `~/.config/loop/playbooks/<name>.md`

A miss lists everything it tried:

```
could not resolve playbook `qa`
  searched: /proj/.loop/playbooks/qa.md
  searched: /home/u/.config/loop/playbooks/qa.md
```

`loop validate` reports the same miss as _playbook for state `{id}` does not resolve in the toolbox_, so you find it before a run burns tokens getting there.

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
- Malformed YAML _inside_ a properly closed block does error: ``playbook `{name}` has malformed frontmatter: {err}``.

### Template variables

The playbook body is rendered with `$UPPER_SNAKE` substitution. This is the complete set of variables — there are no others.

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

> **The variables only reach the agent where you interpolated them.** There is no automatically prepended context header. A playbook that never writes `$TASK` gives the agent no task. A playbook that never writes `$LEDGER_DIGEST` gives it no memory of the previous six stages. The positional message pi is spawned with contains no ticket id, task, plan, or digest — only "you are entering **X**, cycle N" and, when the stage names servers, the MCP connect instructions. Everything else is in the file you wrote.

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

A skill is instructions plus whatever scripts sit beside them. loop resolves a name to a path and passes `--skill <path>` to pi; **loop does not parse skills at all**. The format is entirely pi's business — loop never reads a `SKILL.md`'s contents, only checks that the file exists.

A name containing `/` is an exact path: absolute as-is, otherwise relative to `machine.fnl`'s directory. A bare name has four candidates, in order:

1. `<project>/.loop/skills/<name>/` — a directory, **counted only if it contains `SKILL.md`**
2. `<project>/.loop/skills/<name>.md`
3. `~/.config/loop/skills/<name>/` — same `SKILL.md` rule
4. `~/.config/loop/skills/<name>.md`

The `SKILL.md` rule exists so an empty `skills/foo/` directory fails loudly at `loop validate` instead of resolving and then loading nothing at run time.

The effective skill list for a stage is the **order-preserving deduplicated union** of three lists:

```
config.fnl :default-skills  +  machine :defaults :skills  +  state :skills
```

There is no exclude list and no subtraction. Withholding a skill hides know-how; it does not revoke a capability, because the Worker keeps pi's built-in tools regardless. See [what a stage can do](02-how-it-works.md#worker).

A playbook `.md` can double as a skill — nothing about the format distinguishes them. `examples/toolbox/playbooks/review.md` being the same file a state names as a skill is a normal thing to do when the review procedure is worth loading into another stage.

> `loop validate` checks the whole union — `:default-skills` from `config.fnl` included — because the union is what a spawn actually loads. A name that came from the global config says so in the diagnostic.

---

## MCP servers

`:mcp ["warehouse"]` names a server in **your own** `~/.pi/agent/mcp.json`. loop never reads, ships, writes, or validates that file — it only carries names. `PI_AGENT_DIR` is deliberately not set on the spawn, precisely so pi's `mcp` extension finds your config rather than something loop invented.

The effective list is the same union as skills:

```
config.fnl :default-mcp  +  machine :defaults :mcp  +  state :mcp
```

**How the names reach the agent.** They are not a flag. Every session starts with every server _disconnected_, and the only way in is the agent connecting one, so loop leads the stage's entry message with instructions:

> Before anything else, connect the MCP servers this stage needs — they start the session disconnected, and `mcp({connect: "…"})` is what turns one on:
>
> - `mcp({connect: "warehouse"})`
>
> If one fails to connect, say so in your `transition` rationale rather than working around it.

One bullet per named server, then the ordinary `You are entering **X**, cycle N.` line. When a stage names no servers, the entry message says nothing about MCP at all.

Two consequences:

- **A stage that does not name a server cannot reach it**, because nothing told the agent to connect it.
- **A name that exists nowhere fails at connect time, not at load time.** loop has nothing to check it against, so `loop validate` cannot tell a typo from a server you have not installed on this machine.

The one MCP diagnostic validate does emit is the `:pi-extensions` mismatch: naming servers on a state while `"mcp"` is absent from the list errors with _the stage would be told to call a tool it does not have_.

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
 :check "bash ~/.config/loop/skills/spark-build/build.sh"}

;; Table form, when 120s is not enough.
{:from "debug" :to "qa-staging"
 :check {:cmd "bash ~/.config/loop/skills/spark-run/run.sh" :timeout-s 900}}
```

A pattern worth stealing from `examples/local/machine.fnl`: the same script appears both in a state's `:skills` and in the `:check` on the edge out of it. The agent and the harness run identical code, so an agent cannot pass a gate the harness would fail. And when one script classifies a failure, each outgoing edge can assert its own branch of that classification:

```fennel
{:from "qa-staging" :to "qa-staging"
 :check "bash ~/.config/loop/skills/spark-run/classify.sh --expect transient"
 :backoff-s 30
 :on-fail "abort"}
{:from "qa-staging" :to "debug"
 :check "bash ~/.config/loop/skills/spark-run/classify.sh --expect real"}
```

"Transient" is then decided by a versioned regex set and an exit code rather than by a tired agent that would rather retry than debug.

Empty is an error, not a no-op: ``transitions[N]: `:check` command is empty — omit the key instead``.

---

## Machine templates

A template is an ordinary machine file living at `~/.config/loop/machines/<name>.fnl`. `loop init <TICKET> --template <name>` copies it to `<project>/.loop/machine.fnl`. The default template is `standard-ticket`, written into the toolbox on first `loop init`.

**`$TICKET` in a template is scaffold-time text.** `loop init` does a plain string replacement of `$TICKET` with the ticket argument across the copied `machine.fnl`, `task.md`, and `plan.md`. This is _not_ the `$VAR` render engine and _not_ the runtime `$TICKET_ID` — it happens once, at scaffold, and the resulting file contains the literal ticket. Only `$TICKET` is replaced; every other `$NAME` in a template survives to be a real template variable later.

`loop init`'s project phase refuses to overwrite: `{path} already exists — delete .loop/ to start a new ticket`.

### Adding your own

Copy a machine you have already tuned, generalize it, drop it in `~/.config/loop/machines/`:

```
cp .loop/machine.fnl ~/.config/loop/machines/data-pipeline-ticket.fnl
```

Then replace the ticket id with `$TICKET`, replace ticket-specific `:qa-cases` descriptions with the shape of the question rather than this ticket's answer, and mark the lines you always end up editing. The shipped `examples/toolbox/machines/data-pipeline-ticket.fnl` marks them with `EDIT` comments, which is a cheap convention worth copying.

### When a new template is actually warranted

The bar is not "this ticket had one more step". A sequential stage is a two-line edit to `machine.fnl` in the ticket that needs it, and templating it costs you a file to maintain forever.

**The bar is a state you would have to _re-enter_** — a genuine ambiguity the graph has to resolve by looping rather than by proceeding. The canonical case is transient-vs-real failure: when a stage can fail in two ways that demand different responses, and no single guard can tell them apart in one pass, you need a head, a back-edge, a cap, and a classification check. That is a shape, not a step, and shapes are what templates are for.

Everything else — a different build command, an extra QA case, a bespoke local playbook — belongs in the ticket's own `machine.fnl`, where it costs nothing when the ticket is deleted.
