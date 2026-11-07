//! When the agent shown in the single Agents window exits (`/exit`, Ctrl+C,
//! or it is closed from the sidebar), switch to another agent instead of
//! leaving the user at a bare shell.

use std::collections::HashSet;

use super::AppState;
use crate::tmux;

/// Ticks (about a second each) the visible pane must stay agent-less before
/// the sidebar reacts. A `/clear` ends and restarts a session within moments
/// and must not be mistaken for an exit.
const MISSING_TICKS: u8 = 1;

/// Fast-path ticks (about 100 ms each) the visible pane must show a bare
/// shell before the sidebar switches away from it.
const SHELL_TICKS: u8 = 2;

#[derive(Debug, Default)]
pub(crate) struct ExitFallback {
    /// Agent pane that was last shown in the content slot.
    visible: Option<String>,
    /// Pane ids per folder, top to bottom, as of the last tick the visible
    /// agent was still listed.
    groups: Vec<Vec<String>>,
    missing_ticks: u8,
    /// Session and Agents host window, remembered for the fast path.
    owner: String,
    host: String,
    shell_ticks: u8,
}

/// Choose the agent to show after `exited` is gone.
///
/// Prefers another agent in the same folder (the one above it, else the one
/// below). If the folder has none left, takes the top agent of the nearest
/// folder above, then the nearest folder below.
pub(crate) fn fallback_pane(
    groups: &[Vec<String>],
    exited: &str,
    alive: &HashSet<String>,
) -> Option<String> {
    let folder = groups.iter().position(|g| g.iter().any(|p| p == exited))?;
    let index = groups[folder].iter().position(|p| p == exited)?;
    let alive_in = |g: &Vec<String>| {
        g.iter()
            .filter(|p| alive.contains(*p))
            .cloned()
            .collect::<Vec<_>>()
    };

    let above = groups[folder][..index]
        .iter()
        .rev()
        .find(|p| alive.contains(*p));
    let below = groups[folder][index + 1..]
        .iter()
        .find(|p| alive.contains(*p));
    if let Some(same_folder) = above.or(below) {
        return Some(same_folder.clone());
    }
    for group in groups[..folder].iter().rev() {
        if let Some(top) = alive_in(group).into_iter().next() {
            return Some(top);
        }
    }
    for group in &groups[folder + 1..] {
        if let Some(top) = alive_in(group).into_iter().next() {
            return Some(top);
        }
    }
    None
}

impl AppState {
    /// Call once per refresh tick.
    pub fn follow_exited_agent(&mut self) {
        if !self.persistent_agent_highlight {
            return;
        }
        let own = tmux::display_message(&self.tmux_pane, "#{session_name}|#{window_id}");
        let Some((owner, window)) = own.split_once('|') else {
            return;
        };
        let host = tmux::display_message(owner, &format!("#{{{}}}", tmux::workspace::HOST));
        if host.is_empty() || window != host {
            return;
        }
        let owner = owner.to_string();
        let Ok(content) = tmux::workspace::content_pane(&host) else {
            return;
        };

        let groups: Vec<Vec<String>> = self
            .repo_groups
            .iter()
            .map(|g| g.panes.iter().map(|(p, _)| p.pane_id.clone()).collect())
            .collect();
        let alive: HashSet<String> = groups.iter().flatten().cloned().collect();

        if alive.contains(&content) {
            self.exit_fallback = ExitFallback {
                visible: Some(content),
                groups,
                missing_ticks: 0,
                owner: owner.clone(),
                host: host.clone(),
                shell_ticks: 0,
            };
            return;
        }
        let Some(exited) = self.exit_fallback.visible.clone() else {
            return;
        };
        self.exit_fallback.missing_ticks += 1;
        if self.exit_fallback.missing_ticks < MISSING_TICKS {
            return;
        }
        let placeholder = tmux::display_message(&content, "#{@pane_role}") == "agent-placeholder";
        let command = tmux::display_message(&content, "#{pane_current_command}");
        if !placeholder && !tmux::is_shell_command(&command) {
            // Still running something (for example a restarting session).
            return;
        }
        let target = fallback_pane(&self.exit_fallback.groups, &exited, &alive);
        self.exit_fallback = ExitFallback::default();
        if let Some(target) = target {
            switch_away(&host, &owner, &content, &target, placeholder);
        }
    }

    /// Fast path, called about ten times a second: notice that the visible
    /// agent's pane has dropped to a shell without waiting for the next
    /// refresh, and switch right away.
    pub fn follow_exited_agent_fast(&mut self) {
        if !self.persistent_agent_highlight {
            return;
        }
        let fallback = &mut self.exit_fallback;
        let (Some(visible), false) = (fallback.visible.clone(), fallback.host.is_empty()) else {
            return;
        };
        let info = tmux::display_message(&visible, "#{pane_current_command}|#{window_id}");
        let (command, window) = info.split_once('|').unwrap_or(("", ""));
        // The pane closed outright, or it is no longer the one on screen.
        let gone = info.is_empty();
        if !gone && window != fallback.host {
            fallback.shell_ticks = 0;
            return;
        }
        if !gone && !tmux::is_shell_command(command) {
            fallback.shell_ticks = 0;
            return;
        }
        fallback.shell_ticks += 1;
        if fallback.shell_ticks < SHELL_TICKS {
            return;
        }
        let alive: HashSet<String> = fallback
            .groups
            .iter()
            .flatten()
            .filter(|pane| **pane != visible)
            .cloned()
            .collect();
        let target = fallback_pane(&fallback.groups, &visible, &alive);
        let (host, owner) = (fallback.host.clone(), fallback.owner.clone());
        self.exit_fallback = ExitFallback::default();
        let Some(target) = target else {
            return;
        };
        let content = tmux::workspace::content_pane(&host).unwrap_or(visible);
        let placeholder = tmux::display_message(&content, "#{@pane_role}") == "agent-placeholder";
        switch_away(&host, &owner, &content, &target, placeholder);
    }
}

/// Show `target` in the Agents window and close the shell that `content`
/// was left as, when the sidebar started it.
fn switch_away(host: &str, owner: &str, content: &str, target: &str, placeholder: bool) {
    let spawned = tmux::display_message(content, "#{@agent-sidebar-spawned}") == "1";
    let shown = tmux::workspace::show(host, target, owner);
    if shown.is_ok() && spawned && !placeholder {
        // The leftover shell of an agent this sidebar started has no use.
        let _ = tmux::run_tmux(&["kill-pane", "-t", content]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn groups(spec: &[&[&str]]) -> Vec<Vec<String>> {
        spec.iter()
            .map(|g| g.iter().map(|p| p.to_string()).collect())
            .collect()
    }

    fn alive(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|p| p.to_string()).collect()
    }

    #[test]
    fn prefers_the_agent_above_in_the_same_folder() {
        let g = groups(&[&["%1"], &["%2", "%3", "%4"], &["%5"]]);
        let live = alive(&["%1", "%2", "%4", "%5"]);
        assert_eq!(fallback_pane(&g, "%3", &live).as_deref(), Some("%2"));
    }

    #[test]
    fn falls_back_to_the_agent_below_when_the_exited_one_was_first() {
        let g = groups(&[&["%1"], &["%2", "%3"], &["%5"]]);
        let live = alive(&["%1", "%3", "%5"]);
        assert_eq!(fallback_pane(&g, "%2", &live).as_deref(), Some("%3"));
    }

    #[test]
    fn an_empty_folder_falls_back_to_the_top_agent_of_the_folder_above() {
        let g = groups(&[&["%1", "%2"], &["%3"], &["%4"]]);
        let live = alive(&["%1", "%2", "%4"]);
        assert_eq!(fallback_pane(&g, "%3", &live).as_deref(), Some("%1"));
    }

    #[test]
    fn the_first_folder_falls_back_to_the_folder_below() {
        let g = groups(&[&["%1"], &["%2", "%3"]]);
        let live = alive(&["%2", "%3"]);
        assert_eq!(fallback_pane(&g, "%1", &live).as_deref(), Some("%2"));
    }

    #[test]
    fn skips_folders_whose_agents_are_all_gone_and_handles_no_agents() {
        let g = groups(&[&["%1"], &["%2"], &["%3"]]);
        assert_eq!(
            fallback_pane(&g, "%3", &alive(&["%1"])).as_deref(),
            Some("%1")
        );
        assert_eq!(fallback_pane(&g, "%2", &alive(&[])), None);
        assert_eq!(fallback_pane(&g, "%9", &alive(&["%1"])), None);
    }
}
