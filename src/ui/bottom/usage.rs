use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::state::AppState;
use crate::ui::text::display_width;
use crate::usage::{UsageSnapshot, format_countdown};

/// Fill characters, from empty to full. The last step keeps a sliver so a
/// non-zero usage never renders as completely empty.
const METER_EMPTY: char = '░';
const METER_FULL: char = '█';

pub(super) fn draw_usage_content(frame: &mut Frame, state: &mut AppState, inner: Rect) {
    let theme = &state.theme;
    let Some(key) = crate::state::UsageState::target(state)
        .map(|target| target.key)
        .or_else(|| state.usage.requested_key.clone())
    else {
        super::render_centered(frame, inner, "No agent selected", theme.text_muted);
        return;
    };

    // A missing snapshot only means the background fetch has not returned
    // yet: failures arrive as a snapshot carrying a message.
    let Some(snapshot) = state.usage.current(&key).cloned() else {
        super::render_centered(frame, inner, "Loading usage…", theme.text_muted);
        return;
    };

    let now = state.now;
    let lines = build_lines(&snapshot, now, inner.width as usize, theme);
    state.usage.scroll.total_lines = lines.len();
    state.usage.scroll.visible_height = inner.height as usize;
    state.usage.scroll.scroll(0);

    let paragraph = Paragraph::new(lines).scroll((state.usage.scroll.offset as u16, 0));
    frame.render_widget(paragraph, inner);
}

fn build_lines<'a>(
    snapshot: &UsageSnapshot,
    now: u64,
    width: usize,
    theme: &crate::ui::colors::ColorTheme,
) -> Vec<Line<'a>> {
    let mut lines: Vec<Line<'a>> = Vec::new();
    let mut heading = snapshot.title.clone();
    if let Some(plan) = &snapshot.plan {
        heading.push_str(&format!(" · {plan}"));
    }
    lines.push(Line::from(Span::styled(
        crate::ui::text::truncate_to_width(&heading, width),
        Style::default().fg(theme.session_header),
    )));
    lines.push(Line::from(""));

    // Use the same longest-fit meter for every window so the two bars line
    // up even when their countdowns have different lengths.
    let label_cells = snapshot
        .windows
        .iter()
        .map(|w| display_width(&w.label))
        .max()
        .unwrap_or(0);
    let reserved = snapshot
        .windows
        .iter()
        .map(|window| {
            let percent = format!("{:.0}%", window.percent.clamp(0.0, 100.0));
            let tail = window
                .resets_at
                .map(|at| format!("{}  {percent}", format_countdown(at, now)))
                .unwrap_or(percent);
            label_cells + 1 + display_width(&tail) + 1
        })
        .max()
        .unwrap_or(0);
    let meter_cells = width.saturating_sub(reserved);
    for window in &snapshot.windows {
        lines.push(window_line(
            &format!("{:<label_cells$}", window.label),
            window.percent,
            window.resets_at,
            now,
            width,
            meter_cells,
            theme,
        ));
        lines.push(Line::from(""));
    }

    for (label, value) in &snapshot.local {
        lines.push(kv_line(label, value, width, theme));
        lines.push(Line::from(""));
    }

    for extra in &snapshot.extras {
        for chunk in crate::ui::text::wrap_text_char(extra, width.max(1), 2) {
            lines.push(Line::from(Span::styled(
                chunk,
                Style::default().fg(theme.text_muted),
            )));
        }
    }

    // The note goes last: it explains why the numbers above are all there is.
    if let Some(message) = &snapshot.message {
        if !snapshot.windows.is_empty() || !snapshot.local.is_empty() {
            lines.push(Line::from(""));
        }
        for chunk in crate::ui::text::wrap_text_char(message, width.max(1), 3) {
            lines.push(Line::from(Span::styled(
                chunk,
                Style::default().fg(theme.text_inactive),
            )));
        }
    }

    lines
}

fn window_line<'a>(
    label: &str,
    percent: f32,
    resets_at: Option<u64>,
    now: u64,
    width: usize,
    cells: usize,
    theme: &crate::ui::colors::ColorTheme,
) -> Line<'a> {
    let clamped = percent.clamp(0.0, 100.0);
    let color = severity_color(clamped, theme);
    let percent_text = format!("{:.0}%", clamped);
    let reset_text = resets_at
        .map(|at| format_countdown(at, now))
        .unwrap_or_default();
    let label_width = display_width(label) + 1;

    // Shrink the meter before dropping either of the numbers: a shorter bar
    // still reads as "how full", whereas a missing percentage does not.
    let tail = if reset_text.is_empty() {
        percent_text.clone()
    } else {
        format!("{reset_text}  {percent_text}")
    };
    let tail_width = display_width(&tail);
    if cells < 3 {
        // Not enough room for label + meter + numbers: fall back to a compact
        // `5h 95%` row so the panel stays readable in a very narrow sidebar.
        let compact = format!("{label} {percent_text}");
        return Line::from(Span::styled(
            crate::ui::text::truncate_to_width(&compact, width),
            Style::default().fg(color),
        ));
    }

    let gap = width
        .saturating_sub(label_width + cells + tail_width)
        .max(1);
    let mut spans = vec![
        Span::styled(format!("{label} "), Style::default().fg(theme.text_muted)),
        Span::styled(meter(clamped, cells), Style::default().fg(color)),
        Span::raw(" ".repeat(gap)),
    ];
    if !reset_text.is_empty() {
        spans.push(Span::styled(
            reset_text,
            Style::default().fg(theme.text_inactive),
        ));
        spans.push(Span::raw("  "));
    }
    spans.push(Span::styled(percent_text, Style::default().fg(color)));
    Line::from(spans)
}

fn kv_line<'a>(
    label: &str,
    value: &str,
    width: usize,
    theme: &crate::ui::colors::ColorTheme,
) -> Line<'a> {
    let label_width = 8.min(width);
    let value_width = width.saturating_sub(label_width + 1).max(1);
    let value = crate::ui::text::truncate_to_width(value, value_width);
    Line::from(vec![
        Span::styled(
            format!("{label:<label_width$}"),
            Style::default().fg(theme.text_inactive),
        ),
        Span::styled(value, Style::default().fg(theme.text_active)),
    ])
}

/// `█`-style meter. A percentage below 100 still shows the lowest non-zero
/// step so "1%" is visible.
fn meter(percent: f32, cells: usize) -> String {
    if cells == 0 {
        return String::new();
    }
    // Floor rather than round: 95% must not read as a full bar.
    let filled = if percent <= 0.0 {
        0
    } else {
        ((percent / 100.0) * cells as f32).floor().max(1.0) as usize
    }
    .min(cells);
    let mut out = String::with_capacity(cells * 3);
    for index in 0..cells {
        out.push(if index < filled {
            METER_FULL
        } else {
            METER_EMPTY
        });
    }
    out
}

/// Green while comfortable, amber as the window fills, red when it is
/// almost exhausted. Matches the status colours used in the agent list.
fn severity_color(percent: f32, theme: &crate::ui::colors::ColorTheme) -> ratatui::style::Color {
    if percent >= 90.0 {
        theme.status_error
    } else if percent >= 70.0 {
        theme.status_waiting
    } else {
        theme.status_idle
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::colors::ColorTheme;

    fn snapshot() -> UsageSnapshot {
        UsageSnapshot {
            key: "%1|Claude|".into(),
            title: "Claude Code".into(),
            plan: Some("max".into()),
            windows: vec![
                crate::usage::UsageWindow {
                    label: "5h".into(),
                    percent: 91.0,
                    resets_at: Some(1_794_060_599),
                },
                crate::usage::UsageWindow {
                    label: "1w".into(),
                    percent: 51.0,
                    resets_at: None,
                },
            ],
            ..UsageSnapshot::default()
        }
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn meter_fills_proportionally() {
        assert_eq!(meter(0.0, 10), "░░░░░░░░░░");
        assert_eq!(meter(50.0, 10), "█████░░░░░");
        assert_eq!(meter(95.0, 10), "█████████░");
        assert_eq!(meter(99.9, 10), "█████████░");
        assert_eq!(meter(100.0, 10), "██████████");
    }

    #[test]
    fn meter_shows_a_sliver_for_small_usage() {
        assert_eq!(meter(1.0, 10), "█░░░░░░░░░");
    }

    #[test]
    fn meter_handles_zero_cells() {
        assert_eq!(meter(50.0, 0), "");
    }

    #[test]
    fn lines_start_with_provider_and_plan() {
        let theme = ColorTheme::default();
        let lines = build_lines(&snapshot(), 1_794_000_000, 28, &theme);
        assert_eq!(line_text(&lines[0]), "Claude Code · max");
        assert_eq!(line_text(&lines[1]), "");
    }

    #[test]
    fn window_row_has_label_meter_countdown_and_percent() {
        let theme = ColorTheme::default();
        let lines = build_lines(&snapshot(), 1_794_000_000, 28, &theme);
        let row = line_text(&lines[2]);
        assert!(row.starts_with("5h ██████████░░"), "row was {row:?}");
        assert!(row.ends_with(" 91%"), "row was {row:?}");
        assert!(row.contains("16h 49m"), "row was {row:?}");
        assert_eq!(display_width(&row), 28);
    }

    #[test]
    fn narrow_panel_falls_back_to_compact_window_rows() {
        let theme = ColorTheme::default();
        // 12 columns cannot fit label + meter + countdown + percent.
        let lines = build_lines(&snapshot(), 1_794_000_000, 12, &theme);
        let row = line_text(&lines[2]);
        assert_eq!(row, "5h 91%");
    }

    #[test]
    fn window_without_reset_omits_countdown() {
        let theme = ColorTheme::default();
        let lines = build_lines(&snapshot(), 1_794_000_000, 28, &theme);
        let row = line_text(&lines[4]);
        assert!(row.starts_with("1w ██████░░░░░░"), "row was {row:?}");
        assert!(row.ends_with(" 51%"), "row was {row:?}");
    }

    #[test]
    fn message_is_shown_when_there_are_no_windows() {
        let theme = ColorTheme::default();
        let lines = build_lines(
            &UsageSnapshot {
                key: "k".into(),
                title: "OpenCode".into(),
                message: Some("No usage limits reported by commandcode".into()),
                ..UsageSnapshot::default()
            },
            0,
            28,
            &theme,
        );
        let joined: Vec<String> = lines.iter().map(line_text).collect();
        assert!(joined.iter().any(|l| l.contains("No usage limits")));
    }

    #[test]
    fn local_lines_render_as_key_value_rows() {
        let theme = ColorTheme::default();
        let lines = build_lines(
            &UsageSnapshot {
                key: "k".into(),
                title: "OpenCode".into(),
                local: vec![
                    ("Using".into(), "commandcode/deepseek".into()),
                    ("Session".into(), "$15.22 · 1.2M tok".into()),
                ],
                ..UsageSnapshot::default()
            },
            0,
            28,
            &theme,
        );
        let joined: Vec<String> = lines.iter().map(line_text).collect();
        assert_eq!(joined[0], "OpenCode");
        assert_eq!(joined[1], "");
        assert!(joined[2].starts_with("Using   commandcode"), "{joined:?}");
        assert!(joined[4].starts_with("Session $15.22"), "{joined:?}");
    }

    #[test]
    fn no_row_overflows_a_narrow_panel() {
        let theme = ColorTheme::default();
        for width in [12usize, 16, 20, 24, 28, 40] {
            let lines = build_lines(&snapshot(), 1_794_000_000, width, &theme);
            for line in &lines {
                let text = line_text(line);
                assert!(
                    display_width(&text) <= width,
                    "width {width}: row {text:?} is {} wide",
                    display_width(&text)
                );
            }
        }
    }

    #[test]
    fn severity_picks_error_amber_green() {
        let theme = ColorTheme::default();
        assert_eq!(severity_color(95.0, &theme), theme.status_error);
        assert_eq!(severity_color(75.0, &theme), theme.status_waiting);
        assert_eq!(severity_color(10.0, &theme), theme.status_idle);
    }

    #[test]
    fn token_formatting_is_shared_with_the_usage_module() {
        assert_eq!(crate::usage::format_tokens(2_000), "2k tok");
    }
}
