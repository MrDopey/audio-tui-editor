//! Small text-formatting helpers shared across the UI and batch reporting.

/// Shorten `text` to fit `width`, keeping a trailing ellipsis when it
/// doesn't. Character-counted, not byte-counted, so multi-byte names are not
/// cut mid-character.
pub(crate) fn truncate_with_ellipsis(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let keep = width.saturating_sub(1);
    let truncated: String = text.chars().take(keep).collect();
    format!("{truncated}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_truncated_with_an_ellipsis() {
        assert_eq!(truncate_with_ellipsis("short.opus", 20), "short.opus");
        assert_eq!(
            truncate_with_ellipsis("a-very-long-name.opus", 10),
            "a-very-lo…"
        );
    }
}
