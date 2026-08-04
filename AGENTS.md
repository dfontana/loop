# Development lifecycle

Use the project-local mise tasks for all test, formatting, and linting work:

- `mise run test`
- `mise run format`
- `mise run lint`

Do not invoke the underlying lifecycle commands directly when a mise task exists.

# Layout

Two crates. `crates/loop` is the whole harness — library and `loop` binary — and `crates/mock-pi` is the scripted stand-in for `pi` that keeps the tests offline and free. `mock-pi` stays separate because the tests need it as a real executable on disk.

Inside `crates/loop/src`, the modules layer bottom to top: `core` (the IR and the traits), then `ledger`, `fennel`, `toolbox` and `runner` (the four I/O halves — disk, Lua, prose, subprocesses), then `engine` (the control loop), then the CLI wiring at the top level. They were seven crates until they were merged, and the layering is the thing that survived the merge.

Two conventions carry the weight cargo used to. A module is `pub` in `lib.rs` only when something outside the library reads it — the binary or the integration tests — and `pub(crate)` otherwise. And `engine` imports nothing but `core`: its tests run the entire machine in-process against trait fakes, with no Lua, no subprocess and no filesystem, and a reach into `fennel` or `runner` would compile fine while quietly making that impossible. Read the header on `engine/mod.rs` before adding a `use crate::` line there.
