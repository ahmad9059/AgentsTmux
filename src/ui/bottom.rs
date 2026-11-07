#[cfg(test)]
mod git;
mod usage;

use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::state::{AppState, Focus};

fn render_centered(frame: &mut Frame, area: Rect, text: &str, color: Color) {
    // Vertically center: pad with empty lines above
    let top_pad = area.height.saturating_sub(1) / 2;
    let mut lines: Vec<Line<'_>> = Vec::new();
    for _ in 0..top_pad {
        lines.push(Line::from(""));
    }
    lines.push(Line::from(Span::styled(text, Style::default().fg(color))));
    let paragraph = Paragraph::new(lines).alignment(Alignment::Center);
    frame.render_widget(paragraph, area);
}

pub fn draw_bottom(frame: &mut Frame, state: &mut AppState, area: Rect) {
    // Keep the separator and all content off the sidebar's terminal edges.
    let inset = u16::from(area.width > 2);
    let area = Rect::new(
        area.x + inset,
        area.y,
        area.width.saturating_sub(inset * 2),
        area.height,
    );
    let theme = &state.theme;
    let border_color = if state.focus_state.focus == Focus::BottomPanel {
        theme.accent
    } else {
        theme.border_inactive
    };

    if area.is_empty() {
        return;
    }
    let remaining = area.width.saturating_sub(7) as usize;
    let left = remaining / 2;
    let right = remaining - left;
    let title = Line::from(vec![
        Span::styled(
            format!("{} ", "─".repeat(left)),
            Style::default().fg(border_color),
        ),
        Span::styled("Usage", Style::default().fg(theme.accent)),
        Span::styled(
            format!(" {}", "─".repeat(right)),
            Style::default().fg(border_color),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(title),
        Rect::new(area.x, area.y, area.width, 1),
    );
    let inner = Rect::new(
        area.x,
        area.y + 1,
        area.width,
        area.height.saturating_sub(1),
    );
    usage::draw_usage_content(frame, state, inner);
}

#[cfg(test)]
mod tests {
    use crate::ui::text::truncate_to_width;

    #[test]
    fn truncate_to_width_short() {
        assert_eq!(truncate_to_width("hello", 10), "hello");
    }

    #[test]
    fn truncate_to_width_exact() {
        assert_eq!(truncate_to_width("hello", 5), "hello");
    }

    #[test]
    fn truncate_to_width_truncated() {
        let result = truncate_to_width("hello world", 8);
        assert!(result.ends_with('…'));
        assert!(result.len() <= 10); // 7 chars + ellipsis in bytes
    }
}
