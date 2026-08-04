# examples

One worked ticket, complete. **[`proj-1487/`](proj-1487/)** is a real `.loop/` directory — the machine, the prose it reads, the stage prompts its stages name, the skills those stages load, and the ledger the run actually appended.

It is not an excerpt. Copy it into a project and it runs.

## Verify it

A static smoke test: needs only Rust and Cargo, never invokes pi, staging, or any external service.

```sh
fixture="$(mktemp -d)"
cp -R examples/proj-1487 "$fixture/.loop"

cargo run --quiet -p loop -- --dir "$fixture" validate
cargo run --quiet -p loop -- --dir "$fixture" status --json

rm -rf "$fixture"
```

One `cp`, no environment variables. That is the point of the layout: everything loop reads or writes for a ticket is inside the ticket's own directory, so there is no second location a name could have come from and no precedence to reason about. What you copied is what runs.

## The ticket

PROJ-1487 adds a churn-score field to a Spark retention pipeline and exposes it through the API. Six states, ten transitions, two bounded loops — one for implement/review, one for the QA-and-debug cycle a flaky staging executor makes necessary.

| Path | What it is |
| --- | --- |
| [`machine.fnl`](proj-1487/machine.fnl) | The ticket machine: states, edges, guards, budgets, loops. The only required file. |
| [`task.md`](proj-1487/task.md) | What to do. Read into `$TASK`. |
| [`plan.md`](proj-1487/plan.md) | How. Read into `$PLAN`. |
| [`ledger.jsonl`](proj-1487/ledger.jsonl) | The append-only record the run produced. Every claim `loop status` and `loop recap` make is folded from this file. |
| [`stage-prompts/`](proj-1487/stage-prompts) | One prompt per stage. `validate-contract.md` is bespoke to this ticket; the rest are the generic ones. |
| [`skills/`](proj-1487/skills) | Situational know-how plus the scripts that carry it out. |

Several `.sh` files under `skills/` also back an edge's `:check` command, so the same script that tells a stage how to do something is the one that gates the transition out of it. `skills/debug-transient.md` is the single-file form — a bare `.md` with nothing beside it; the rest are directories with a `SKILL.md`.

## Reuse is a copy

There is no shared library directory. To start a new ticket from this one:

```sh
loop init PROJ-1500 --from examples/proj-1487
```

`--from` copies the directory and substitutes the ticket id. Nothing resolves at run time out of a location you have to remember, which means editing this example later cannot change a ticket already in flight — and it means keeping a `.loop/` of your own somewhere is the whole of what a shared toolbox used to be.

The cost is real: a fix to a generic stage prompt does not propagate. You re-copy, or you edit the ticket that needs it. That trade is argued in [design notes](../docs/05-design-notes.md).

## What is loop's own, and what isn't

Everything here is ordinary content you write. loop ships no code into a ticket directory — a Worker ends its stage by writing the handoff file the harness names in its prompt, and the Judge and Navigator have no tools at all, so there is nothing to vendor.

Skills use pi's own loader: loop resolves a name to a path and passes `--skill <path>`, and never parses the format. There is no `mcp.json` here on purpose — a state's `:mcp` names servers out of _your_ `~/.pi/agent/mcp.json`, and the stage connects them itself.

## Suggested reading order

1. [`task.md`](proj-1487/task.md) and [`plan.md`](proj-1487/plan.md) — what the run was asked to do.
2. [`machine.fnl`](proj-1487/machine.fnl) — the shape of the answer. Its comments carry the reasoning for each guard.
3. `loop diagram` — the same graph, drawn.
4. [`ledger.jsonl`](proj-1487/ledger.jsonl) — what actually happened, or `loop recap` for the readable version.
