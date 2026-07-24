//! Fennel evaluation: `config.fnl` and `machine.fnl` → the `loop-core` IR.
//!
//! The Fennel compiler (`vendor/fennel.lua`, 1.5.3, MIT) is embedded in the
//! binary and loaded into an `mlua` Lua 5.4 VM at startup. A machine file is a
//! Fennel *module* that returns a table; guards inside it are ordinary
//! functions, kept alive in the Lua registry and invoked through
//! [`loop_core::GuardEvaluator`].

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use mlua::LuaSerdeExt as _;
use sha2::{Digest, Sha256};

use loop_core::{Config, CoreError, GuardRef, Machine, Paths, Result, Vars};

mod convert;
mod eval;

pub use convert::{config_from_table, machine_from_table};

/// The embedded Fennel compiler.
pub const FENNEL_LUA: &str = include_str!("../vendor/fennel.lua");

struct GuardEntry {
    key: mlua::RegistryKey,
    /// Best-effort human-readable label: the author's `:when-doc`, or a
    /// `file:line` derived from the closure's debug info.
    src: Option<String>,
}

/// An initialized Lua VM with Fennel loaded and the `loop` support module in
/// `package.preload`. Holds every guard closure a loaded machine declared.
pub struct FennelVm {
    lua: mlua::Lua,
    guards: RefCell<Vec<GuardEntry>>,
}

impl FennelVm {
    /// Create the VM, install the Fennel compiler, and register the `loop`
    /// runtime module (`transient?`, `real?`, `mins`, `secs` — the helpers
    /// examples/local/machine.fnl uses).
    ///
    /// The VM is deliberately *not* sandboxed: machine files are authored by
    /// the person running the harness. `os`/`io` stay available so a guard can
    /// shell out if it must. The vendored Fennel compiler also needs the
    /// `debug` library at `require` time (for `debug.getinfo`/`traceback`), so
    /// this uses `Lua::unsafe_new` rather than the safe-subset constructor.
    pub fn new() -> Result<Self> {
        // SAFETY: no untrusted code is ever loaded into this VM — machine and
        // config files are authored by the person invoking the harness, and
        // full stdlib access (including `debug`) is required by fennel.lua
        // itself and is an explicit, documented design choice (docs/09).
        let lua = unsafe { mlua::Lua::unsafe_new() };
        eval::install_fennel(&lua)?;
        eval::install_loop_module(&lua)?;
        Ok(Self {
            lua,
            guards: RefCell::new(Vec::new()),
        })
    }

    /// Compile and run a `.fnl` file, returning the table it evaluates to.
    /// Compilation errors must carry the **Fennel** file/line, not the
    /// generated Lua's — that is the documented weakness of this backend
    /// (docs/02-language.md) and the thing to get right.
    pub fn eval_file(&self, path: &Path) -> Result<mlua::Value> {
        let source = std::fs::read_to_string(path)
            .map_err(|e| CoreError::io(format!("reading {}", path.display()), e))?;
        let filename = path.to_string_lossy().to_string();
        eval::eval_fennel(&self.lua, &source, &filename)
    }

    /// Load `~/.config/loop/config.fnl` over [`Config::defaults`]. A missing
    /// file is not an error — the defaults stand.
    pub fn load_config(&self, paths: Paths) -> Result<Config> {
        let base = Config::defaults(paths);
        let path = base.paths.config_file();
        if !path.is_file() {
            return Ok(base);
        }
        let value = self.eval_file(&path)?;
        let table = self.value_as_table(value, &path)?;
        convert::config_from_table(&table, base)
    }

    /// Load `.loop/machine.fnl`, resolve `:task`/`:plan` file references
    /// against the machine's directory, apply `config` where the machine is
    /// silent, hash the source, and register every guard closure.
    pub fn load_machine(&self, path: &Path, config: &Config) -> Result<Machine> {
        let source = std::fs::read_to_string(path)
            .map_err(|e| CoreError::io(format!("reading machine file {}", path.display()), e))?;
        let filename = path.to_string_lossy().to_string();
        let value = eval::eval_fennel(&self.lua, &source, &filename)?;
        let table = self.value_as_table(value, path)?;
        let source_hash = hex::encode(Sha256::digest(source.as_bytes()));
        let machine_dir = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        convert::machine_from_table(self, &table, &machine_dir, source_hash, path, config)
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

    /// Keep a guard closure alive in the Lua registry and hand back an opaque
    /// handle. `src` is the best-effort human-readable label used in error
    /// messages and the ledger.
    pub(crate) fn register_guard(
        &self,
        f: mlua::Function,
        src: Option<String>,
    ) -> Result<GuardRef> {
        let key = self
            .lua
            .create_registry_value(f)
            .map_err(|e| CoreError::machine(format!("registering guard closure: {e}")))?;
        let mut guards = self.guards.borrow_mut();
        guards.push(GuardEntry { key, src });
        Ok(GuardRef((guards.len() - 1) as u32))
    }
}

impl loop_core::GuardEvaluator for FennelVm {
    /// Call the registered closure with the vars table. A guard that errors or
    /// returns a non-boolean is a machine authoring bug: surface it as
    /// [`loop_core::CoreError::Guard`], never silently treat it as `false`.
    fn eval(&self, guard: GuardRef, vars: &Vars) -> Result<bool> {
        let (label, f) = {
            let guards = self.guards.borrow();
            let entry = guards
                .get(guard.0 as usize)
                .ok_or_else(|| CoreError::Guard {
                    guard: format!("#{}", guard.0),
                    detail: "unknown guard reference".into(),
                })?;
            let label = entry.src.clone().unwrap_or_else(|| format!("#{}", guard.0));
            let f: mlua::Function =
                self.lua
                    .registry_value(&entry.key)
                    .map_err(|e| CoreError::Guard {
                        guard: label.clone(),
                        detail: format!("resolving guard closure: {e}"),
                    })?;
            (label, f)
        };

        let vars_value = self
            .lua
            .to_value(&vars.as_value())
            .map_err(|e| CoreError::Guard {
                guard: label.clone(),
                detail: format!("marshalling vars: {e}"),
            })?;

        let result: mlua::Value = f.call(vars_value).map_err(|e| CoreError::Guard {
            guard: label.clone(),
            detail: e.to_string(),
        })?;

        match result {
            mlua::Value::Boolean(b) => Ok(b),
            other => Err(CoreError::Guard {
                guard: label,
                detail: format!("guard must return a boolean, got {}", other.type_name()),
            }),
        }
    }

    fn source(&self, guard: GuardRef) -> Option<String> {
        self.guards
            .borrow()
            .get(guard.0 as usize)
            .and_then(|e| e.src.clone())
    }
}
