# Design notes

The [`loop-authoring` skill](../skills/loop-authoring/SKILL.md) describes what `loop` does and how to drive it. This one is the argument: why the system is shaped this way, what each choice costs, what was tried and taken back out, and which limits come with it. If you are trying to change `loop` rather than use it, start here.

## The problem

Point a good coding agent at a real ticket and let it run for an hour. Three things happen, reliably.

It **drifts**. The ticket said "add the column and expose it on the endpoint"; somewhere around the third failing test the session is refactoring the test harness, and nothing in the loop notices that the work stopped matching the task.

It **declares victory**. The agent is the only party with an opinion about whether it is done, and it is the party that wants to be done. "Tests pass" gets written by the same process that would have had to run them.

It **cannot be audited**. What actually happened lives in one 200k-token transcript. You can read it, but you cannot fold it, diff it, grep it for the moment a decision was made, or resume from it after a crash.

None of these is a failure of the model at the work. They are failures of the _frame_: one prompt, one session, one uninterrupted stretch of autonomy with nobody but the agent deciding whether to keep going. And ticket-level work already has structure — implement, review, test, fix, ship — that a single session flattens into prose. That structure is exactly where you would want to put a gate. `loop` exists to make it explicit enough to attach one.

## Why a state machine

States are stages; edges are claims. "Implementation is done enough to review" is a claim, and once it is an edge in a declared graph it is a thing the harness can refuse. In a single session that same claim is a sentence the agent writes to itself.

The graph is also the unit of reuse, and now the unit you keep. A ticket's machine is meant to be hacked into shape in five minutes and thrown away with the branch; what makes that affordable is that it starts as a copy of a directory that already worked — the bundled machine, or one you kept from a previous ticket and pointed `loop init --from` at. Almost none of it is original.

Making the graph a declared value buys a few things for free that a prompt never gives you. `loop validate` walks it statically — reachability, dangling stage prompts, a state with no path to any terminal, an unguarded edge — and `loop diagram` renders it as mermaid without touching the filesystem, because the graph is a value and not a behaviour. Both are in [the CLI reference](../skills/loop-authoring/references/cli.md).

The cost is real: you now maintain a second artifact per ticket, and a badly drawn graph fails in ways a prompt cannot — a state nothing reaches, a loop head no edge re-enters. That failure mode is why `validate` exists, and why it errors rather than warns on most of it.

## Why the harness decides, not the agent

This is the thesis. An agent grading its own work is _the_ failure mode, so the harness owns commitment: the agent proposes, the harness disposes. Nothing an agent says moves the machine on its own.

Three mechanisms carry that, in order of how hard they are to argue with.

**The proposal is validated against the graph, not trusted.** A Worker names its next state in a handoff file; the harness looks that name up in the current state's outgoing edges before anything moves. A target that is not a declared neighbour does not become one — it routes to the Navigator. (See [the handoff protocol](../skills/loop-authoring/references/runtime.md#the-handoff-protocol).)

This used to be enforced one step earlier, by an injected `transition` tool whose `to` parameter was typed as an enum of reachable states, so an invalid edge was unrepresentable rather than merely rejected. That was a genuinely stronger constraint and it is worth being honest that it is gone. What makes the loss small is that it was never the thing doing the work: the harness re-checked the target against the graph regardless, because the tool ran inside the agent's process and nothing inside that process is evidence. The enum saved a Navigator spawn on a bad proposal. It did not prevent a bad commit, because the check that prevents a bad commit is downstream of it and still runs.

**The Judge does not share the Worker's context.** It is a separate process with its own model, `--no-session`, no builtin tools, no extensions, no skills, and no tools of loop's own either — it answers in prose against a first-line contract because it could not write a file if it wanted to. It is handed the Worker's summary and the artifact paths as _evidence_ — not the Worker's proposed target, not its argument for advancing, and no ability to go look at anything else. It judges what it was given, against criteria written by you rather than by the stage it is judging. Be honest about the boundary: the summary is still Worker-authored, so the Judge weighs a claim. What it cannot do is be talked around by the session that produced the claim, because it never sees that session.

**A `check` never touches the Worker's session at all.** It is a command the harness runs itself, in its own subprocess, after the stage has exited; the exit code decides. That is the one signal on the whole ledger a Worker cannot author, and it is why a failed check short-circuits — the Judge is never spawned, so a deterministic fact is not appealable to a model. The rule that follows: any fact that actually matters should be pushed into a check, and `criteria` should be reserved for the genuinely fuzzy remainder.

The corollary is a design constraint on you, not on the harness. Everything else on the ledger — the summary, the claimed artifact paths, the rationale — is Worker-authored and should be read as testimony.

## Why three roles

Because judging and routing are not the work, and paying Worker prices for them is waste. The Worker runs on the strong model with real thinking budget; the Judge and the Navigator default to a cheap model at low thinking, and their jobs are small enough that this is not a compromise. Deciding "does this diff satisfy these three criteria" is a smaller question than producing the diff.

But cost is the secondary reason. The primary one is that **isolation is what turns the Judge's verdict into evidence**. A Judge sharing the Worker's session would be a second opinion from the same context — self-grading with extra steps and an extra invoice. Spawning it cold, tool-less, and criteria-first is the whole mechanism; the cheap model is what that mechanism happens to make affordable.

The Navigator is the same trick pointed at routing. It fires only when the proposal is unusable — blocked, absent, or naming something that is not a neighbour — picks from an enum of reachable states, and can leave a note for the next stage. It is capped both run-wide and per source state, and the cap is deliberately low: a Navigator that keeps firing is not a routing problem, it is a machine whose graph does not match the work, and escalating to a human is the correct answer to that.

The Judge's tokens land on its `guard_checked` event and count against the run's dollar budget like any other spend, which is the only way a criteria-heavy machine can be bounded by the same number that bounds a Worker-heavy one.

## Why a file instead of a tool

A decision the harness must act on should not travel as free text the harness has to interpret. That is the argument; where it lands is a file, and getting there took one reversal worth explaining.

**The first design was three injected tools.** `transition`, `verdict`, and `choose` — vendored TypeScript, `-e`'d into each spawn, one per role. Each did the same thing: take structured arguments from the model, `JSON.stringify` them, and return them as a marker line (`LOOP_TRANSITION {…}`) that the harness scraped back off pi's JSONL stdout. Written out like that, the shape of the problem is visible: the tool was a round-trip through a string. The structured data was serialized specifically so it could be found again by prefix-matching lines of tool output.

What that bought was one real thing — `to` typed as an enum of reachable states — and it cost a hard dependency on pi's extension ABI. Every role's answer had to route through a TypeScript file that imported `@earendil-works/pi-ai` and `@earendil-works/pi-coding-agent`. loop could not drive any agent but pi, and not because of anything about how loop works.

**A file keeps the structure and drops the coupling.** The harness names a path in `$LOOP_HANDOFF` and in the rendered prompt; the Worker writes JSON there and stops; the harness reads it with serde after the process exits. It is still structured data, still parsed by a schema, still incapable of being confused with prose. The Worker already had a `write` tool, so nothing was added to make this possible.

The two tool-less roles are a different case, and the difference is instructive. The Judge and Navigator are spawned `--no-builtin-tools --no-extensions` precisely so they cannot go looking for evidence — which also means they cannot write a file. So they answer in their final message, against the narrowest contract a model can be held to: one bare token on the first line. `PASS` or `FAIL`. A state name. Not JSON, which has more ways to be subtly wrong, and not a sentence, which has to be interpreted. The parser fails on anything else, and failure is safe (below).

Two alternatives, and why not:

**A CLI shell-out would invert control.** Letting the agent run `loop transition review` puts the child in charge of the parent. `loop` is the parent process; `pi` is the child it spawned and is currently blocked on. A subcommand invoked from inside that child would have to reach back into a run its own parent owns, mid-stage, and hand a decision back through a channel the parent then has to parse. It reintroduces the text-parsing problem and adds a lifecycle question ("what if the agent calls it twice?") that a file does not have — a second write simply wins, the same way the last marker used to.

**Scraping the final message for a JSON block** would work for the Worker too, and is what the two tool-less roles effectively do. It is worse where there is a choice, because a Worker's final message is also its summary — a human-facing artifact that lands on the ledger — and overloading it with a machine-readable payload means every change to one is a risk to the other. A file separates the audience.

Two honest notes on failure. First, what happens when the answer is missing is a design decision, not an accident, and it differs per role: a Worker with no usable handoff becomes a blocked proposal and goes to the Navigator; a Judge with no usable verdict **fails closed**; a Navigator with no usable choice escalates. Every default sends the run toward a human rather than toward the next state.

Second, tolerance is bounded on purpose. The handoff is read as JSON or not at all. A verdict tolerates blank lines and markdown decoration around `PASS`, because those are formatting habits — but not a preamble sentence, because "let me assess this" before a verdict is a model that did not follow the contract, and a grader that has stopped following instructions should not be waving work through. The Navigator matches its first line against the offered states exactly: no prefix matching, no fuzzy fallback. Guessing which state a near-miss meant is how a run ends up somewhere nobody chose.

None of this makes an agent's claim trustworthy, and it was never supposed to. A handoff is a _proposal_ that still has to survive the guards. That distinction is where an earlier design went wrong (see [Reversed decisions](#reversed-decisions)): values scraped out of tool output were treated as trusted facts, and any stage with `bash` could print them. The current shape has the same exposure and the same answer — the file is worker-authored, so it is testimony, and what gates a transition is a `check`'s exit code and an isolated Judge.

## Why an append-only ledger

There is no mutable state file anywhere in `loop`. The run's state is _folded_ from the event log every time it is needed, which means `resume`, `status`, and the digest handed to the next stage are three views of one source that cannot disagree with each other. The alternative — a `state.json` alongside a log — gives you two things to keep in sync across crashes, and they will drift.

Durability is unglamorous and deliberate: the file is opened once and held, each event is one `write_all` of a line plus a newline, then an fsync. On open, a torn tail (a half-written last line from a kill mid-append) is truncated back to the last whole line, idempotently. An unparseable line in the _interior_ is a hard error, because that is corruption rather than a crash and silently skipping it would produce a plausible-looking wrong fold.

The costs are worth naming.

The fold has to stay total. Every event either contributes to run state or is explicitly inert, and adding an event type means deciding which. Get that wrong and the failure is not a crash, it is a run that resumes at the wrong point.

The event schema is a compatibility surface with no version marker — no `seq`, no run id, no schema field; ordering is file order. Today that is fine because nothing has shipped and back-compat shims were deliberately deleted rather than carried (see below): a schema change is simply a change, and old ledgers stop loading. It will stop being fine the first time someone has a ledger they care about, and a version field is cheaper to add then than to infer later.

The envelope carries one field beyond `ts` and the payload: `elapsed_s`, the run's accumulated wallclock. It is there because time is the one budget the ledger cannot reconstruct — timestamps include the hours a run sat interrupted, so a resumed run computing elapsed from `ts` would be instantly over budget, and a resumed run computing it from its own process start would have no budget at all. Carrying the accumulator forward on every append is what makes `:wallclock-s` bound the run rather than the session.

And resume granularity is per-event, which means an interrupted stage re-runs from the top at the next attempt number rather than picking up mid-work. That is a straightforward consequence of not checkpointing inside a stage, and it makes **stage idempotency a real authoring requirement**: an `open-pr` stage has to check for an existing PR, a deploy has to be keyed on something stable. The mechanics are in [the runtime reference](../skills/loop-authoring/references/runtime.md).

## Why Fennel

A machine is configuration, not a program. The load path makes that literal: the Fennel file evaluates to a plain Lua table, serde deserializes that table into an authored-shape struct, and `convert.rs` turns that into the IR. Everything downstream — the validator, the diagram renderer, the engine — sees the IR and never the Lua. The language is on one side of a hard boundary.

That middle layer is newer than the rest and earns its place by what it deletes. It replaced a hand-written walker that read the keys it knew and ignored everything else, so `:playbok "implement"` loaded fine and failed later as "needs either `:stage-prompt` or `:prompt`", and a misspelled `:max-cycles` left a loop unbounded while the run kept going. Every struct is `deny_unknown_fields` now, and the error carries the path to the offending value. What is left in `convert.rs` is only the rules serde cannot state — a removed key that deserves a migration message, a `.md` path that has to be read, exactly-one-of `:stage-prompt`/`:prompt`, and the budget and model layering.

So why a language at all, rather than YAML? Because the 5% of a machine that wants structure wants it badly: a comment explaining why an edge exists, a binding shared by four states, a template you clone and edit rather than copy-paste. YAML answers that with anchors and, the moment a guard needs to be more than a comparison, with an embedded expression mini-language you have to design, document, and debug. Fennel gives you ordinary bindings and comments for free, and the guard problem is solved a different way entirely — by `:check` commands and `:criteria` prose, both of which the harness owns.

Which is the point of the boundary. `:when` guards were real: Fennel closures registered at load time and called by the engine during a run. They were removed, and using `:when` now fails with an error that points at `:check` and `:criteria` instead. Removing them is what keeps a machine a _value_ — something you can hash, diagram, and statically validate — rather than a program whose behaviour you can only observe by running it. The macro DSL sketched in the early design notes was never built for the same reason; machines are plain tables today.

The costs: a Lua runtime embedded in the binary, an unfamiliarity tax on anyone who has to read parens six months from now, and the fact that loading a machine evaluates arbitrary code. The last one is acceptable only because machines are files you author yourself; it would need rethinking if machines were ever shared.

## Why one location

Everything is `<project>/.loop/`: the machine, the prose, the stage prompts, the skills, the ledger, the artifacts, and the derived renders under `run/`. One root, one answer to "where does this come from".

The argument for that is ownership. A ticket directory that holds everything the run touched is a thing you can review in the same diff as the code, back up in the same commit, and `rm -rf` in the same breath as the branch. Nothing about the run depends on the state of a home directory that another ticket may have edited yesterday, so the machine you can read is the machine that ran. Resolution is correspondingly dull: a stage prompt or skill name names a file under `.loop/`, or it is an error naming the path it looked at. No precedence, nothing to shadow, nothing for `loop preview` to have to disambiguate.

Reuse is a copy rather than a lookup. `loop init --from <DIR>` copies a `.loop/`-shaped directory into the new ticket, so what you started from is recorded in the ticket instead of being resolved out from under it mid-run.

**What that gave up is real.** There is no shared library any more. Improving a `review` stage prompt once and having every ticket pick it up is gone — you fix the ticket in front of you, and you re-copy into the next one. A kit of directories you `--from` accumulates the same drift a vendored dependency does, and nothing in loop reconciles it. If you run many tickets against one workflow, you will feel this.

It was traded for the paragraph above it. A shared library that propagates propagates _mid-run_: editing a toolbox stage prompt silently changed the next stage of every ticket then in flight, which is the same property seen from the side where it hurts. The old design admitted that in its own text — unpinned resolution "does mean a toolbox edit lands on the next stage that resolves it", written as an acceptable cost — and that sentence is what eventually killed it. The current design pays a different, quieter price and does not pretend otherwise. Paths are enumerated in [the machine reference](../skills/loop-authoring/references/machine.md#where-everything-lives).

## Skills bound instructions, not capability

Workers are spawned with skills pinned shut — automatic discovery off, each stage's skills passed explicitly. It is tempting to read that as a sandbox. It is not, and the docs should not let you believe it is.

A skill is a prompt plus the scripts sitting next to it, and the agent runs those scripts through `bash` like anything else. The Worker keeps its builtin tools and pi's ambient extension discovery — it gets no flag turning either off. So withholding a skill from a stage **hides know-how; it does not revoke access**. A QA stage that was not given the deploy skill cannot be relied on not to deploy; it was only never told how you like it done.

That is still worth doing, because the thing it actually buys is scoping. Instructions are the scarce resource in a stage: every skill you load is context competing for attention, and a stage told about four things does better than a stage told about forty. Pinning the list is prompt hygiene with a stable, reviewable definition — and it makes a stage's instruction set a declared, diffable part of the machine rather than a function of whatever is installed on this laptop today.

The real containment lives elsewhere, and it is worth being precise about where: the Judge and Navigator spawns, which genuinely are stripped — no builtin tools, no extensions, no skills, no session — and the harness-run `check`, which decides outside the agent's reach entirely. Those two are boundaries. The skill list is a scope.

## Why stage prompts and skills are both here

They look like the same thing. Both are markdown with YAML frontmatter, both live one directory apart under `.loop/`, and a file written as one will usually parse as the other. The reasonable question is why loop has two concepts rather than folding stage prompts into the skill mechanism pi already provides.

Because they are not two spellings of one idea, they are opposite ends of one axis: **a stage prompt is told, a skill is offered**. `--append-system-prompt` puts a file in the stage's context unconditionally. `--skill` shows the model a name and a description and lets it decide. That difference is the whole design, and three things fall out of it that a skill structurally cannot do.

**A skill cannot be relied on to be read.** The stage's job, its definition of done, the instruction to write a handoff — none of that survives being optional. Forcing a skill to load in order to fix that is `--append-system-prompt` with extra steps and a worse name.

**A skill cannot carry run state.** loop never opens one. There is no automatically prepended context header either, so `$TASK`, `$PLAN`, `$LEDGER_DIGEST`, and `$ENTRY_ADDENDUM` reach a stage exactly where the stage prompt interpolates them and nowhere else. Rendering into skills instead would mean loop parsing and rewriting `SKILL.md`, which is the second reason not to: the format is pi's, and the only pi-specific code left in loop is one argv builder ([why a file instead of a tool](#why-a-file-instead-of-a-tool)). Taking ownership of a second pi format to avoid one concept is a bad trade.

**A skill cannot set the stage's model.** Frontmatter `model`/`thinking` is layer 2 of the four-layer resolve, and it works because a stage prompt belongs to exactly one state. A skill belongs to however many states name it, so there is no coherent answer to whose model it would set.

What the two _should_ share is the plumbing, and now do: one resolver, parameterized by the candidate list and by what counts as a usable hit. There used to be two copies of the `/`-means-exact-path escape hatch, the first-hit-wins loop, and the `Unresolved` error, differing only in those two things — which is how `.loop/skills/<name>/` and `.loop/stage-prompts/<name>.md` ended up with subtly different notions of what a hit was.

The cost of keeping two concepts is the confusion this section exists to answer, and it has bitten once already: a bundled `debug-transient.md` shipped for several releases in `playbooks/` with a header comment explaining it was really a skill, reachable by a `:skills` name that could never have resolved there, citing a docs section that had been deleted. It is a skill now, and the bundled machine names it. The lesson is that the distinction has to be visible in where a file lives and what the docs call it, because it is not visible in the file.

## Reversed decisions

Things that were built or specified and then taken out. Each is a place where the current design is a correction rather than a first draft.

- **The three injected tools.** `transition`, `verdict`, and `choose` — vendored TypeScript, `-e`'d per spawn, each one JSON-encoding its arguments into a marker line the harness scraped back off pi's stdout. A round-trip through a string, in exchange for a hard dependency on pi's extension ABI. Replaced by a handoff file the Worker writes and a first-line contract for the two tool-less roles. The one real loss is the `to` enum, argued above; the gain is that the only pi-specific code left in loop is one argv builder.
- **`:transition-mode`.** Chose between a constrained enum and a free string for the injected tool's `to`. With no injected tool there is no schema to pick, and the surviving behaviour is what `open` described: the harness checks the target against the graph and routes an unknown one to the Navigator. Setting the key is now an error that says so.
- **The two-location split.** A portable toolbox at `~/.config/loop/` holding stage prompts, skills, and machine templates; a ticket directory at `<project>/.loop/`; a third root for generated renders. Names resolved local-first across the first two, so a ticket could shadow a shared stage prompt without forking it. What killed it was the property that made it useful: a shared definition reaches every ticket, including the ones already running, so editing a toolbox stage prompt silently changed the next stage of every ticket in flight — and "where did this prompt come from" had two answers that `loop preview` had to report on. One root now, and reuse is `loop init --from <dir>`, which copies. The loss is that there is no library to fix once — an improved stage prompt reaches the next ticket you copy into and no others, and a kit of `--from` directories drifts the way vendored code drifts.
- **`config.fnl`.** A second authored file, in the toolbox, holding `:provider`, `:worker`, `:judge`, `:navigator`, `:default-skills`, `:default-mcp`, `:pi-extensions`, `:budgets`, and `:digest-last-n`. Every one of those was a value a machine could already override, so the file was a second place to look for an answer the machine gave anyway — and a second thing to keep in sync when the machine's answer differed. The keys are machine keys now, the two `default-*` lists folded into the `:defaults` a machine already had, and what a machine does not name comes from a built-in floor in code rather than from a file. Writing a removed key is an error that names its replacement.
- **The seven-crate split.** One crate per layer — core, engine, fennel, toolbox, runner, ledger, CLI — with the dependency edges between them written in `Cargo.toml`, so a reach from the engine into the Fennel VM or the process runner was a build failure rather than a review comment. It is one crate now, `loop`, with those layers as modules. Be clear about what that costs: the constraint is a convention, documented at the top of `crates/loop/src/engine/mod.rs` and held up by a test suite that runs the whole control loop in-process against trait objects — no Lua, no subprocess, no filesystem. An errant `use crate::fennel::…` in the engine compiles fine; what stops it is that those tests would then need a Lua VM to keep passing. That is weaker than a compiler error, and it depends on whoever adds the import noticing.
- **`:when` guards and ledger vars.** Vars were sold as trusted facts ("a real exit code asserted it"), but every path into them ran through the Worker's session — the harness scraped them from tool output, so any stage with `bash` could print the marker itself. The whole tier went, closures included. Trust had to come from _who ran the command_, which is now `:check`.
- **YAML machines.** The original plan was YAML-first with Fennel as an opt-in second backend behind a shared IR. v1 ships one loader and one authoring surface; two would have doubled the loader surface for a format that could not express the shared structure anyway.
- **Per-stage tool filtering.** Stages once carried tool allowlists and scoped tool wrappers. Once nothing gated on tool stdout, an allowlist only constrained blast radius — and only on a stage without `bash`, which no stage was, since the machine-wide default granted it. Replaced by pi skills.
- **The reproducibility snapshot.** Removed. An LLM run is not reproducible, and pretending otherwise buys a heavy artifact in exchange for a guarantee that does not hold; the ledger records decisions and rationale instead.
- **A loop-owned `mcp.json`.** MCP was modeled as a config file `loop` staged into a generated agent directory. That duplicated a file you already have — one holding OAuth state and bearer tokens — and the redirect it relied on replaces rather than overlays, so a machine naming no server would have taken away every server you had configured. Now a state names which of _your_ servers it needs, and nothing is staged.
- **Pre-launch ledger back-compat.** A serde default existed so a ledger written before an early schema change would still fold. Nothing had shipped, so no such ledger existed; it was deleted rather than carried forward as a permanent apology.
- **`:context "full"`.** A key with two values, one of which was never wired to anything: the digest path was unconditional, so `full` silently got you `digest`. Rather than implement a second continuity channel nobody had asked for, the key is gone and setting it is an error that points at `$LEDGER_DIGEST`. The rolling digest is the channel; `:digest-last-n` is the knob.
- **Artifact hashing.** Captures recorded a sha256 and wrote a `.sha256` sidecar. Nothing ever checked either: the Judge cannot open a file, and no consumption path re-verified. A hash nobody checks is not integrity, it is a claim about integrity — so it went, and what remains is the property capture actually provides, which is a _snapshot_ under a stable per-cycle name.
- **The name "playbook".** The key was `:playbook` and the directory was `.loop/playbooks/`, borrowed from the CI vocabulary the reuse model came from. It was a bad name for the thing: a playbook is a procedure you consult, which is exactly what a stage prompt is _not_ — it is the text a stage is handed whether it wants it or not, and the one place run state can enter a stage. Every question about "is this a playbook or a skill" was really a question the name should have answered. `:stage-prompt` and `.loop/stage-prompts/` now; the old key is a hard error naming its replacement, because a wrong guess between `:stage-prompt` and `:prompt` loads cleanly and runs the wrong text.
- **A `pi_extensions` field on the Worker spec.** It was assembled per spawn and read by nothing, because pi has no flag for enabling an installed extension by name. The machine key survives as a declaration the linter reads; the plumbing that pretended to act on it does not.

## Prior art

Almost every piece of this exists somewhere. The combination — user-authored per-ticket state machines, a durable ledger, a self-contained ticket directory, driven by a thin CLI over headless coding agents — is the part that seems unoccupied.

**LangGraph** is the closest conceptual match: agents as a graph, conditional edges, cycles, checkpointing, human-in-the-loop interrupts. `loop` differs on three axes — it is a CLI over subprocess agents rather than a library embedding them, the graph is authored per ticket rather than built in code, and the checkpointer is a greppable JSONL file rather than a database. The edge-case thinking in their reducer is worth reading before touching the fold.

**Temporal, Restate, DBOS, Step Functions** are the ledger's ancestry: event-sourced workflows, deterministic replay, fold-to-current-state as the only state representation, and the discipline that side effects must be idempotent because replay re-runs them. The difference that shapes everything downstream: their activities are deterministic functions, ours are LLMs. That is exactly why there is a fuzzy Judge tier sitting on top of a deterministic core, and why retries are bounded by declared cycle limits rather than assumed convergent.

**XState and statecharts** supplied the vocabulary — guard, transition, entry — and, more usefully, the precision about _when_ a transition may fire. Their hierarchical and parallel-region models are deliberately not adopted; a ticket graph that needs nested superstates is a signal the machine is too clever.

**GitHub Actions, Argo, Dagger** are the reuse model: declarative steps referenced by name, templating, artifacts passed between steps. Stage prompts are their `uses:`. One thing explicitly _not_ taken: their `name@version` pin — and the way loop avoided needing one is worth noting, because it is the reverse of theirs. They pin so that a shared, mutable definition cannot change under a running job. loop copies instead: a stage prompt is a file inside the ticket, so there is no shared definition to pin and nothing that can change mid-run. That was not the original answer. The original answer was a shared toolbox resolved by name, unpinned, where an edit did land on the next stage that resolved it — see [why one location](#why-one-location). Their gap is the reason this project exists at all: they are acyclic and have no reasoning executor.

**StateFlow** (Wu et al., 2024) and AutoGen's group-chat patterns are the empirical argument that framing LLM task-solving as an explicit state machine beats free-form ReAct on control and cost for multi-step work. Cite it the next time the structure feels like overhead.

**OpenHands, SWE-agent, Aider's architect mode** are what happens _inside_ a Worker stage — edit, run, observe, against a real test suite. They are not competitors; a Worker can be one. The difference is that their control flow lives inside one agent's head, where it cannot be inspected, budgeted, or resumed. `loop` externalizes exactly that layer and leaves the inner loop alone.

**AutoGPT and BabyAGI** are the cautionary tale: goal in, unbounded autonomy, agents wandering and burning money with nothing to show. The declared graph, bounded cycles, objective gates, and hard budgets are all corrections to that specific failure.

**pi's own `run-plan` / `run-review` skills** are the direct precedent — a coordinator that plans, delegates to a persistent implementer, and runs a bounded adversarial review loop with an independent reviewer model. That is `loop` in miniature, inside one session. The generalization is to lift the hard-coded plan→implement→review→fix loop into a user-authored machine and the coordinator out of the session into a CLI with a ledger — so control flow becomes inspectable, budgetable, and resumable across crashes.

## Limits we accept

Not defects — consequences of choices made above, listed so nobody has to rediscover them by being surprised.

**A stage is the unit of recovery.** Nothing checkpoints inside a stage, so an interrupted one re-runs from the top at the next attempt number. That is what makes **stage idempotency an authoring requirement**: an `open-pr` stage checks for an existing PR, a deploy is keyed on something stable. The harness helps as much as it honestly can — `$CRASHED` tells a stage prompt it is re-entering after a death — but it cannot make a non-idempotent stage safe.

**Budgets are sampled between stages.** The check happens at stage entry, before a spawn. A single Worker that runs for three hours under a two-hour `:wallclock-s` is not interrupted; it is noticed when it finishes. Bounding _within_ a stage would mean killing an agent mid-edit, which trades a budget overrun for a corrupted working tree.

**The ledger has no version marker.** No `seq`, no run id, no schema field; ordering is file order. A schema change is just a change, and ledgers written before it stop loading. That is a deliberate pre-launch position, not an oversight — see the note under [the ledger](#why-an-append-only-ledger).

**`loop validate`'s terminal-reachability ignores `on_fail: route` edges.** The reverse walk covers declared `:transitions` only, so a state whose only way forward is a guard-failure route reads as having no path to a terminal. Cycle detection already special-cases routes; reachability does not, and the false positive is louder than the false negative would be.

**Skills scope instructions, not capability.** Covered in full above: a stage that was not given the deploy skill can still deploy, because `bash` is not withheld. The containment is the Judge and Navigator spawns and the harness-run `check`, not the skill list.

**Artifacts are snapshots, not verified evidence.** The harness copies what a Worker claims and records the path. It does not hash, re-verify, or inspect the contents, and the Judge — having no tools — cannot open one. An artifact is a durable hand-off between stages and a record for a human. What actually gates a transition is a `check`'s exit code and the Judge's reading of the summary.
