//! Modal overlays: the startup warning, help, save summary, errors,
//! confirmations, and folder-run progress (design §3, §16, §17).

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph, Wrap};
use ratatui::Frame;

use super::{ACCENT, RETAINED};
use crate::app::{App, Overlay};
use crate::media::first_line;

pub(super) fn render_overlay(frame: &mut Frame, app: &mut App) {
    // Build the popup content first so nothing borrows `app` while it is
    // updated with the measurements the scroll handler needs.
    let popup = match &app.overlay {
        Overlay::None => None,
        Overlay::Warning => Some((
            " WARNING ".to_string(),
            Color::Yellow,
            warning_lines(app),
            66,
        )),
        Overlay::Help => Some((" Help ".to_string(), ACCENT, help_lines(), 78)),
        Overlay::Summary(lines) => Some((" Save ".to_string(), RETAINED, lines.clone(), 60)),
        Overlay::Working(message) => {
            Some((" Working ".to_string(), ACCENT, vec![message.clone()], 46))
        }
        Overlay::Error {
            message,
            detail,
            showing_detail,
        } => {
            let mut lines: Vec<String> = vec!["ERROR".to_string(), String::new()];
            lines.extend(message.lines().map(str::to_string));
            lines.push(String::new());
            if *showing_detail {
                lines.extend(detail.lines().map(str::to_string));
                lines.push(String::new());
                lines.push("[Esc] close".to_string());
            } else {
                lines.push("[Enter] details    [Esc] close".to_string());
            }
            Some((" Error ".to_string(), Color::Red, lines, 74))
        }
        Overlay::ConfirmApply => Some((
            " Apply defaults ".to_string(),
            Color::Yellow,
            app.apply_confirmation_lines(),
            66,
        )),
        Overlay::ConfirmDiscard(_) => Some((
            " Unsaved changes ".to_string(),
            Color::Yellow,
            vec![
                "This file has unsaved changes.".to_string(),
                String::new(),
                "[Enter] discard them    [w] save first    [Esc] cancel".to_string(),
            ],
            62,
        )),
        Overlay::Batch(view) => {
            let mut lines: Vec<String> = Vec::new();
            let heading = if view.mode.is_dry_run() {
                "Dry run — nothing is written"
            } else {
                "Applying automatic trim"
            };
            lines.push(format!("{heading}  ({}/{})", view.items.len(), view.total));
            lines.push(String::new());

            // A window of results ending at the scroll position.
            let visible = 12usize;
            let end = (view.scroll + 1).min(view.items.len());
            let start = end.saturating_sub(visible);
            lines.extend(view.items[start..end].iter().map(|i| i.line()));

            if let Some(report) = &view.report {
                lines.push(String::new());
                lines.extend(report.summary_lines());
                lines.push(String::new());
                lines.push("j/k scroll    [Esc] close".to_string());
            }
            Some((
                format!(" Folder run — {} ", view.mode.label()),
                ACCENT,
                lines,
                80,
            ))
        }
    };

    let Some((title, colour, lines, percent_x)) = popup else {
        return;
    };
    let (total, visible) = draw_popup(frame, &title, colour, &lines, percent_x, app.overlay_scroll);
    app.overlay_lines = total;
    app.overlay_view_rows = visible;
}

/// The in-place editing warning shown at startup (design §3).
const WARNING_BODY: &[&str] = &[
    "Saving edits modifies the original audio file in place.",
    "",
    "A temporary output is created and verified before the",
    "original file is replaced.",
    "",
    "Metadata is preserved where supported by the source format.",
];

fn warning_lines(app: &App) -> Vec<String> {
    let mut lines: Vec<String> = WARNING_BODY.iter().map(|l| (*l).to_string()).collect();
    lines.push(String::new());
    lines.push(format!("Folder: {}", app.folder.display()));
    lines.push(format!("Files:  {}", app.files.len()));
    if let Some(error) = app.audio_device_error() {
        lines.push(String::new());
        lines.push(format!("No audio device ({}).", first_line(error)));
        lines.push("Everything except sound output still works.".to_string());
    }
    lines.push(String::new());
    lines.push("[Enter] continue".to_string());
    lines
}

fn help_lines() -> Vec<String> {
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
        "  e               EDIT mode          m   METADATA mode",
        "  Esc, q          back to BROWSE",
        "",
        "EDIT",
        "  ←/→, h/l        move the active marker (fine step)",
        "  Ctrl-←/→, C-h/l move the active marker (large step)",
        "  Ctrl-↑/↓, C-k/j next / previous song in this folder",
        "  Tab             switch between the beginning and ending marker",
        "  b / e           set the beginning / ending marker at the playhead",
        "  B / E           type a position for the beginning / ending marker",
        "  i               type a position for the active marker (see Tab)",
        "                  all three accept: 10:00, +10s, -1m, 50%",
        "  a               recalculate automatic markers",
        "  r               reset BOTH markers to the whole file (see METADATA's",
        "                  u, which reverts one field at a time, not everything)",
        "  p               play from the active marker",
        "  Esc, q          back to PLAY",
        "",
        "METADATA",
        "  j/k             next / previous field",
        "  Ctrl-↑/↓, C-k/j next / previous song in this folder",
        "  Enter or i      edit the field       u   revert the field",
        "  Esc, q          back to PLAY",
        "",
        "COMMANDS",
        "  :w              save in place        :q   leave    :wq  save and leave",
        "  :q!             leave, discarding changes",
        "  :b <pos>        set the beginning marker   e.g. :b +10s",
        "  :e <pos>        set the ending marker      e.g. :e -10s",
        "  :auto           recalculate automatic markers",
        "  :reset          reset markers to the whole file",
        "  :apply-defaults           trim every file in the folder",
        "  :apply-defaults --dry-run report what would change, writing nothing",
        "  :help           this screen",
        "",
        "[Esc] close",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect()
}

/// Draw a centred popup sized to its content.
///
/// Returns the number of rows the content needs and the number visible, so the
/// key handler can clamp scrolling for overlays taller than the terminal.
fn draw_popup(
    frame: &mut Frame,
    title: &str,
    colour: Color,
    lines: &[String],
    percent_x: u16,
    scroll: u16,
) -> (usize, usize) {
    let screen = frame.area();
    if screen.width < 8 || screen.height < 4 {
        return (0, 0);
    }

    let width = (screen.width * percent_x / 100).clamp(24.min(screen.width), screen.width);
    // Two border columns plus one column of padding on each side.
    let inner_width = width.saturating_sub(4).max(1) as usize;
    let content_rows: usize = lines.iter().map(|l| wrapped_rows(l, inner_width)).sum();

    let height = ((content_rows + 2) as u16).clamp(3, screen.height);
    let visible_rows = height.saturating_sub(2) as usize;

    let area = Rect {
        x: screen.x + screen.width.saturating_sub(width) / 2,
        y: screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, area);

    let text: Vec<Line> = lines.iter().map(|l| Line::raw(l.clone())).collect();
    let mut title = title.to_string();
    if content_rows > visible_rows {
        title.push_str("(j/k scroll) ");
    }
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(colour))
        .padding(Padding::horizontal(1))
        .title(title)
        .title_alignment(Alignment::Left)
        .title_style(Style::default().fg(colour).add_modifier(Modifier::BOLD));

    frame.render_widget(
        Paragraph::new(text)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        area,
    );

    (content_rows, visible_rows)
}

/// Rows a line occupies once word-wrapped to `width`.
///
/// Mirrors the greedy wrapping the paragraph widget performs, so a popup is
/// sized tall enough for its own content. Character wrapping under-counts:
/// a long word pushed onto the next row costs a row the character count hides.
fn wrapped_rows(line: &str, width: usize) -> usize {
    if width == 0 || line.is_empty() {
        return 1;
    }
    let mut rows = 1usize;
    let mut used = 0usize;

    for word in line.split(' ') {
        let len = word.chars().count();
        let fits = if used == 0 {
            len <= width
        } else {
            used + 1 + len <= width
        };
        if fits {
            used += if used == 0 { len } else { 1 + len };
            continue;
        }
        if used > 0 {
            rows += 1;
        }
        // A word longer than the line is broken across rows.
        if len > width {
            rows += (len - 1) / width;
            used = match len % width {
                0 => width,
                remainder => remainder,
            };
        } else {
            used = len;
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapped_rows_counts_what_a_popup_needs() {
        assert_eq!(wrapped_rows("", 40), 1, "a blank line still takes a row");
        assert_eq!(wrapped_rows("short", 40), 1);
        assert_eq!(wrapped_rows(&"x".repeat(80), 40), 2);
        assert_eq!(wrapped_rows(&"x".repeat(81), 40), 3);
        assert_eq!(
            wrapped_rows("anything", 0),
            1,
            "a zero width must not divide by zero"
        );
    }

    #[test]
    fn wrapped_rows_accounts_for_a_long_word_pushed_to_the_next_row() {
        // "Folder:" then a path too long to share the row: label, then two
        // rows of path. Character wrapping would wrongly say two rows.
        let line = format!("Folder: {}", "p".repeat(100));
        assert_eq!(wrapped_rows(&line, 75), 3);
        assert_eq!(wrapped_rows("one two three", 5), 3);
        assert_eq!(wrapped_rows("one two three", 20), 1);
    }

    #[test]
    fn the_startup_warning_states_what_saving_does() {
        // The wording required by design §3.
        let body = WARNING_BODY.join(" ");
        assert!(body.contains("modifies the original audio file in place"));
        assert!(body.contains("temporary output is created and verified"));
        assert!(body.contains("original file is replaced"));
        assert!(body.contains("Metadata is preserved where supported"));
    }

    #[test]
    fn help_covers_every_documented_command() {
        let help = help_lines().join("\n");
        for command in [":w", ":q", ":wq", ":help", ":apply-defaults", "--dry-run"] {
            assert!(help.contains(command), "help is missing {command}");
        }
    }
}
