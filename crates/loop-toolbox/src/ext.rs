//! loop's own vendored pi extensions — the three tools that carry every
//! decision as structured data rather than prose the harness has to parse.

use std::path::PathBuf;

pub const TRANSITION_TOOL_TS: &str = include_str!("../ext/transition-tool.ts");
pub const VERDICT_TOOL_TS: &str = include_str!("../ext/verdict-tool.ts");
pub const CHOOSE_TOOL_TS: &str = include_str!("../ext/choose-tool.ts");

#[derive(Clone, Debug)]
pub struct ExtPaths {
    /// `-e`'d into every Worker spawn.
    pub transition: PathBuf,
    /// `-e`'d into the Judge spawn (which has no other tools).
    pub verdict: PathBuf,
    /// `-e`'d into the Navigator spawn.
    pub choose: PathBuf,
}
