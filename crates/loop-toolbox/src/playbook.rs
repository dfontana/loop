//! Playbooks are pi skills: YAML frontmatter + a markdown body.

use std::path::PathBuf;

use loop_core::{Result, Thinking};

#[derive(Clone, Debug, Default)]
pub struct ResolvedPlaybook {
    pub name: String,
    /// `None` for an inline prompt.
    pub path: Option<PathBuf>,
    /// The markdown body, frontmatter stripped, **not yet rendered**.
    pub body: String,
    pub description: Option<String>,
    pub model: Option<String>,
    pub thinking: Option<Thinking>,
    /// sha256 of the file, pinned into the `run_started` snapshot so a mid-run
    /// toolbox edit cannot change behavior (docs/07-risks.md #14).
    pub sha256: String,
}

/// Split `---\n<yaml>\n---\n<body>`. A file without frontmatter is all body.
///
/// TASK T3.
pub fn parse(name: &str, source: &str, path: Option<PathBuf>) -> Result<ResolvedPlaybook> {
    let _ = (name, source, path);
    todo!("T3")
}
