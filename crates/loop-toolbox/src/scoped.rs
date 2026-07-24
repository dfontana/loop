//! Merging `tools/*.yaml` into the one `scoped-tools.yaml` the installed
//! `scoped-tools` extension reads from `$PI_AGENT_DIR`.
//!
//! loop does **not** reimplement scoped-tools (docs/04-toolbox.md) — it only
//! assembles the file and points the extension at it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_yaml_ng::Value;

use loop_core::{IoContext, Result};

/// Read every `*.yaml` in `tools_dir`, merge the top-level tool maps, and write
/// the result to `dest`. Returns the tool names available and any collision
/// warnings.
///
/// Files are merged in alphabetical order so that on a same-named tool the
/// later file wins deterministically. Each entry must be a mapping with a
/// `description` and a `commandTemplate`; anything else — a malformed tool, a
/// file that isn't a top-level mapping, invalid YAML — is dropped with a
/// warning instead of failing the run, matching the extension's own
/// "skip with a warning" behavior at session start.
pub fn merge_tools(tools_dir: &Path, dest: &Path) -> Result<(Vec<String>, Vec<String>)> {
    let mut warnings = Vec::new();
    let mut merged: BTreeMap<String, Value> = BTreeMap::new();
    let mut owner: BTreeMap<String, PathBuf> = BTreeMap::new();

    let mut files: Vec<PathBuf> = Vec::new();
    if tools_dir.is_dir() {
        for entry in std::fs::read_dir(tools_dir)
            .io_ctx(format!("reading tools dir {}", tools_dir.display()))?
        {
            let entry = entry.io_ctx(format!("reading tools dir {}", tools_dir.display()))?;
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("yaml") {
                files.push(path);
            }
        }
    }
    files.sort();

    for file in &files {
        let src = std::fs::read_to_string(file).io_ctx(format!("reading {}", file.display()))?;
        let doc: Value = match serde_yaml_ng::from_str(&src) {
            Ok(v) => v,
            Err(e) => {
                warnings.push(format!("{}: invalid YAML, skipped ({e})", file.display()));
                continue;
            }
        };
        let Some(mapping) = doc.as_mapping() else {
            warnings.push(format!(
                "{}: expected a top-level mapping of tool name -> spec, skipped",
                file.display()
            ));
            continue;
        };

        for (key, value) in mapping {
            let Some(name) = key.as_str() else {
                warnings.push(format!("{}: non-string tool name, skipped", file.display()));
                continue;
            };
            if !is_valid_tool_spec(value) {
                warnings.push(format!(
                    "{}: tool `{name}` is malformed (needs `description` and `commandTemplate`), dropped",
                    file.display()
                ));
                continue;
            }
            if let Some(prev_owner) = owner.get(name) {
                warnings.push(format!(
                    "tool `{name}` defined in both {} and {} — {} wins",
                    prev_owner.display(),
                    file.display(),
                    file.display()
                ));
            }
            merged.insert(name.to_string(), value.clone());
            owner.insert(name.to_string(), file.clone());
        }
    }

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).io_ctx(format!("creating {}", parent.display()))?;
    }
    let out_mapping: Value = Value::Mapping(
        merged
            .iter()
            .map(|(k, v)| (Value::String(k.clone()), v.clone()))
            .collect(),
    );
    let rendered = serde_yaml_ng::to_string(&out_mapping)
        .map_err(|e| loop_core::CoreError::other(e.to_string()))?;
    std::fs::write(dest, rendered).io_ctx(format!("writing {}", dest.display()))?;

    Ok((merged.into_keys().collect(), warnings))
}

fn is_valid_tool_spec(v: &Value) -> bool {
    let Some(m) = v.as_mapping() else {
        return false;
    };
    let has_description = m
        .get("description")
        .and_then(|d| d.as_str())
        .is_some_and(|s| !s.is_empty());
    let has_command = m
        .get("commandTemplate")
        .and_then(|d| d.as_str())
        .is_some_and(|s| !s.is_empty());
    has_description && has_command
}

/// Copy `tools/mcp.json` to `$PI_AGENT_DIR/mcp.json` if it exists. Returns the
/// server names found, for `validate` to report.
pub fn stage_mcp(tools_dir: &Path, agent_dir: &Path) -> Result<Vec<String>> {
    let src = tools_dir.join("mcp.json");
    if !src.is_file() {
        return Ok(Vec::new());
    }

    let content = std::fs::read_to_string(&src).io_ctx(format!("reading {}", src.display()))?;
    let value: serde_json::Value = serde_json::from_str(&content)?;
    let servers = value
        .get("mcpServers")
        .and_then(|v| v.as_object())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();

    std::fs::create_dir_all(agent_dir).io_ctx(format!("creating {}", agent_dir.display()))?;
    std::fs::write(agent_dir.join("mcp.json"), &content)
        .io_ctx(format!("writing {}", agent_dir.join("mcp.json").display()))?;

    Ok(servers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write(dir: &Path, name: &str, content: &str) {
        std::fs::write(dir.join(name), content).unwrap();
    }

    #[test]
    fn merges_two_files_with_no_collisions() {
        let dir = tempdir().unwrap();
        write(
            dir.path(),
            "a.yaml",
            "tool_a:\n  description: A tool\n  commandTemplate: echo a\n",
        );
        write(
            dir.path(),
            "b.yaml",
            "tool_b:\n  description: B tool\n  commandTemplate: echo b\n",
        );
        let dest = dir.path().join("out/scoped-tools.yaml");
        let (names, warnings) = merge_tools(dir.path(), &dest).unwrap();
        assert_eq!(names, vec!["tool_a".to_string(), "tool_b".to_string()]);
        assert!(warnings.is_empty());
        assert!(dest.is_file());
        let out = std::fs::read_to_string(&dest).unwrap();
        assert!(out.contains("tool_a"));
        assert!(out.contains("tool_b"));
    }

    #[test]
    fn same_name_collision_later_file_wins_and_warns() {
        let dir = tempdir().unwrap();
        write(
            dir.path(),
            "a.yaml",
            "shared:\n  description: from a\n  commandTemplate: echo a\n",
        );
        write(
            dir.path(),
            "b.yaml",
            "shared:\n  description: from b\n  commandTemplate: echo b\n",
        );
        let dest = dir.path().join("scoped-tools.yaml");
        let (names, warnings) = merge_tools(dir.path(), &dest).unwrap();
        assert_eq!(names, vec!["shared".to_string()]);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("shared"));
        assert!(warnings[0].contains("a.yaml"));
        assert!(warnings[0].contains("b.yaml"));
        let out = std::fs::read_to_string(&dest).unwrap();
        assert!(out.contains("from b"));
        assert!(!out.contains("from a"));
    }

    #[test]
    fn malformed_tool_is_dropped_with_warning_not_failure() {
        let dir = tempdir().unwrap();
        write(
            dir.path(),
            "mixed.yaml",
            "good_tool:\n  description: fine\n  commandTemplate: echo ok\nbad_tool:\n  description: missing command\n",
        );
        let dest = dir.path().join("scoped-tools.yaml");
        let (names, warnings) = merge_tools(dir.path(), &dest).unwrap();
        assert_eq!(names, vec!["good_tool".to_string()]);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("bad_tool"));
    }

    #[test]
    fn invalid_yaml_file_is_skipped_with_warning() {
        let dir = tempdir().unwrap();
        write(dir.path(), "broken.yaml", "not: [valid: yaml");
        write(
            dir.path(),
            "ok.yaml",
            "tool_ok:\n  description: fine\n  commandTemplate: echo ok\n",
        );
        let dest = dir.path().join("scoped-tools.yaml");
        let (names, warnings) = merge_tools(dir.path(), &dest).unwrap();
        assert_eq!(names, vec!["tool_ok".to_string()]);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("broken.yaml"));
    }

    #[test]
    fn missing_tools_dir_yields_empty_merge() {
        let dir = tempdir().unwrap();
        let tools_dir = dir.path().join("nonexistent");
        let dest = dir.path().join("out/scoped-tools.yaml");
        let (names, warnings) = merge_tools(&tools_dir, &dest).unwrap();
        assert!(names.is_empty());
        assert!(warnings.is_empty());
        assert!(dest.is_file());
    }

    #[test]
    fn stage_mcp_copies_file_and_lists_servers() {
        let src_dir = tempdir().unwrap();
        write(
            src_dir.path(),
            "mcp.json",
            r#"{"mcpServers": {"linear": {"url": "https://example"}, "warehouse": {"command": "npx"}}}"#,
        );
        let agent_dir = tempdir().unwrap();
        let servers = stage_mcp(src_dir.path(), agent_dir.path()).unwrap();
        assert_eq!(servers, vec!["linear".to_string(), "warehouse".to_string()]);
        assert!(agent_dir.path().join("mcp.json").is_file());
    }

    #[test]
    fn stage_mcp_no_op_when_absent() {
        let src_dir = tempdir().unwrap();
        let agent_dir = tempdir().unwrap();
        let servers = stage_mcp(src_dir.path(), agent_dir.path()).unwrap();
        assert!(servers.is_empty());
        assert!(!agent_dir.path().join("mcp.json").exists());
    }
}
