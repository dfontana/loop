//! Merging `tools/*.yaml` into the one `scoped-tools.yaml` the installed
//! `scoped-tools` extension reads from `$PI_AGENT_DIR`.
//!
//! loop does **not** reimplement scoped-tools (docs/04-toolbox.md) — it only
//! assembles the file and points the extension at it.

use std::path::Path;

use loop_core::Result;

/// Read every `*.yaml` in `tools_dir`, merge the top-level tool maps, and write
/// the result to `dest`. Returns the tool names available and any collision
/// warnings.
///
/// TASK T3. Validate minimally: each entry must be a map with `description`
/// and `commandTemplate`. A malformed tool is dropped with a warning rather
/// than failing the run — that matches the extension's own behavior.
pub fn merge_tools(tools_dir: &Path, dest: &Path) -> Result<(Vec<String>, Vec<String>)> {
    let _ = (tools_dir, dest);
    todo!("T3")
}

/// Copy `tools/mcp.json` to `$PI_AGENT_DIR/mcp.json` if it exists. Returns the
/// server names found, for `validate` to report.
///
/// TASK T3.
pub fn stage_mcp(tools_dir: &Path, agent_dir: &Path) -> Result<Vec<String>> {
    let _ = (tools_dir, agent_dir);
    todo!("T3")
}
