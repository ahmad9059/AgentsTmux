use std::collections::HashMap;

use crate::tmux::{self, PaneStatus};

/// Braille spinner shown while an agent works.
pub const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
/// A single dot circling a braille cell, shown for background work.
pub const ORBIT_FRAMES: &[&str] = &["⠁", "⠂", "⠄", "⡀", "⢀", "⠠", "⠐", "⠈"];
/// Number of animation ticks after which every animation lines up again.
pub const ANIMATION_PERIOD: usize = 40;
/// Ticks per phase of the attention blink.
const BLINK_TICKS: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusIcons {
    /// Icon for the "All" filter in the top filter bar.
    all: String,
    /// `None` keeps the built-in animation; a configured icon is static.
    running: Option<String>,
    background: Option<String>,
    waiting: Option<String>,
    idle: String,
    error: String,
    unknown: String,
}

impl Default for StatusIcons {
    fn default() -> Self {
        Self {
            all: "≡".into(),
            running: None,
            background: None,
            waiting: None,
            idle: "○".into(),
            error: "◉".into(),
            unknown: "·".into(),
        }
    }
}

impl StatusIcons {
    /// Load status icons from tmux @sidebar_icon_* variables, falling back to defaults.
    pub fn from_tmux() -> Self {
        let all_opts = tmux::get_all_global_options();
        Self::from_options(&all_opts)
    }

    pub fn from_options(all_opts: &HashMap<String, String>) -> Self {
        let mut icons = Self::default();

        let read = |var: &str| -> Option<String> {
            all_opts
                .get(var)
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned)
        };

        icons.all = read(tmux::SIDEBAR_ICON_ALL).unwrap_or(icons.all);
        icons.running = read(tmux::SIDEBAR_ICON_RUNNING);
        icons.background = read(tmux::SIDEBAR_ICON_BACKGROUND);
        icons.waiting = read(tmux::SIDEBAR_ICON_WAITING);
        icons.idle = read(tmux::SIDEBAR_ICON_IDLE).unwrap_or(icons.idle);
        icons.error = read(tmux::SIDEBAR_ICON_ERROR).unwrap_or(icons.error);
        icons.unknown = read(tmux::SIDEBAR_ICON_UNKNOWN).unwrap_or(icons.unknown);
        icons
    }

    /// Icon used for the "All" filter (not tied to any PaneStatus).
    pub fn all_icon(&self) -> &str {
        self.all.as_str()
    }

    /// The glyph for `status` at animation tick `frame`: a spinner while
    /// working, an orbiting dot for background work, a blinking dot when the
    /// agent needs attention, and steady rings or dots otherwise.
    pub fn status_icon(&self, status: &PaneStatus, frame: usize) -> &str {
        let frame = frame % ANIMATION_PERIOD;
        match status {
            PaneStatus::Running => self
                .running
                .as_deref()
                .unwrap_or(SPINNER_FRAMES[frame % SPINNER_FRAMES.len()]),
            PaneStatus::Background => self
                .background
                .as_deref()
                .unwrap_or(ORBIT_FRAMES[frame % ORBIT_FRAMES.len()]),
            PaneStatus::Waiting => {
                self.waiting
                    .as_deref()
                    .unwrap_or(if (frame / BLINK_TICKS).is_multiple_of(2) {
                        "●"
                    } else {
                        "◌"
                    })
            }
            PaneStatus::Idle => self.idle.as_str(),
            PaneStatus::Error => self.error.as_str(),
            PaneStatus::Unknown => self.unknown.as_str(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_icons_animate_working_states_and_use_dots_elsewhere() {
        let icons = StatusIcons::default();
        assert_eq!(icons.all_icon(), "≡");
        assert_eq!(icons.status_icon(&PaneStatus::Running, 0), "⠋");
        assert_eq!(icons.status_icon(&PaneStatus::Running, 3), "⠸");
        assert_eq!(icons.status_icon(&PaneStatus::Background, 0), "⠁");
        assert_eq!(icons.status_icon(&PaneStatus::Background, 3), "⡀");
        assert_eq!(icons.status_icon(&PaneStatus::Waiting, 0), "●");
        assert_eq!(icons.status_icon(&PaneStatus::Waiting, 4), "◌");
        assert_eq!(icons.status_icon(&PaneStatus::Waiting, 8), "●");
        assert_eq!(icons.status_icon(&PaneStatus::Idle, 7), "○");
        assert_eq!(icons.status_icon(&PaneStatus::Error, 7), "◉");
        assert_eq!(icons.status_icon(&PaneStatus::Unknown, 7), "·");
    }

    #[test]
    fn the_spinner_cycles_through_every_frame() {
        let icons = StatusIcons::default();
        let seen: Vec<&str> = (0..SPINNER_FRAMES.len())
            .map(|frame| icons.status_icon(&PaneStatus::Running, frame))
            .collect();
        assert_eq!(seen, SPINNER_FRAMES);
        assert_eq!(
            icons.status_icon(&PaneStatus::Running, ANIMATION_PERIOD),
            icons.status_icon(&PaneStatus::Running, 0)
        );
    }

    #[test]
    fn tmux_options_override_defaults() {
        let mut opts = HashMap::new();
        opts.insert(tmux::SIDEBAR_ICON_ALL.into(), "∀".into());
        opts.insert(tmux::SIDEBAR_ICON_RUNNING.into(), "◉".into());
        opts.insert(tmux::SIDEBAR_ICON_UNKNOWN.into(), "∎".into());

        let icons = StatusIcons::from_options(&opts);
        assert_eq!(icons.all_icon(), "∀");
        assert_eq!(icons.status_icon(&PaneStatus::Running, 5), "◉");
        assert_eq!(icons.status_icon(&PaneStatus::Unknown, 0), "∎");
        assert_eq!(icons.status_icon(&PaneStatus::Background, 0), "⠁");
    }
}
