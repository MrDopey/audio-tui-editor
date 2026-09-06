//! Best-effort, env-var-only detection of Kitty graphics protocol support.
//!
//! Deliberately conservative: a false negative just means the plain-text
//! "cover art" fact is shown instead of an image (safe), while a false
//! positive means garbled escape-sequence text on screen (bad) — so every
//! check here is an exact match against a value only a genuinely capable
//! terminal sets, never a substring/prefix match that a terminal
//! multiplexer's rewritten `TERM` (e.g. tmux's `screen.xterm-kitty`) could
//! also satisfy.

/// Whether the current terminal looks like it supports the Kitty graphics
/// protocol (kitty itself, WezTerm, Ghostty).
pub fn kitty_graphics_supported() -> bool {
    supported(|key| std::env::var(key).ok())
}

fn supported(lookup: impl Fn(&str) -> Option<String>) -> bool {
    if lookup("KITTY_WINDOW_ID").is_some() {
        return true;
    }
    if lookup("TERM").as_deref() == Some("xterm-kitty") {
        return true;
    }
    matches!(
        lookup("TERM_PROGRAM").as_deref(),
        Some("WezTerm") | Some("ghostty")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: BTreeMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |key: &str| map.get(key).cloned()
    }

    #[test]
    fn kitty_window_id_is_sufficient_on_its_own() {
        assert!(supported(env(&[("KITTY_WINDOW_ID", "1")])));
    }

    #[test]
    fn exact_xterm_kitty_term_is_supported() {
        assert!(supported(env(&[("TERM", "xterm-kitty")])));
    }

    #[test]
    fn wezterm_and_ghostty_term_program_are_supported() {
        assert!(supported(env(&[("TERM_PROGRAM", "WezTerm")])));
        assert!(supported(env(&[("TERM_PROGRAM", "ghostty")])));
    }

    #[test]
    fn plain_term_is_not_supported() {
        assert!(!supported(env(&[("TERM", "xterm-256color")])));
        assert!(!supported(env(&[])));
    }

    #[test]
    fn a_multiplexer_mangled_term_is_not_mistaken_for_support() {
        // tmux rewrites TERM to something like "screen.xterm-kitty" and does
        // not propagate KITTY_WINDOW_ID -- an exact match on TERM is what
        // keeps this a safe false negative instead of a false positive.
        assert!(!supported(env(&[("TERM", "screen.xterm-kitty")])));
    }
}
