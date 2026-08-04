//! `loop` — a local, ticket-level agent orchestrator.
//!
//! The modules layer bottom to top: [`core`] is the IR and the traits;
//! `ledger`, [`fennel`], `toolbox` and [`runner`] are the four I/O halves
//! (disk, Lua, prose, subprocesses); `engine` is the control loop written
//! against nothing but `core`'s traits; and the rest — [`commands`] and its
//! private neighbours — is the CLI wiring that hands the engine the real
//! implementations.
//!
//! This file only declares that layering; nothing enforces it. A module is
//! `pub` when something outside the library reads it — the binary, or the
//! integration tests under `tests/` — and `pub(crate)` otherwise. Nothing stops
//! `engine` reaching into `fennel` except review and the test that would have
//! to grow a Lua VM to keep passing. See the header on `engine/mod.rs` for the
//! boundary that actually matters.

// `core`, `fennel` and `runner` are the three the integration tests link
// against — `tests/machine_test.rs` needs a Fennel VM, `tests/mock_pi_e2e.rs`
// needs a `PiRunner`, and both need the IR to assert on.
pub mod core;
pub mod fennel;
pub mod runner;

pub(crate) mod engine;
pub(crate) mod ledger;
pub(crate) mod toolbox;

// `main.rs` is a separate compilation unit from this library, so the dispatch
// it performs has to reach `commands` across the crate boundary. The rest of
// the CLI wiring stays where it was: private to the library.
pub mod commands;

mod episode;
mod output;
mod report;
mod sessions;
mod stage;
