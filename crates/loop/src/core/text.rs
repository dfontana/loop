//! Shaping arbitrary text for display.
//!
//! Everything here bounds something a worker, a model, or somebody else's build
//! tool wrote, so that it fits a terminal row, a mermaid label, or a prompt.
//!
//! Whether newlines survive is the one real distinction, so it is a separate
//! function rather than a flag: [`truncate`] preserves the text it bounds (it
//! feeds prompts and the ledger), [`one_line`] flattens it, and [`brief`] is
//! both, which is what every terminal row wants.
//!
//! Under `core` rather than beside the CLI because `engine` bounds text too —
//! `mermaid` flattens a ticket name into a node label — and `engine` imports
//! nothing but `core` (AGENTS.md). Four pure functions over `&str`, no I/O and
//! no types of its own, so it costs `core` nothing to hold them and the
//! alternative is a second copy that drifts.

/// The first non-blank line, trimmed. `""` when there isn't one.
pub fn first_line(s: &str) -> &str {
    s.lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
}

/// Every run of whitespace — newlines and tabs included — collapsed to a single
/// space. What keeps a chatty worker's summary inside one row of a listing, and
/// a ticket name inside one mermaid label.
pub fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// At most `max` characters, with a trailing `…` marking anything cut.
///
/// Counts characters, not bytes, and never splits a codepoint. Text is
/// otherwise preserved: this bounds values that land in prompts and in ledger
/// events, where a newline is content.
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    // The ellipsis is part of the budget, so the result is `max` characters —
    // a caller sizing a column gets the width it asked for.
    let head: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{head}…")
}

/// One line, bounded — the terminal-row form of [`truncate`].
pub fn brief(s: &str, max: usize) -> String {
    truncate(&one_line(s), max)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `:criteria` written with a leading blank line still has to render as
    /// its first real line, in the graph summary as well as in preview.
    #[test]
    fn first_line_skips_leading_blanks() {
        assert_eq!(first_line("\n\n  the real line\nmore"), "the real line");
        assert_eq!(first_line("first\nsecond"), "first");
        assert_eq!(first_line("   \n\t\n"), "");
        assert_eq!(first_line(""), "");
    }

    #[test]
    fn one_line_collapses_every_kind_of_whitespace() {
        assert_eq!(one_line("a\nb\tc   d"), "a b c d");
        assert_eq!(one_line("  padded  "), "padded");
        assert_eq!(one_line(""), "");
    }

    /// A caller sizing a column gets the width it asked for, ellipsis included.
    #[test]
    fn truncate_result_never_exceeds_max() {
        assert_eq!(truncate("abcdef", 4).chars().count(), 4);
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("abcd", 4), "abcd", "exactly max is untouched");
        assert_eq!(truncate("ab", 4), "ab");
    }

    #[test]
    fn truncate_preserves_newlines_and_multibyte_text() {
        assert_eq!(truncate("a\nb", 10), "a\nb");
        // Counting characters, not bytes: each of these is 3 bytes.
        assert_eq!(truncate("日本語テスト", 4), "日本語…");
    }

    #[test]
    fn brief_is_one_line_and_bounded() {
        assert_eq!(brief("first line\nsecond line", 12), "first line …");
        assert_eq!(brief("short", 40), "short");
    }
}
