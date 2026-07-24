//! Fennel compilation plumbing: install the compiler, compile a `.fnl` source
//! to Lua, run it, and map errors back to Fennel source positions.

use loop_core::Result;

/// Install `fennel.lua` into `package.preload` and return the `fennel` table.
///
/// TASK T2.
pub fn install_fennel(lua: &mlua::Lua) -> Result<mlua::Table> {
    let _ = lua;
    todo!("T2")
}

/// Register the `loop` runtime module machine files may `require`: the
/// helpers examples/local/machine.fnl uses — `transient?`, `real?`, `mins`,
/// `secs`. Keep it tiny; it is a convenience layer, not an API surface.
///
/// TASK T2.
pub fn install_loop_module(lua: &mlua::Lua) -> Result<()> {
    let _ = lua;
    todo!("T2")
}

/// `fennel.compileString` with `{:filename path :correlate true}` so runtime
/// errors carry Fennel line numbers, then `load` + call the chunk.
///
/// TASK T2.
pub fn eval_fennel(lua: &mlua::Lua, source: &str, filename: &str) -> Result<mlua::Value> {
    let _ = (lua, source, filename);
    todo!("T2")
}
