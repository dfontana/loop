//! Fennel compilation plumbing: install the compiler, compile a `.fnl` source
//! to Lua, run it, and map errors back to Fennel source positions.

use crate::core::{CoreError, Result};

use crate::fennel::FENNEL_LUA;

fn lua_err(e: mlua::Error) -> CoreError {
    CoreError::other(format!("lua: {e}"))
}

/// Install `fennel.lua` into `package.preload` and return the `fennel` table.
pub fn install_fennel(lua: &mlua::Lua) -> Result<mlua::Table> {
    let globals = lua.globals();
    let package: mlua::Table = globals.get("package").map_err(lua_err)?;
    let preload: mlua::Table = package.get("preload").map_err(lua_err)?;

    let loader = lua
        .load(FENNEL_LUA)
        .set_name("=(fennel.lua)")
        .into_function()
        .map_err(|e| CoreError::other(format!("loading vendored fennel.lua: {e}")))?;
    preload.set("fennel", loader).map_err(lua_err)?;

    let require: mlua::Function = globals.get("require").map_err(lua_err)?;
    let fennel: mlua::Table = require
        .call("fennel")
        .map_err(|e| CoreError::other(format!("initializing fennel compiler: {e}")))?;
    Ok(fennel)
}

/// The `fennel` module table, from `package.loaded` if already installed,
/// otherwise installing it fresh.
fn fennel_table(lua: &mlua::Lua) -> Result<mlua::Table> {
    let package: mlua::Table = lua.globals().get("package").map_err(lua_err)?;
    let loaded: mlua::Table = package.get("loaded").map_err(lua_err)?;
    match loaded.get::<mlua::Value>("fennel").map_err(lua_err)? {
        mlua::Value::Table(t) => Ok(t),
        _ => install_fennel(lua),
    }
}

/// True when `scope` is a table whose `error_class` field equals `expected`.
fn error_class_is(scope: &mlua::Value, expected: &str) -> bool {
    if let mlua::Value::Table(t) = scope {
        if let Ok(mlua::Value::String(s)) = t.get::<mlua::Value>("error_class") {
            return s.to_string_lossy() == expected;
        }
    }
    false
}

/// Register the `loop` runtime module machine files may `require`: the
/// helpers examples/local/machine.fnl uses — `transient?`, `real?`, `mins`,
/// `secs`. Keep it tiny; it is a convenience layer, not an API surface.
pub fn install_loop_module(lua: &mlua::Lua) -> Result<()> {
    let module = lua.create_table().map_err(lua_err)?;

    let transient = lua
        .create_function(|_, scope: mlua::Value| Ok(error_class_is(&scope, "transient")))
        .map_err(lua_err)?;
    module.set("transient?", transient).map_err(lua_err)?;

    let real = lua
        .create_function(|_, scope: mlua::Value| Ok(!error_class_is(&scope, "transient")))
        .map_err(lua_err)?;
    module.set("real?", real).map_err(lua_err)?;

    let mins = lua
        .create_function(|_, n: f64| Ok(n * 60.0))
        .map_err(lua_err)?;
    module.set("mins", mins).map_err(lua_err)?;

    let secs = lua.create_function(|_, n: f64| Ok(n)).map_err(lua_err)?;
    module.set("secs", secs).map_err(lua_err)?;

    let package: mlua::Table = lua.globals().get("package").map_err(lua_err)?;
    let preload: mlua::Table = package.get("preload").map_err(lua_err)?;
    let loader = lua
        .create_function(move |_, _: mlua::MultiValue| Ok(module.clone()))
        .map_err(lua_err)?;
    preload.set("loop", loader).map_err(lua_err)?;
    Ok(())
}

/// `fennel.eval` with `{:filename path :correlate true}` so both compile-time
/// errors and any runtime error inside a compiled closure (a guard included)
/// carry Fennel line numbers rather than positions in the generated Lua.
///
/// This is the documented weak point of the Fennel backend (docs/05-design-notes.md
/// §"Weaker static analysis") and the thing this function exists to defeat:
/// `correlate` makes the compiler emit one Lua line per Fennel top-level form,
/// and we load the chunk under the `.fnl` filename, so `chunkname:line` in any
/// error — compile or runtime — already *is* a Fennel source position.
pub fn eval_fennel(lua: &mlua::Lua, source: &str, filename: &str) -> Result<mlua::Value> {
    let fennel = fennel_table(lua)?;
    let eval_fn: mlua::Function = fennel
        .get("eval")
        .map_err(|e| CoreError::machine(format!("internal: fennel.eval missing: {e}")))?;

    let opts = lua.create_table().map_err(lua_err)?;
    opts.set("filename", filename).map_err(lua_err)?;
    opts.set("correlate", true).map_err(lua_err)?;

    eval_fn
        .call::<mlua::Value>((source.to_string(), opts))
        .map_err(|e| CoreError::machine(fennel_error_message(filename, &e)))
}

/// Fennel's own compile/parse errors already embed `filename:line:col`; only
/// add our own prefix if, for some other reason, the message doesn't carry it.
fn fennel_error_message(filename: &str, err: &mlua::Error) -> String {
    let msg = err.to_string();
    if msg.contains(filename) {
        msg
    } else {
        format!("{filename}: {msg}")
    }
}
