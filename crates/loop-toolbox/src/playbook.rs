//! Playbooks are pi skills: YAML frontmatter + a markdown body.

use std::path::PathBuf;

use serde::Deserialize;

use loop_core::{CoreError, Result, Thinking};

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
}

/// The frontmatter fields we care about. Anything else the author put in the
/// block (e.g. a pi-skill `tags:`) is ignored, not rejected.
#[derive(Debug, Default, Deserialize)]
struct Frontmatter {
    name: Option<String>,
    description: Option<String>,
    model: Option<String>,
    thinking: Option<Thinking>,
}

/// Split `---\n<yaml>\n---\n<body>`. A file without frontmatter is all body.
pub fn parse(name: &str, source: &str, path: Option<PathBuf>) -> Result<ResolvedPlaybook> {
    let (frontmatter_src, body) = split_frontmatter(source);

    let fm: Frontmatter = match frontmatter_src {
        Some(fm_src) => serde_yaml_ng::from_str(fm_src).map_err(|e| {
            CoreError::machine(format!("playbook `{name}` has malformed frontmatter: {e}"))
        })?,
        None => Frontmatter::default(),
    };

    Ok(ResolvedPlaybook {
        name: fm.name.unwrap_or_else(|| name.to_string()),
        path,
        body: body.to_string(),
        description: fm.description,
        model: fm.model,
        thinking: fm.thinking,
    })
}

/// Returns `(Some(yaml_source), body)` when `source` opens with a `---`
/// delimiter line; otherwise `(None, source)` unchanged.
///
/// The closing delimiter is the *first* line equal to `---` after the opening
/// one — found by a single linear scan — so any `---` occurring later in the
/// body (e.g. a markdown horizontal rule) is never considered, and is returned
/// as part of `body` untouched.
fn split_frontmatter(source: &str) -> (Option<&str>, &str) {
    let first_line_end = source.find('\n').unwrap_or(source.len());
    if source[..first_line_end].trim_end_matches('\r') != "---" {
        return (None, source);
    }
    let after_open = if first_line_end < source.len() {
        &source[first_line_end + 1..]
    } else {
        ""
    };

    let mut offset = 0usize;
    for line in after_open.split('\n') {
        if line.trim_end_matches('\r') == "---" {
            let yaml_src = &after_open[..offset];
            let body_start = (offset + line.len() + 1).min(after_open.len());
            let body = after_open[body_start..].trim_start_matches('\n');
            return (Some(yaml_src), body);
        }
        offset += line.len() + 1;
    }
    // Opened but never closed: treat the whole thing as an unparsed body
    // rather than failing the run over a stray leading `---`.
    (None, source)
}

#[cfg(test)]
mod tests {
    use super::*;
    use loop_core::Thinking;

    #[test]
    fn parses_frontmatter_fields() {
        let src = "---\nname: implement\ndescription: Do the thing.\nmodel: claude-sonnet-5\nthinking: high\n---\n\n# Implement\n\nBody text.\n";
        let pb = parse("implement", src, Some(PathBuf::from("implement.md"))).unwrap();
        assert_eq!(pb.name, "implement");
        assert_eq!(pb.description.as_deref(), Some("Do the thing."));
        assert_eq!(pb.model.as_deref(), Some("claude-sonnet-5"));
        assert_eq!(pb.thinking, Some(Thinking::High));
        assert_eq!(pb.body, "# Implement\n\nBody text.\n");
    }

    #[test]
    fn no_frontmatter_is_all_body() {
        let src = "# Just a body\n\nNo frontmatter here.\n";
        let pb = parse("bare", src, None).unwrap();
        assert_eq!(pb.name, "bare");
        assert_eq!(pb.body, src);
        assert!(pb.description.is_none());
        assert!(pb.model.is_none());
        assert!(pb.thinking.is_none());
    }

    #[test]
    fn missing_optional_frontmatter_fields_are_none() {
        let src = "---\nname: qa\n---\nBody.\n";
        let pb = parse("qa", src, None).unwrap();
        assert_eq!(pb.name, "qa");
        assert!(pb.description.is_none());
        assert!(pb.model.is_none());
        assert!(pb.thinking.is_none());
    }

    #[test]
    fn body_dashes_are_not_mistaken_for_a_second_delimiter() {
        let src = "---\nname: review\ndescription: d\n---\n\n# Review\n\nabove the rule\n\n---\n\nbelow the rule\n";
        let pb = parse("review", src, None).unwrap();
        assert_eq!(pb.name, "review");
        assert_eq!(pb.description.as_deref(), Some("d"));
        assert!(pb.body.contains("above the rule"));
        assert!(pb.body.contains("---"));
        assert!(pb.body.contains("below the rule"));
    }
}
