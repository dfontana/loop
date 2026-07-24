//! Fennel evaluation: `config.fnl` and `machine.fnl` → the `loop-core` IR.
//!
//! The Fennel compiler (`vendor/fennel.lua`, 1.5.3, MIT) is embedded in the
//! binary and loaded into an `mlua` Lua 5.4 VM at startup. A machine file is a
//! Fennel *module* that returns a table; guards inside it are ordinary
//! functions, kept alive in the Lua registry and invoked through
//! [`loop_core::GuardEvaluator`].
//!
//! TASK T2 implements this crate.

use std::path::Path;

use loop_core::{Config, GuardRef, Machine, Paths, Result, Vars};

mod convert;
mod eval;

pub use convert::{config_from_table, machine_from_table};

/// The embedded Fennel compiler.
pub const FENNEL_LUA: &str = include_str!("../vendor/fennel.lua");

/// An initialized Lua VM with Fennel loaded and the `loop` support module in
/// `package.preload`. Holds every guard closure a loaded machine declared.
pub struct FennelVm {
    lua: mlua::Lua,
}

impl FennelVm {
    /// Create the VM, install the Fennel compiler, and register the `loop`
    /// runtime module (`transient?`, `real?`, `mins`, `secs` — the helpers
    /// examples/local/machine.fnl uses).
    ///
    /// The VM is deliberately *not* sandboxed: machine files are authored by
    /// the person running the harness. `os`/`io` stay available so a guard can
    /// shell out if it must.
    pub fn new() -> Result<Self> {
        todo!("T2")
    }

    /// Compile and run a `.fnl` file, returning the table it evaluates to.
    /// Compilation errors must carry the **Fennel** file/line, not the
    /// generated Lua's — that is the documented weakness of this backend
    /// (docs/02-language.md) and the thing to get right.
    pub fn eval_file(&self, path: &Path) -> Result<mlua::Value> {
        let _ = path;
        todo!("T2")
    }

    /// Load `~/.config/loop/config.fnl` over [`Config::defaults`]. A missing
    /// file is not an error — the defaults stand.
    pub fn load_config(&self, paths: Paths) -> Result<Config> {
        let _ = paths;
        todo!("T2")
    }

    /// Load `.loop/machine.fnl`, resolve `:task`/`:plan` file references
    /// against the machine's directory, apply `config` where the machine is
    /// silent, hash the source, and register every guard closure.
    pub fn load_machine(&self, path: &Path, config: &Config) -> Result<Machine> {
        let _ = (path, config);
        todo!("T2")
    }
}

impl loop_core::GuardEvaluator for FennelVm {
    /// Call the registered closure with the vars table. A guard that errors or
    /// returns a non-boolean is a machine authoring bug: surface it as
    /// [`loop_core::CoreError::Guard`], never silently treat it as `false`.
    fn eval(&self, guard: GuardRef, vars: &Vars) -> Result<bool> {
        let _ = (guard, vars);
        todo!("T2")
    }

    fn source(&self, guard: GuardRef) -> Option<String> {
        let _ = guard;
        todo!("T2")
    }
}
