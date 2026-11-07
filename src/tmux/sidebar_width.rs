//! Sidebar width resolution and enforcement.
//!
//! tmux rescales split panes proportionally when a window grows. A sidebar
//! created while a window was still 80 columns wide (a detached session, or
//! a window restored before the client attached) therefore balloons to a
//! large fraction of the screen once the real terminal size arrives. The
//! sidebar process re-applies its configured width whenever the window width
//! changes.

use super::{SIDEBAR_WIDTH, display_message, run_tmux};

pub const DEFAULT_WIDTH: &str = "32";
const MIN_COLUMNS: u32 = 10;

/// Resolve `@sidebar_width` (columns such as `32`, or a percentage such as
/// `15%`) into a column count for a window `window_width` columns wide.
pub fn resolve_columns(setting: &str, window_width: u32) -> u32 {
    let setting = setting.trim();
    let setting = if setting.is_empty() {
        DEFAULT_WIDTH
    } else {
        setting
    };
    if let Some(percent) = setting.strip_suffix('%') {
        let percent: u32 = percent.trim().parse().unwrap_or(15);
        if window_width > 0 && percent > 0 {
            return (window_width * percent / 100).max(MIN_COLUMNS);
        }
        return DEFAULT_WIDTH.parse().unwrap_or(32);
    }
    setting
        .parse::<u32>()
        .ok()
        .filter(|columns| *columns > 0)
        .unwrap_or_else(|| DEFAULT_WIDTH.parse().unwrap_or(32))
}

/// Re-applies the configured width after the window width changes.
#[derive(Debug, Default)]
pub struct WidthGuard {
    last_window_width: Option<u32>,
}

impl WidthGuard {
    /// Call periodically. Resizes `pane` only when the window width differs
    /// from the previous call (or on the first call), so a width the user
    /// drags to by hand is respected until the window itself resizes.
    pub fn tick(&mut self, pane: &str) {
        let info = display_message(
            pane,
            &format!(
                "#{{window_width}} #{{pane_width}} #{{window_zoomed_flag}} #{{{SIDEBAR_WIDTH}}}"
            ),
        );
        let mut parts = info.splitn(4, ' ');
        let (Some(window), Some(current), Some(zoomed)) = (
            parts.next().and_then(|v| v.parse::<u32>().ok()),
            parts.next().and_then(|v| v.parse::<u32>().ok()),
            parts.next(),
        ) else {
            return;
        };
        let setting = parts.next().unwrap_or("");
        if self.last_window_width == Some(window) {
            return;
        }
        self.last_window_width = Some(window);
        if zoomed == "1" {
            return;
        }
        let wanted = resolve_columns(setting, window);
        if wanted + 20 > window || current.abs_diff(wanted) <= 1 {
            return;
        }
        let _ = run_tmux(&["resize-pane", "-t", pane, "-x", &wanted.to_string()]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_are_used_as_is() {
        assert_eq!(resolve_columns("40", 230), 40);
        assert_eq!(resolve_columns(" 28 ", 80), 28);
    }

    #[test]
    fn percentages_scale_with_the_window_and_keep_a_minimum() {
        assert_eq!(resolve_columns("15%", 200), 30);
        assert_eq!(resolve_columns("50%", 200), 100);
        assert_eq!(resolve_columns("1%", 200), MIN_COLUMNS);
    }

    #[test]
    fn unset_or_invalid_falls_back_to_the_default() {
        assert_eq!(resolve_columns("", 230), 32);
        assert_eq!(resolve_columns("wide", 230), 32);
        assert_eq!(resolve_columns("0", 230), 32);
        assert_eq!(resolve_columns("15%", 0), 32);
    }
}
