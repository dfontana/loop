# Recipes: interviewing, shaping, triaging

## The interview

Most of what a machine needs is in the repository. Ask the user only for what is genuinely in their head, and tell them what you assumed for the rest.

**Read from the repo, do not ask:**

| Needed | Where to get it |
| --- | --- |
| The build/test/lint command → `:check` | `mise.toml`, `Makefile`, `justfile`, `package.json` scripts, `Cargo.toml`, CI workflow files, `CONTRIBUTING.md`, the project's agent instructions file |
| Whether a `.loop/` already exists | `loop doctor`, or list `.loop/` |
| A kit worth copying | ask once whether they keep one; otherwise use the bundled machine |
| Ticket id | the branch name, or the user's prose |

**Ask the user (short list, in one message):**

1. **The task** — what to change and what "done" means. One paragraph is enough; you will write `task.md` from it.
2. **The plan** — if they have one, take it; if not, offer to draft a numbered checklist from the task and have them approve it. The `implement → review` gate is usually "every item in the plan is addressed", and a Judge can only check that against a list.
3. **The command that proves it works.** This is the highest-value answer in the interview. It becomes a `:check`, which is the only gate a Worker cannot talk its way past.
4. **Anything the loop must not do** — don't push, don't touch migrations, don't open a PR. These become explicit lines in the relevant stage prompt.
5. **Budget appetite**, only if they seem cost-sensitive. Otherwise take the bundled `$8 / 5400 s / 40 transitions` and say so.

Do **not** ask about models, thinking levels, providers, `:digest-last-n`, or `:pi-extensions`. Take the defaults and mention them once.

## Turning the answers into a machine

| The user said | What it becomes |
| --- | --- |
| "run `cargo test`" | `:check "cargo test --quiet"` on the edge out of the test stage |
| "make sure it actually addresses the plan" | `:criteria` on the edge out of `implement` |
| "it fails sometimes for no reason" | a classifier script + a self-edge with `:backoff-s`, capped by a `:loops` entry |
| "have it open a PR at the end" | an `open-pr` state whose prompt branches on `$CRASHED`, and a `:criteria` that a PR exists with a populated body |
| "don't let it run forever" | tighten `:budgets`, and set `:max-attempts` on any edge whose check could be structurally impossible to pass |
| "I want to review before it ships" | make the last state's only outgoing edge point at a terminal that is _not_ `done`, or stop the machine at `open-pr` |
| "it keeps wandering off" | shorter `task.md`, a tighter `:criteria`, and an explicit "do not do X" paragraph in the stage prompt |

## Shapes worth copying

### 1. The bundled spine — implement → review → test → open-pr

What `loop init` writes. Two back-edges make it a loop rather than a pipeline: a failed review or a failed test routes back to `implement` with the findings on the ledger, and a `fix` loop caps how many times that can happen.

```fennel
:entry "implement"
:terminals ["done" "blocked"]
:escalation-state "blocked"

:transitions
[{:from "implement" :to "review"
  :criteria "Every item in the plan is addressed in the diff, the build is green, and no TODO/FIXME markers remain in the changed files."
  :on-fail "retry"}

 {:from "review" :to "test"
  :criteria "The review found no blocking defects: no correctness bugs, no missing error handling on the changed paths, and no unaddressed findings from a previous cycle."
  :on-fail {:route "implement"}}

 {:from "test" :to "open-pr"
  :check "cargo test --quiet"          ; ← the single highest-value edit to this template
  :criteria "The test suite was actually run in this stage (the output is present, not asserted), and it passed."
  :on-fail {:route "implement"}}

 {:from "open-pr" :to "done"
  :criteria "A pull request exists for this branch with a populated description."}]

:loops
[{:name "fix" :states ["implement" "review" "test"] :max-cycles 4 :on-exhausted "escalate"}]
```

The template ships that `:check` commented out. **Swapping in the command that actually runs the suite is the single highest-value edit you can make to any machine**, because it converts the gate from "the stage says it passed" into "the harness ran it".

### 2. Transient vs. real — one classifier, three edges

The shape that actually justifies a loop. A stage can fail in two ways that demand different responses, and no single guard can tell them apart in one pass. One script classifies; each outgoing edge asserts its own branch of that taxonomy, so "transient" is decided by a versioned regex set and an exit code rather than by a tired agent that would rather retry than debug.

```fennel
{:from "qa" :to "qa"                                    ; self-edge: retry in place
 :check "bash .loop/skills/run/classify.sh --expect transient"
 :backoff-s 30
 :on-fail "abort"}
{:from "qa" :to "debug"                                 ; real failure: spawn the debugger
 :check "bash .loop/skills/run/classify.sh --expect real"}
{:from "qa" :to "ship"                                  ; pass: move on
 :check "bash .loop/skills/run/classify.sh --expect pass"
 :criteria "The output sample satisfies every QA case, not just the job's exit status."}

:loops
[{:name "qa"           :states ["qa" "debug"] :max-cycles 4 :on-exhausted "escalate"}
 {:name "qa-transient" :states ["qa"]         :max-cycles 3 :on-exhausted "escalate"}]
```

Two loops sharing a head count different things about the same node: total QA cycles, and consecutive transient retries.

### 3. The same script as skill and as check

Put the script in a skill directory, name the skill on the state, and point the edge's `:check` at the same file. The agent and the harness then run identical code, so an agent cannot pass a gate the harness would fail.

```
.loop/skills/build/SKILL.md
.loop/skills/build/build.sh
```

```fennel
:states {:implement {:stage-prompt "implement" :skills ["build"] :description "…"}}
:transitions [{:from "implement" :to "review" :check "bash .loop/skills/build/build.sh"}]
```

### 4. A one-off stage without a file

For a stage whose prompt is three sentences and will never be reused, skip the file:

```fennel
:tidy {:prompt "Run the formatter and the linter. Change nothing else. Write your handoff naming `review` when the tree is clean."
       :thinking "low"
       :description "Format and lint; no behavior changes."}
```

`:prompt` gets no `$VAR` substitution problems because there are no variables to forget — but it also cannot carry `$TASK` or `$LEDGER_DIGEST`. Use it only where the stage genuinely needs no context.

### 5. A human gate at the end

To stop short of shipping, make the last stage's target a terminal that is not the escalation state, and let the run report `Done` there:

```fennel
:terminals ["ready-for-human" "blocked"]
:escalation-state "blocked"
:transitions [{:from "open-pr" :to "ready-for-human"
               :criteria "A draft pull request exists with a populated description and the CI run is green."}]
```

## Editing a machine that already exists

Before touching it: `loop status` (is a run in flight?) and `loop preview` (what does it actually resolve to?).

**A machine edited mid-run breaks the recap.** `loop recap` uses the machine only when its hash still matches the one recorded on `run_started`; after an edit it drops state descriptions and folds machine-agnostically. If a run is in flight and the user wants a change, finish or abandon the run first.

Common edits:

| Want | Do |
| --- | --- |
| Add a stage | Add the state, write `.loop/stage-prompts/<name>.md`, add the edges **in and out**, and check the loop membership. `loop validate` catches an unreachable state or one with no path to a terminal. |
| Make a gate real | Move the fact out of `:criteria` and into `:check`. |
| Stop a stage thrashing | Set `:max-attempts 1` or `2` on the failing edge, or convert `:on-fail "retry"` to `{:route "implement"}`. |
| Cheapen a run | Lower `:thinking` on the stages that do not need it (`open-pr` at `"low"` is free money), tighten `:budgets`, and cut `:criteria` from edges a `:check` already settles. |
| Give a stage know-how | A skill under `.loop/skills/`, named in that state's `:skills`. Not a longer stage prompt. |
| Give every stage the same thing | `:defaults {:skills [...]}` — it unions with each state's own. |

After **every** edit: `loop validate`, then `loop preview`. Both are free and neither spawns anything.

## Triage: a run that went wrong

Start with `loop recap`. It is deterministic, labels every claim by author, and includes failed attempts. Then:

| Symptom | Likely cause | Where to look |
| --- | --- | --- |
| A stage acts as though it does not know the task or plan | The stage prompt never interpolated `$TASK` / `$PLAN` | `.loop/run/<state>-<cycle>-<attempt>-system.md` — the prompt actually sent. Or `loop preview <state>`, which lists the variables the body writes. |
| The same stage ran over and over, no transitions committed | A `:check` that cannot pass (missing tool, wrong cwd, wrong path) under `:on-fail "retry"` | `loop logs --raw \| jq 'select(.type=="guard_checked")'` for the check output. Remember: checks run with cwd = **project root**, not `.loop/`. |
| The run escalated with no obvious failure | Navigator cap hit, edge selection found no matching transition, `:max-attempts` exhausted, or 3 consecutive Worker crashes | The last fatal `error` in the recap's "Why it ended". |
| The Judge rejected work that looks fine | The Judge sees only the Worker's summary, artifact paths, and the check output — it cannot open a file | Widen what the check prints, or attach the evidence as an artifact and reference it in the criteria. Also check the criteria is answerable from that evidence. |
| The Judge passed work that is wrong | The criteria was answerable from a claim rather than from evidence | Convert it to a `:check`. Criteria is for the fuzzy remainder only. |
| "judge returned no usable verdict" | The Judge model drifted off the first-line contract, or exited non-zero | The fallback quotes what it actually said. Consider a stronger `:judge {:model ...}`. |
| A stage did something twice (two PRs, two comments) | The stage crashed and re-ran; it was not idempotent | Branch the stage prompt on `$CRASHED`, and make the action check-before-act. |
| The run aborted early on cost or time | Budget breach — always `Aborted`, never `Failed` | `loop status --json`. Note budgets are sampled **between** stages, so one long spawn can overshoot. |
| An artifact is missing downstream | The claim was dropped — path never written, or it escaped the project root | The `error` event naming the claim. Also: **the extension is not preserved**, so name the claim `diff.patch` if you need one. |
| `loop run` refuses to start | The ledger already has a run | `loop resume`, or delete `.loop/ledger.jsonl` to start over. |

To read what a Worker actually did, rather than what it said:

```sh
loop sessions                       # every attempt, oldest first
loop session --latest implement     # newest attempt at that stage
loop sessions | fzf | awk '{print $6}' | xargs loop session
```

## What to hand back to the user

After setting a machine up, report in this order:

1. The graph, in one line each: `implement → review → test → open-pr → done`, plus the back-edges and the loop cap.
2. **Which edges are gated by a real command** and which rest on a Judge. Name the commands.
3. The budgets in force.
4. Anything you assumed rather than asked.
5. The exact command to start it: `loop run` — and that `loop recap` afterwards writes the ticket up.

Do not run it for them unless they ask. It spends money.
