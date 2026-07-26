# Add `loop logs`

## Outcome

Give an operator a convenient human view of recent ledger activity without having to know the ledger path or write `jq`, while preserving a zero-boilerplate way to access the complete JSONL ledger.

## CLI contract

```sh
loop logs                 # last 20 events, human-readable
loop logs -n 50           # last 50 events
loop logs --raw            # complete ledger as JSONL
loop logs --raw | jq '…'   # machine processing
```

- `-n <N>` defaults to `20` and applies only to the human view.
- Human output prints one event per line, oldest-first within the selected tail, using the timestamp and the existing `status` event summary grammar. It has no `recent:` wrapper, so it composes cleanly with `tail`, `grep`, and pagers.
- `--raw` writes the entire repaired ledger to stdout exactly as JSONL, with no headings, status messages, reformatting, or filtering. It should conflict with an explicitly supplied `-n`, rather than silently choosing one meaning.
- On an empty ledger, human mode prints the same helpful `no run yet` message as `status`; raw mode writes zero bytes and exits successfully.
- The command must work when `machine.fnl` is missing or invalid. The ledger is its only source.
- Read and parse failures go to stderr and return exit 1. Valid ledger content goes only to stdout.

## Implementation work

- Add the `Logs` subcommand and clap arguments in `crates/loop-cli/src/main.rs`.
- Add the command implementation in `crates/loop-cli/src/commands.rs`.
- Extract the current private `summarize(Event)` logic used by `status` into a small shared CLI formatter (either in `commands.rs` or a new `crates/loop-cli/src/output.rs`) so `status` and `logs` cannot drift.
- Open the ledger through `loop_ledger::Ledger` before raw output so torn-tail repair remains consistent with every other ledger reader. If the CLI would otherwise need to reach through ledger internals, add a narrow raw-read API in `crates/loop-ledger/src/lib.rs` rather than duplicating repair rules.
- Update `crates/loop-cli/tests/e2e.rs` with coverage for:
  - the default 20-event tail and ordering;
  - `-n` overrides, including fewer events than requested;
  - `--raw` being parseable as JSONL and byte-preserving after repair;
  - the `--raw`/`-n` conflict;
  - empty ledgers in both modes;
  - operation without a loadable machine.

## Documentation work

Update, rather than create a separate document:

- `README.md`: add `loop logs` to the “while it runs, or after it stops” block.
- `docs/01-getting-started.md`: use `logs` in the monitoring/post-run workflow.
- `docs/02-how-it-works.md`: add `logs` under “Inspecting a run” and make `logs --raw | jq …` the documented way to reach the ledger without knowing its path.
- `docs/04-cli-reference.md`: add the complete command, flag, empty-ledger, stdout, and exit behavior; update the subcommand list/count.
- Update the module-level “seven subcommands” comment in `crates/loop-cli/src/commands.rs`.

## Non-goals

- Following a live stream (`-f`); operators can rerun `loop logs` initially.
- Parsing or displaying pi session transcripts.
- Replacing `loop status`, which remains the compact folded-state view.
- Adding a second log store.
