# Stage prompts, skills, and what reaches the model

## They are not the same thing

Both are markdown with YAML frontmatter, and a file that works as one will usually parse as the other. The format is where the similarity stops. What separates them is **how each reaches the model**, and settling that decides which one a given piece of text has to be.

|  | Stage prompt | Skill |
| --- | --- | --- |
| Bound to | Exactly one state | Any state that names it, via `:skills` |
| Reaches pi as | `--append-system-prompt <path>` | `--skill <path>` |
| In the stage's context | **Always.** It _is_ the system prompt. | **Only if the model elects to load it**, having seen its name and description. |
| Does loop read it? | Yes — parses frontmatter, substitutes `$VAR`s | **Never.** Checks the path exists and hands it over. |
| Can carry `$TASK`, `$PLAN`, `$LEDGER_DIGEST` | Yes | No. loop never renders one, so the text arrives literally. |
| Can set the stage's model | Yes, via frontmatter | No — silently inert. |
| Can carry scripts | No, one file of prose | Yes — a directory, and `:check` can run the same script |

Two rules fall out, and they settle most authoring questions:

- **Anything the stage must be told is a stage prompt.** "Offered" is not "told". A description of the job, the definition of done, the instruction to write a handoff — none of these can be a skill, because a skill the model chooses not to open is a skill that did nothing.
- **Anything that depends on where the run has been is a stage prompt.** There is no automatically prepended context header, so a `$VAR` is the only way the task, the plan, the digest, or the Navigator's note enters a stage — and a skill is never rendered.

What is left for skills is the good case for them: **situational know-how most runs will not need, plus the scripts that carry it out.** "How to tell a flaky test from a real one" is worth having available and not worth spending context on every stage. That is a skill.

## Stage prompts

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

`loop validate` reports the same miss as _stage prompt for state `{id}` does not resolve in .loop/stage-prompts/_, so you find it before a run burns tokens getting there. `loop preview` names the file each state resolved to; `loop preview <state>` prints the body too.

### Frontmatter

Optional YAML between `---` fences at the top. **Exactly four keys are read:**

| Key           | Effect                                        |
| ------------- | --------------------------------------------- |
| `name`        | Display name. Defaults to the file stem.      |
| `description` | Carried along; does not drive the run.        |
| `model`       | Model override — layer 2 of model resolution. |
| `thinking`    | Thinking override — same layer.               |

Parsing rules that surprise people:

- Frontmatter is recognized **only if line 1 is exactly `---`**. A blank line, a BOM, or a leading comment above it and the whole file is body.
- **Unclosed frontmatter is silently treated as body.** No error; you get a prompt that starts with `---` and your YAML as prose.
- Unknown keys are ignored, not rejected.
- Malformed YAML _inside_ a properly closed block does error.

### What a good stage prompt contains

The bundled ones are the template to follow. In order:

1. **A title line** naming the stage and the ticket — `# Implement — ticket $TICKET_ID, cycle $CYCLE`.
2. **The job in one paragraph**, including what this stage does _not_ do. (`You validate; you do not fix.`)
3. **`$TASK`**, and `$PLAN` where the stage works against it, and `$QA_CASES` where it grades.
4. **`$LEDGER_DIGEST`** under a "Context so far" heading — every stage starts fresh, and this is the only memory of the previous six.
5. **`$ENTRY_ADDENDUM`** on its own line, so a Navigator that routed here can say why.
6. **Numbered "How to work" steps**, ending with: write the handoff naming the next state, with a rationale and any artifacts; and, separately, what to do when genuinely blocked.
7. **An integrity note.** State plainly that the stage's classification is a _proposal_, and that the edges out of it are gated on commands the harness runs itself and on a Judge that never sees the stage's own claim. This measurably reduces optimistic self-grading.

Branch on `$CRASHED` in any stage with an external side effect: it is `1` when this entry follows a stage that died mid-flight, and empty otherwise. `$ATTEMPT` cannot distinguish a crash from a guard failure sending the stage back.

## Template variables

The stage prompt body is rendered with `$UPPER_SNAKE` substitution. **This is the complete set — there are no others.**

| Variable | Value |
| --- | --- |
| `$TICKET_ID` | The machine's `:ticket`. |
| `$TASK` | Full text of `:task`. |
| `$PLAN` | Full text of `:plan`. |
| `$STATE` | Current state id. |
| `$PREV_STATE` | The `from` of the most recent committed transition. **Empty** when there is none. |
| `$CYCLE` | Cycle number. |
| `$ATTEMPT` | Attempt number within the cycle. |
| `$CRASHED` | `1` when this entry follows a stage that died mid-flight. **Empty** on a clean entry. |
| `$LEDGER_DIGEST` | The rolling digest — totals, the last `:digest-last-n` committed transitions, and every artifact. |
| `$ENTRY_ADDENDUM` | The Navigator's get-back-on-track note for this state. **Empty** when the Navigator did not route here. |
| `$QA_CASES` | Markdown bullets, `- **{id}** — {desc}` per case. **Empty** when `:qa-cases` is absent. |
| `$ARTIFACT_<NAME>` | Project-relative path of a captured artifact. `<NAME>` is the claimed name uppercased: a claim named `diff` becomes `$ARTIFACT_DIFF`. |

The same map is used for `:check` command strings.

> **The variables only reach the agent where you interpolated them.** There is no automatically prepended context header. A stage prompt that never writes `$TASK` gives the agent no task. The positional message pi is spawned with contains no ticket id, task, plan, or digest — only "you are entering **X**, cycle N" and, when the stage names servers, the MCP connect instructions. Everything else is in the file you wrote.

`loop preview <state>` answers this directly: it lists the variables the body **actually writes**, split from the `$NAME`s that will pass through untouched, then renders the body. The render is _representative, not exact_ — cycle 1, attempt 1, no previous state, no artifacts, empty digest. Which variables are wired in is exact; what they will contain is not.

Substitution rules:

- **Maximal munch.** At each `$` the whole following `[A-Za-z_][A-Za-z0-9_]*` run is consumed before any lookup, so `$ARTIFACT_DIFF_PATH` is one token and can never be truncated to `$ARTIFACT_DIFF` with a dangling `_PATH`.
- **Unknown names pass through untouched.** `$HOME`, `$1`, and shell snippets in a fenced block survive verbatim.
- **`$$` is a literal `$`.**
- No `${...}` braces, no conditionals, no loops. Pure textual substitution.

### The four environment variables

Four scalars are also exported as real environment variables, to both the pi spawn and every `:check` subprocess:

```
TICKET_ID   STATE   CYCLE   ATTEMPT
```

Note the absence of a `$` and of everything else in the table above. `$TASK` and `$LEDGER_DIGEST` are template variables only; nothing in the agent's environment carries them.

## Model resolution

The Worker's model is assembled from four layers, most specific first:

1. The state's own `:model` / `:thinking` / `:provider`
2. The stage prompt's frontmatter `model` / `thinking`
3. The machine's `:defaults`
4. The machine's `:worker`, over the built-in floor (`claude-sonnet-5` at `medium`)

Layers merge **field by field**, not wholesale. A state that sets only `:thinking "high"` still takes its model from whichever lower layer supplies one.

**Stage prompt frontmatter never supplies a provider.** That layer contributes `model` and `thinking` only.

The resolved pair becomes one pi flag — `model:thinking`, joined by a colon:

```
--model claude-sonnet-5:high
```

Thinking levels, lowercase: `off` · `minimal` · `low` · `medium` · `high` · `xhigh` · `max`

The **Judge and Navigator are resolved separately** and do not participate in this chain: the machine's `:judge` / `:navigator` over the built-in floor. No state can change them — a stage cannot pick its own grader.

Do not merge the four layers in your head. `loop preview` prints the resolved `provider/model:thinking` for every state, computed by the same resolver the run calls; `loop preview <state>` adds the `--model` flag pi is handed verbatim.

## Skills

A skill is know-how a stage is _offered_, plus whatever scripts sit beside it. loop resolves a name to a path and passes `--skill <path>` to pi; **loop does not parse skills at all** — it never reads a `SKILL.md`'s contents, only checks that the file exists.

Two consequences: a skill's body is **not** rendered, so a `$TASK` written in one arrives as five literal characters; and a skill's frontmatter cannot set a model or thinking level.

A name containing `/` is an exact path: absolute as-is, otherwise relative to `machine.fnl`'s directory. A bare name has two candidates, in order:

1. `<project>/.loop/skills/<name>/` — a directory, **counted only if it contains `SKILL.md`**
2. `<project>/.loop/skills/<name>.md`

The directory form wins when both exist — a `SKILL.md` with scripts beside it is the richer thing. The `SKILL.md` rule exists so an empty `skills/foo/` fails loudly at `loop validate` instead of resolving and loading nothing.

The effective skill list for a stage is the **order-preserving deduplicated union**:

```
machine :defaults :skills  +  state :skills
```

There is no exclude list and no subtraction. **Withholding a skill hides know-how; it does not revoke a capability** — the Worker keeps bash, file editing, and pi's ambient extension discovery regardless. A QA stage that was not given the deploy skill can still deploy; it was only never told how you like it done. What pinning the list buys is _scoping_: instructions are the scarce resource in a stage, and a stage told about four things does better than one told about forty.

A stage prompt `.md` can double as a skill, but **only through the path form** — `:skills ["stage-prompts/review.md"]`, not `:skills ["review"]`, because a bare name is looked for under `.loop/skills/` and nowhere else. The file is rendered where it is the stage prompt and handed over raw where it is the skill, so a `$VAR` in it interpolates in one stage and appears literally in the other.

`loop validate` checks the whole union, not just the names the state writes, and a diagnostic for a name that came from `:defaults` says so.

### Writing one

A skill is a checklist, not a task with its own handoff — the calling stage stays in control of its own transition. Frontmatter carries `name` and `description` only; the description is what the model sees when deciding whether to open it, so write it as a _situation_, not a topic:

```yaml
---
name: debug-transient
description: Tell an infrastructure flake apart from a real bug before deciding whether to spend a code fix on a failure. Use when a test or pipeline failure could plausibly be either.
---
```

Directory form when there are scripts:

```
.loop/skills/spark-build/SKILL.md
.loop/skills/spark-build/build.sh
```

Then point the edge's `:check` at the same script (`:check "bash .loop/skills/spark-build/build.sh"`) so the agent and the harness run identical code.

## MCP servers

`:mcp ["warehouse"]` names a server in **the user's own** `~/.pi/agent/mcp.json`. loop never reads, ships, writes, or validates that file — it only carries names. `PI_AGENT_DIR` is deliberately not set on the spawn, precisely so pi's `mcp` extension finds the user's config.

The effective list is the same union as skills: `machine :defaults :mcp + state :mcp`.

**How the names reach the agent.** They are not a flag. Every session starts with every server _disconnected_, so loop leads the stage's entry message with instructions:

> Before anything else, connect the MCP servers this stage needs — they start the session disconnected, and `mcp({connect: "…"})` is what turns one on:
>
> - `mcp({connect: "warehouse"})`
>
> If one fails to connect, say so in your handoff rationale rather than working around it.

Two consequences:

- **A stage that does not name a server cannot reach it**, because nothing told the agent to connect it.
- **A name that exists nowhere fails at connect time, not at load time.** loop has nothing to check it against, so `loop validate` cannot tell a typo from a server not installed on this machine. Read the names back with `loop preview`; it reports them without connecting to anything.

The one MCP diagnostic `validate` emits is the `:pi-extensions` mismatch: naming servers on a state while `"mcp"` is absent from that list.
