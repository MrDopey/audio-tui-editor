//! Rendering. The current mode is always visible (design §4) and the waveform
//! is redrawn from cached analysis rather than recomputed (design §7).

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, List, ListItem, Padding, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, MarkerKind, Mode, Overlay, Session};
use crate::media::first_line;
use crate::media::probe::MediaInfo;
use crate::timespec::format_timestamp;

/// Eighth-block glyphs used to draw amplitude, plus a blank for silence.
const BLOCKS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

const ACCENT: Color = Color::Cyan;
const RETAINED: Color = Color::Green;
const REMOVED: Color = Color::DarkGray;

pub fn render(frame: &mut Frame, app: &mut App) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(2),
    ])
    .areas(frame.area());

    render_header(frame, app, header);
    match app.mode {
        Mode::Browse => render_browse(frame, app, body),
        Mode::Play => render_play(frame, app, body),
        Mode::Edit => render_edit(frame, app, body),
        Mode::Metadata => render_metadata(frame, app, body),
    }
    render_footer(frame, app, footer);
    render_overlay(frame, app);
}

fn render_header(frame: &mut Frame, app: &App, area: Rect) {
    let mut spans = vec![
        Span::styled(
            format!(" {} ", app.mode.label()),
            Style::default()
                .fg(Color::Black)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
    ];

    match &app.session {
        Some(session) => {
            spans.push(Span::styled(
                session.info.file_name(),
                Style::default().add_modifier(Modifier::BOLD),
            ));
            if session.is_dirty() {
                spans.push(Span::styled(" [+]", Style::default().fg(Color::Yellow)));
            }
        }
        None => spans.push(Span::styled(
            app.folder.display().to_string(),
            Style::default().fg(Color::Gray),
        )),
    }

    let right = format!("{} files ", app.files.len());
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let gap = (area.width as usize).saturating_sub(used + right.chars().count());
    spans.push(Span::raw(" ".repeat(gap)));
    spans.push(Span::styled(right, Style::default().fg(Color::DarkGray)));

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

// ---- BROWSE (design §5) --------------------------------------------------

fn render_browse(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(" Files ")
        .title_style(Style::default().fg(ACCENT));

    if app.files.is_empty() {
        let text = vec![
            Line::raw(""),
            Line::raw("  No supported audio files were found in this folder."),
            Line::raw(""),
            Line::raw("  Start audioedit elsewhere with --folder, or press r to rescan."),
        ];
        frame.render_widget(Paragraph::new(text).block(block), area);
        return;
    }

    let inner = block.inner(area);
    app.page_rows = inner.height as usize;

    let name_width = (inner.width as usize).saturating_sub(20).max(10);
    let items: Vec<ListItem> = app
        .file_rows()
        .iter()
        .map(|(name, duration, format)| {
            ListItem::new(Line::from(vec![
                Span::raw(format!(
                    "{:<width$} ",
                    truncate(name, name_width),
                    width = name_width
                )),
                Span::styled(format!("{duration:>8} "), Style::default().fg(Color::Gray)),
                Span::styled(format!("{format:<6}"), Style::default().fg(Color::DarkGray)),
            ]))
        })
        .collect();

    app.list_state.select(Some(app.selected));
    let list = List::new(items)
        .block(block)
        .highlight_symbol("> ")
        .highlight_style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD));
    frame.render_stateful_widget(list, area, &mut app.list_state);
}

// ---- PLAY (design §6, §7) -------------------------------------------------

fn render_play(frame: &mut Frame, app: &mut App, area: Rect) {
    let Some(session) = &app.session else {
        app.mode = Mode::Browse;
        return;
    };

    // Three detail lines plus the box borders.
    let [details, waveform, transport] = Layout::vertical([
        Constraint::Length(5),
        Constraint::Min(5),
        Constraint::Length(3),
    ])
    .areas(area);

    render_details(frame, &session.info, details);
    render_waveform(frame, session, waveform, None);
    render_transport(frame, session, app.audio_device_available(), transport);
}

fn render_details(frame: &mut Frame, info: &MediaInfo, area: Rect) {
    // The container is worth showing next to the codec: `opus` in `ogg`.
    let container = info.format_name.split(',').next().unwrap_or("").to_string();
    let mut facts = vec![info.audio_codec.clone()];
    if !container.is_empty() && container != info.audio_codec {
        facts.push(container);
    }
    if let Some(rate) = info.sample_rate {
        facts.push(format!("{rate} Hz"));
    }
    if let Some(channels) = info.channels {
        facts.push(format!("{channels} ch"));
    }
    if let Some(bitrate) = info.bit_rate {
        facts.push(format!("{} kb/s", bitrate / 1000));
    }
    if info.has_cover_art {
        facts.push("cover art".to_string());
    }
    if info.chapter_count > 0 {
        facts.push(format!("{} chapters", info.chapter_count));
    }

    let title = info.tag("title").unwrap_or("—").to_string();
    let artist = info.tag("artist").unwrap_or("—").to_string();

    let text = vec![
        Line::from(vec![
            Span::styled("Format  ", Style::default().fg(Color::DarkGray)),
            Span::raw(facts.join(" · ")),
        ]),
        Line::from(vec![
            Span::styled("Title   ", Style::default().fg(Color::DarkGray)),
            Span::raw(title),
        ]),
        Line::from(vec![
            Span::styled("Artist  ", Style::default().fg(Color::DarkGray)),
            Span::raw(artist),
        ]),
    ];
    frame.render_widget(
        Paragraph::new(text).block(Block::bordered().border_type(BorderType::Rounded)),
        area,
    );
}

/// Draw the waveform, the playback cursor and (in EDIT) the marker range.
fn render_waveform(frame: &mut Frame, session: &Session, area: Rect, markers: Option<()>) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(" Waveform ")
        .title_style(Style::default().fg(ACCENT));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width < 4 || inner.height < 3 {
        return;
    }

    let width = inner.width as usize;
    let duration = session.duration().max(0.001);

    // Row 0: the time scale. Last row: the playback cursor. Rest: amplitude.
    let scale = Line::from(vec![
        Span::styled("00:00", Style::default().fg(Color::DarkGray)),
        Span::raw(" ".repeat(width.saturating_sub(10 + 3))),
        Span::styled(
            format!("{:>8}", format_timestamp(duration)),
            Style::default().fg(Color::DarkGray),
        ),
    ]);

    let bar_rows = (inner.height as usize).saturating_sub(2).max(1);
    let columns = match session.waveform.ready() {
        Some(waveform) => waveform.downsample(width),
        None => vec![(0.0, 0.0); width],
    };

    let position_column = column_for(session.player.position(), duration, width);
    let (begin_column, end_column) = (
        column_for(session.begin.seconds(), duration, width),
        column_for(session.end.seconds(), duration, width),
    );
    let show_markers = markers.is_some();

    let mut lines = vec![scale];
    for row in (0..bar_rows).rev() {
        let spans = columns
            .iter()
            .enumerate()
            .map(|(col, (peak, rms))| {
                // Peak sets the outline, RMS fills the body.
                let level = (*peak).max(*rms * 1.2).clamp(0.0, 1.0);
                let eighths = (level as f64 * (bar_rows * 8) as f64).round() as isize;
                let in_row = (eighths - (row * 8) as isize).clamp(0, 8) as usize;
                let mut style = Style::default().fg(ACCENT);
                if show_markers {
                    style = if col >= begin_column && col <= end_column {
                        Style::default().fg(RETAINED)
                    } else {
                        Style::default().fg(REMOVED)
                    };
                }
                if col == position_column {
                    style = style.bg(Color::DarkGray);
                }
                Span::styled(BLOCKS[in_row].to_string(), style)
            })
            .collect::<Vec<_>>();
        lines.push(Line::from(spans));
    }

    // The cursor line, plus marker letters when editing.
    let mut cursor: Vec<char> = vec![' '; width];
    if position_column < width {
        cursor[position_column] = '│';
    }
    let mut cursor_spans: Vec<Span> = Vec::with_capacity(width);
    for (col, ch) in cursor.iter().enumerate() {
        let (glyph, style) = if show_markers && col == begin_column {
            (
                'b',
                Style::default().fg(RETAINED).add_modifier(Modifier::BOLD),
            )
        } else if show_markers && col == end_column {
            (
                'e',
                Style::default().fg(RETAINED).add_modifier(Modifier::BOLD),
            )
        } else {
            (*ch, Style::default().fg(Color::White))
        };
        cursor_spans.push(Span::styled(glyph.to_string(), style));
    }
    lines.push(Line::from(cursor_spans));

    if let Some(error) = session.waveform.error() {
        lines.push(Line::styled(
            format!("waveform unavailable: {error}"),
            Style::default().fg(Color::Red),
        ));
    } else if session.waveform.is_running() {
        lines.push(Line::styled(
            "analysing waveform…",
            Style::default().fg(Color::DarkGray),
        ));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

fn column_for(seconds: f64, duration: f64, width: usize) -> usize {
    if width == 0 || duration <= 0.0 {
        return 0;
    }
    let fraction = (seconds / duration).clamp(0.0, 1.0);
    ((fraction * width as f64) as usize).min(width - 1)
}

fn render_transport(frame: &mut Frame, session: &Session, device: bool, area: Rect) {
    let position = session.player.position();
    let duration = session.duration();
    let symbol = if session.player.is_playing() {
        "▶"
    } else {
        "‖"
    };

    let mut spans = vec![
        Span::styled(format!(" {symbol} "), Style::default().fg(ACCENT)),
        Span::styled(
            format!(
                "{} / {}",
                format_timestamp(position),
                format_timestamp(duration)
            ),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw("    "),
        Span::styled(
            format!("vol {:.0}%", session.player.volume()),
            Style::default().fg(Color::Gray),
        ),
    ];
    if !device {
        spans.push(Span::styled(
            "    (no audio device — silent transport)",
            Style::default().fg(Color::Yellow),
        ));
    }
    if let Some(error) = session.player.error() {
        spans.push(Span::styled(
            format!("    playback failed: {error}"),
            Style::default().fg(Color::Red),
        ));
    }

    frame.render_widget(
        Paragraph::new(Line::from(spans)).block(Block::bordered().border_type(BorderType::Rounded)),
        area,
    );
}

// ---- EDIT (design §8–§11) -------------------------------------------------

fn render_edit(frame: &mut Frame, app: &mut App, area: Rect) {
    let Some(session) = &app.session else {
        app.mode = Mode::Browse;
        return;
    };

    let [markers, waveform, transport] = Layout::vertical([
        Constraint::Length(6),
        Constraint::Min(5),
        Constraint::Length(3),
    ])
    .areas(area);

    render_markers(frame, session, markers);
    render_waveform(frame, session, waveform, Some(()));
    render_transport(frame, session, app.audio_device_available(), transport);
}

fn render_markers(frame: &mut Frame, session: &Session, area: Rect) {
    let kept = (session.end.seconds() - session.begin.seconds()).max(0.0);
    let removed_begin = session.begin.seconds();
    let removed_end = (session.duration() - session.end.seconds()).max(0.0);

    let marker_line = |kind: MarkerKind, marker: &crate::timespec::Marker| {
        let active = session.active == kind;
        let arrow = if active { "▸ " } else { "  " };
        let style = if active {
            Style::default().fg(RETAINED).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        Line::from(vec![
            Span::styled(arrow, Style::default().fg(RETAINED)),
            Span::styled(
                format!("{:<10}", format!("{}:", kind.label())),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(marker.to_string(), style),
        ])
    };

    let status = if session.auto.is_running() {
        Span::styled(
            "detecting automatic markers…",
            Style::default().fg(Color::DarkGray),
        )
    } else if let Some(error) = session.auto.error() {
        Span::styled(
            format!("auto-detect failed: {error}"),
            Style::default().fg(Color::Red),
        )
    } else if session.markers_dirty {
        Span::styled("markers edited", Style::default().fg(Color::Yellow))
    } else {
        Span::styled("automatic markers", Style::default().fg(Color::DarkGray))
    };

    let text = vec![
        marker_line(MarkerKind::Begin, &session.begin),
        marker_line(MarkerKind::End, &session.end),
        Line::from(vec![
            Span::styled("  retained:  ", Style::default().fg(Color::DarkGray)),
            Span::raw(format_timestamp(kept)),
            Span::styled("   removed:  ", Style::default().fg(Color::DarkGray)),
            Span::raw(format!("{removed_begin:.3}s + {removed_end:.3}s")),
        ]),
        Line::from(vec![Span::raw("  "), status]),
    ];

    frame.render_widget(
        Paragraph::new(text).block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .title(" Markers ")
                .title_style(Style::default().fg(ACCENT)),
        ),
        area,
    );
}

// ---- METADATA (design §18) -------------------------------------------------

fn render_metadata(frame: &mut Frame, app: &mut App, area: Rect) {
    let Some(session) = &app.session else {
        app.mode = Mode::Browse;
        return;
    };

    let lines: Vec<Line> = session
        .fields
        .iter()
        .enumerate()
        .map(|(index, field)| {
            let selected = index == session.field_index;
            let marker = if selected { "▸ " } else { "  " };
            let value_style = if field.is_changed() {
                Style::default().fg(Color::Yellow)
            } else if field.value.is_none() {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default()
            };
            let value = if field.display().is_empty() {
                "—"
            } else {
                field.display()
            };
            Line::from(vec![
                Span::styled(marker, Style::default().fg(ACCENT)),
                Span::styled(
                    format!("{:<14}", format!("{}:", field.label)),
                    if selected {
                        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::Gray)
                    },
                ),
                Span::styled(value.to_string(), value_style),
                if field.is_changed() {
                    Span::styled("  (edited)", Style::default().fg(Color::Yellow))
                } else {
                    Span::raw("")
                },
            ])
        })
        .collect();

    frame.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .title(" Metadata ")
                .title_style(Style::default().fg(ACCENT)),
        ),
        area,
    );
}

// ---- footer ---------------------------------------------------------------

fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    let [hints, message] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);

    frame.render_widget(
        Paragraph::new(Line::styled(
            hint_text(app),
            Style::default().fg(Color::DarkGray),
        )),
        hints,
    );

    let line = if let Some(prompt) = &app.prompt {
        Line::from(vec![
            Span::styled(
                format!("{}{}", prompt.sigil(), prompt.label()),
                Style::default().fg(ACCENT),
            ),
            Span::raw(prompt.buffer.clone()),
            Span::styled("█", Style::default().fg(ACCENT)),
        ])
    } else if let Some(status) = &app.status {
        let colour = if status.is_error {
            Color::Red
        } else {
            Color::Green
        };
        Line::styled(status.text.clone(), Style::default().fg(colour))
    } else {
        Line::raw("")
    };
    frame.render_widget(Paragraph::new(line), message);
}

fn hint_text(app: &App) -> &'static str {
    match app.mode {
        Mode::Browse => {
            "j/k move  gg/G ends  C-d/C-u page  / search  n/N repeat  Enter open  ? help  q quit"
        }
        Mode::Play => {
            "space play  h/l seek  C-h/C-l ±60s  g/G start/end  j/k volume  e edit  m metadata  :w save  Esc back"
        }
        Mode::Edit => {
            "h/l move marker  C-h/C-l large  Tab switch  b/e set here  B/E/i type +10s -1m 50%  a auto  r reset both  :w save  Esc back"
        }
        Mode::Metadata => "j/k field  Enter edit  u revert  :w save  :wq save & leave  Esc back",
    }
}

// ---- overlays --------------------------------------------------------------

fn render_overlay(frame: &mut Frame, app: &mut App) {
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
        "  e               EDIT mode          m   METADATA mode",
        "  Esc, q          back to BROWSE",
        "",
        "EDIT",
        "  ←/→, h/l        move the active marker (fine step)",
        "  Ctrl-←/→, C-h/l move the active marker (large step)",
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

/// Shorten a name to fit a column, keeping the extension visible.
fn truncate(text: &str, width: usize) -> String {
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
    use crate::media::waveform::Waveform;

    #[test]
    fn columns_map_positions_across_the_full_width() {
        assert_eq!(column_for(0.0, 100.0, 80), 0);
        assert_eq!(column_for(100.0, 100.0, 80), 79);
        assert_eq!(column_for(50.0, 100.0, 80), 40);
    }

    #[test]
    fn column_mapping_survives_degenerate_input() {
        assert_eq!(column_for(5.0, 0.0, 80), 0);
        assert_eq!(column_for(5.0, 10.0, 0), 0);
        assert_eq!(column_for(-5.0, 10.0, 8), 0);
        assert_eq!(column_for(1000.0, 10.0, 8), 7);
    }

    #[test]
    fn names_are_truncated_with_an_ellipsis() {
        assert_eq!(truncate("short.opus", 20), "short.opus");
        assert_eq!(truncate("a-very-long-name.opus", 10), "a-very-lo…");
    }

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

    #[test]
    fn waveform_downsamples_to_the_drawn_width() {
        let waveform = Waveform {
            duration: 10.0,
            peaks: (0..500).map(|i| (i % 100) as f32 / 100.0).collect(),
            rms: (0..500).map(|i| (i % 100) as f32 / 200.0).collect(),
        };
        assert_eq!(waveform.downsample(64).len(), 64);
    }
}
