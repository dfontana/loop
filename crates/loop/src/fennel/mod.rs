//! Fennel evaluation: `machine.fnl` → the [`crate::core`] IR.
//!
//! The Fennel compiler (`vendor/fennel.lua`, 1.5.3, MIT) is embedded in the
//! binary and loaded into an `mlua` Lua 5.4 VM at startup. A machine file is a
//! Fennel *module* that returns a table, which this module converts to the
//! [`crate::core`] IR. Evaluation is one-shot: nothing from the VM outlives the
//! load, so the IR is plain data by the time the engine sees it.

use std::path::{Path, PathBuf};

use crate::core::{CoreError, Floor, Machine, Result, machine_hash};

mod convert;
mod eval;
mod wire;

pub use convert::machine_from_table;

/// The embedded Fennel compiler.
pub const FENNEL_LUA: &str = include_str!("../../vendor/fennel.lua");

/// An initialized Lua VM with Fennel loaded and the `loop` support module in
/// `package.preload`.
pub struct FennelVm {
    lua: mlua::Lua,
    /// The `fennel` module table, kept from `install_fennel`.
    ///
    /// `eval_fennel` used to look it up through `package.loaded` on every call
    /// and fall back to installing the compiler if it was missing — a fallback
    /// that could never fire, since `new` installs it and then threw the table
    /// it got back away.
    fennel: mlua::Table,
}

impl FennelVm {
    /// Create the VM, install the Fennel compiler, and register the `loop`
    /// runtime module (`transient?`, `real?`, `mins`, `secs` — the helpers
    /// examples/local/machine.fnl uses).
    ///
    /// The VM is deliberately *not* sandboxed: machine files are authored by
    /// the person running the harness. The vendored Fennel compiler needs the
    /// `debug` library at `require` time (for `debug.getinfo`/`traceback`), so
    /// this uses `Lua::unsafe_new` rather than the safe-subset constructor.
    pub fn new() -> Result<Self> {
        // SAFETY: no untrusted code is ever loaded into this VM — machine and
        // config files are authored by the person invoking the harness, and
        // full stdlib access (including `debug`) is required by fennel.lua
        // itself and is an explicit, documented design choice (docs/05-design-notes.md).
        let lua = unsafe { mlua::Lua::unsafe_new() };
        let fennel = eval::install_fennel(&lua)?;
        eval::install_loop_module(&lua)?;
        Ok(Self { lua, fennel })
    }

    /// Load `.loop/machine.fnl`, resolve `:task`/`:plan` file references
    /// against the machine's directory, apply `config` where the machine is
    /// silent, and hash the source.
    ///
    /// `floor` is [`Floor::default`] — the built-in defaults. There is no
    /// `config.fnl` to read first; a machine that wants a different model or
    /// budget says so itself, and a template you copied with `loop init
    /// --from` is how that stops being retyped per ticket.
    pub fn load_machine(&self, path: &Path, floor: &Floor) -> Result<Machine> {
        let source = std::fs::read_to_string(path)
            .map_err(|e| CoreError::io(format!("reading machine file {}", path.display()), e))?;
        self.load_machine_source(path, &source, floor)
    }

    /// The same, for a caller that has already read the file.
    ///
    /// `loop recap` has: it reads and hashes `machine.fnl` to decide whether
    /// the machine on disk is even the one that ran, and on a match then paid
    /// for a second read and a second hash of the identical bytes inside
    /// `load_machine`.
    pub fn load_machine_source(&self, path: &Path, source: &str, floor: &Floor) -> Result<Machine> {
        let filename = path.to_string_lossy().to_string();
        let value = eval::eval_fennel(&self.lua, &self.fennel, source, &filename)?;
        let table = self.value_as_table(value, path)?;
        let source_hash = machine_hash(source);
        let machine_dir = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        convert::machine_from_table(&table, &machine_dir, source_hash, path, floor)
    }

    /// A missing/empty file compiles to Lua `nil`; treat that as an empty
    /// table (every default stands) rather than an error.
    fn value_as_table(&self, value: mlua::Value, path: &Path) -> Result<mlua::Table> {
        match value {
            mlua::Value::Table(t) => Ok(t),
            mlua::Value::Nil => self
                .lua
                .create_table()
                .map_err(|e| CoreError::machine(format!("{}: {e}", path.display()))),
            other => Err(CoreError::machine(format!(
                "{}: expected the file to evaluate to a table, got {}",
                path.display(),
                other.type_name()
            ))),
        }
    }
}
