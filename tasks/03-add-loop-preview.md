# Add `loop preview`

## Outcome

Give the operator a deterministic pre-run answer to “what will this loop do?” using the same resolution code the engine will use, without spawning pi, running checks, creating a ledger, or writing generated render files.

## CLI contract

```sh
loop preview            # concise whole-machine preview
loop preview implement  # detailed preview for one state
```

### Whole-machine preview

Print a stable, human-readable report containing:

- ticket, entry, terminals, escalation state, transition mode, and effective budgets;
- task, plan, and QA-case sources/content summary;
- every state in deterministic order with description, resolved playbook source, effective provider/model/thinking, skills, MCP servers, and reachable states;
- every outgoing transition with check command and timeout, criteria, failure action, and backoff;
- declared loops, heads, cycle limits, and exhaustion behavior;
- validation warnings. Validation errors should make preview fail after showing the diagnostics.

### State preview

For the requested state, additionally show:

- the resolved playbook path and frontmatter;
- the exact skills paths and MCP names that would be passed to the Worker;
- the effective Worker invocation metadata, excluding secrets/environment noise;
- the playbook body and which loop template variables it references;
- a clearly labelled representative render using cycle 1, attempt 1, no previous state, no artifacts, and an empty ledger digest.

The representative render must not claim to be the exact future prompt for a non-entry state: `$PREV_STATE`, `$LEDGER_DIGEST`, artifacts, cycle, attempt, crash state, and Navigator addendum are run-dependent. Print that limitation next to the render.

Both forms must be read-only and deterministic. They must not materialize vendored extensions, create `.loop/ledger.jsonl`, create `.loop/artifacts/`, or write under `LOOP_STATE_DIR`.

## Implementation work

- Add the `Preview` subcommand and optional state argument in `crates/loop-cli/src/main.rs`.
- Add report orchestration in `crates/loop-cli/src/commands.rs` and put reusable report structures/formatting in a new `crates/loop-cli/src/report.rs` if that keeps preview and the later recap command aligned.
- Refactor `crates/loop-cli/src/stage.rs` so model, playbook, skill, MCP, and prompt resolution has a pure/read-only path. `preview` must call the same resolver as `build_stage`; do not copy the four-layer model or local-first toolbox rules into the command.
- Reuse `loop_engine::validate` from `crates/loop-engine/src/validate.rs` rather than inventing weaker preview-only validation.
- Reuse the existing template scanner/substitution code in `crates/loop-core/src/context.rs` and `crates/loop-engine/src/prompts.rs` where possible. Add a small API for referenced variable names if necessary rather than regexing prompt text in the CLI.
- Avoid changing runtime behavior or the ledger schema.
- Add coverage in `crates/loop-cli/tests/e2e.rs` for resolved overrides, local-first playbooks/skills, edge and loop details, validation failure, unknown state, deterministic output, and proof that preview creates no run or rendered state files.

## Documentation work

Update existing operator documentation:

- `README.md`: place `loop preview` between editing and `validate`/`run` in the quickstart.
- `docs/01-getting-started.md`: add preview to “Shape the machine” and explain the representative-render limitation.
- `docs/03-customizing.md`: reference preview from model resolution, playbooks, template variables, skills, MCP, checks, and loops as the way to inspect the effective merged result.
- `docs/04-cli-reference.md`: specify both output forms, validation behavior, read-only guarantees, and exit behavior; update the subcommand list/count.
- No new standalone document is needed; `01` remains the workflow and `03` the configuration reference.

## Non-goals

- Running checks, connecting MCP servers, or testing credentials.
- Predicting future Worker decisions or transition paths.
- Making run-dependent prompts look exact before those events exist.
- AI-generated analysis of the machine.
- Writing the ordinary runtime render files as a side effect.
