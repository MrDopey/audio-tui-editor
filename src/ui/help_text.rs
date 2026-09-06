//! The `?` help screen's static text (design §3, §4–§11, §18–§20) — pulled
//! out of `overlay` since it is pure content, not rendering logic.

pub(super) fn help_lines() -> Vec<String> {
    [
        "BROWSE",
        "  j/k, ↓/↑        next / previous file",
        "  gg / G          first / last file",
        "  Ctrl-d/Ctrl-u   page down / up",
        "  / n N           search, next match, previous match",
        "  Enter           open in PLAY mode",
        "  r               rescan folder",
        "  q               quit",
        "",
        "PLAY",
        "  space           play / pause",
        "  ←/→, h/l        seek by the small step",
        "  Ctrl-←/→, C-h/l seek by the large step",
        "  g / G           seek to the start / end of the file",
        "  ↑/↓, k/j        volume up / down",
        "  Ctrl-↑/↓, C-k/j next / previous song in this folder",
        "  c               type a jump for the cursor — e.g. 10 or +10s (from",
        "                  here), ++10s/--10s (from start/end), 50%, 10:00",
        "  e               EDIT mode",
        "  m               METADATA mode",
        "  Esc, q          back to BROWSE",
        "",
        "EDIT",
        "  ←/→, h/l        move the cursor (playback position); the marker",
        "                  currently hugging it (bold, underlined) moves too",
        "  Ctrl-←/→, C-h/l move the cursor (large step)",
        "  Ctrl-↑/↓, C-k/j next / previous song in this folder",
        "  Tab             switch which marker hugs the cursor, picking it",
        "                  up from its own position first",
        "  b / e           type a jump for the beginning / ending marker:",
        "                  moves the cursor there and makes it the one",
        "                  hugging — e.g. 10 or +10s (from THIS marker's own",
        "                  position), ++10s/--10s (from start/end), 50%, 10:00",
        "  c               same jump, for the cursor alone (relative to the",
        "                  cursor itself) — doesn't change which marker is",
        "                  hugging",
        "  g               seek playback to the active marker, without playing",
        "  a               recalculate automatic markers",
        "  r               reset BOTH markers to the whole file (see METADATA's",
        "                  u, which reverts one field at a time, not everything)",
        "  p               play from the active marker",
        "  Esc, q          back to PLAY",
        "",
        "  Dragging one marker past the other is allowed briefly — it",
        "  corrects (Begin/End swap, keeping the range's width) once you",
        "  stop moving, switch focus (Tab), or save.",
        "",
        "METADATA",
        "  j/k             next / previous field (every tag the file",
        "                  carries: the preconfigured fields, then any",
        "                  extras alphabetically)",
        "  Ctrl-↑/↓, C-k/j next / previous song in this folder",
        "  / n N           search fields, next match, previous match",
        "  Enter or i      edit the field",
        "  u               revert the field",
        "  Esc, q          back to PLAY",
        "",
        "COMMANDS",
        "  :w              save in place",
        "  :q              leave",
        "  :wq             save and leave",
        "  :q!             leave, discarding changes",
        "  :b <pos>        set the beginning marker: +/- from its own",
        "                  position (same as the b prompt), ++/-- from",
        "                  start/end, e.g. :b +10s",
        "  :e <pos>        set the ending marker, same grammar as :b",
        "  :auto           recalculate automatic markers",
        "  :reset          reset markers to the whole file",
        "  :apply-defaults           trim every file in the folder",
        "  :apply-defaults --dry-run report what would change, writing nothing",
        "  :help           this screen",
        "",
        "GLOBAL",
        "  Ctrl-C (x2)     quit immediately, discarding unsaved changes",
        "",
        "[Esc] close",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect()
}

#[cfg(test)]
mod tests {
    use super::help_lines;

    #[test]
    fn help_covers_every_documented_command() {
        let help = help_lines().join("\n");
        for command in [":w", ":q", ":wq", ":help", ":apply-defaults", "--dry-run"] {
            assert!(help.contains(command), "help is missing {command}");
        }
    }
}
