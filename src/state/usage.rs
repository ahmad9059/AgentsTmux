use std::time::{Duration, Instant};

use super::AppState;
use crate::usage::{UsageSnapshot, UsageTarget};

/// How often the visible Usage tab asks for a fresh snapshot. Provider data
/// itself is cached inside [`crate::usage::UsageFetcher`], so this only
/// controls how quickly local (OpenCode/Pi) numbers and the "updated" footer
/// move.
pub const USAGE_REFRESH_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Default)]
pub struct UsageState {
    /// Last snapshot received from the background worker.
    pub snapshot: Option<UsageSnapshot>,
    /// Target key the most recent request was made for.
    pub requested_key: Option<String>,
    /// When the most recent request was sent.
    pub last_request: Option<Instant>,
    pub scroll: super::ScrollState,
}

impl UsageState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Which agent's usage the panel should describe.
    ///
    /// The focused pane wins (that is "the agent you are looking at"); when
    /// focus sits on a non-agent pane we fall back to the sidebar selection
    /// so the tab keeps showing the agent the user picked.
    pub fn target(state: &AppState) -> Option<UsageTarget> {
        let pane = state
            .focus_state
            .focused_pane_id
            .as_deref()
            .and_then(|id| state.pane_by_id(id))
            .or_else(|| state.selected_pane())?;
        if pane.agent == crate::tmux::AgentType::Unknown {
            return None;
        }
        Some(UsageTarget {
            key: format!(
                "{}|{:?}|{}",
                pane.pane_id,
                pane.agent,
                pane.session_id.as_deref().unwrap_or("")
            ),
            agent: pane.agent.clone(),
            is_pi: is_pi_pane(pane),
            session_id: pane.session_id.clone(),
            cwd: pane.path.clone(),
        })
    }

    /// Decide whether a new snapshot should be requested now.
    ///
    /// A focus change is fetched immediately; otherwise the visible tab
    /// refreshes on [`USAGE_REFRESH_INTERVAL`]. The hidden tab still fetches
    /// once per target so switching to it shows data instantly.
    pub fn should_request(&mut self, key: &str, tab_visible: bool) -> bool {
        let changed = self.requested_key.as_deref() != Some(key);
        let stale = self
            .last_request
            .is_none_or(|at| at.elapsed() >= USAGE_REFRESH_INTERVAL);
        if !changed && !(tab_visible && stale) {
            return false;
        }
        self.requested_key = Some(key.to_string());
        self.last_request = Some(Instant::now());
        true
    }

    pub fn apply(&mut self, snapshot: UsageSnapshot) {
        // Ignore a late reply for an agent the user already left.
        if self.requested_key.as_deref() == Some(snapshot.key.as_str()) {
            self.snapshot = Some(snapshot);
        }
    }

    /// The snapshot that belongs to the currently focused agent.
    pub fn current(&self, key: &str) -> Option<&UsageSnapshot> {
        self.snapshot.as_ref().filter(|s| s.key == key)
    }
}

/// Pi reuses the Claude hook adapter, so its panes look like Claude panes.
/// The bundled extension prefixes user prompts and assistant previews with
/// `[Pi]`, and `pi` is the pane's foreground command while it runs.
fn is_pi_pane(pane: &crate::tmux::PaneInfo) -> bool {
    pane.prompt.starts_with("[Pi]")
        || pane.current_command == "pi"
        || pane.current_command.ends_with("/pi")
        || pane
            .current_command
            .split_whitespace()
            .next()
            .and_then(|cmd| cmd.rsplit('/').next())
            .is_some_and(|cmd| cmd == "pi")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::group::{PaneGitInfo, RepoGroup};
    use crate::tmux::{AgentType, PaneInfo, PaneStatus, PermissionMode, WorktreeMetadata};

    fn test_pane(id: &str, agent: AgentType) -> PaneInfo {
        PaneInfo {
            pane_id: id.into(),
            pane_active: false,
            status: PaneStatus::Running,
            attention: false,
            agent,
            path: "/tmp".into(),
            current_command: String::new(),
            prompt: String::new(),
            prompt_is_response: false,
            started_at: None,
            wait_reason: String::new(),
            permission_mode: PermissionMode::Default,
            subagents: vec![],
            pane_pid: None,
            worktree: WorktreeMetadata::default(),
            session_id: None,
            session_name: String::new(),
            sidebar_spawned: false,
            bg_shell_cmd: None,
        }
    }

    fn state_with(pane: PaneInfo, focused: Option<&str>) -> AppState {
        let mut state = AppState::new("%99".into());
        state.repo_groups = vec![RepoGroup {
            name: "project".into(),
            has_focus: true,
            panes: vec![(pane, PaneGitInfo::default())],
        }];
        state.focus_state.focused_pane_id = focused.map(str::to_string);
        state
    }

    #[test]
    fn target_follows_focused_agent() {
        let state = state_with(test_pane("%1", AgentType::Codex), Some("%1"));
        let target = UsageState::target(&state).expect("target");
        assert_eq!(target.agent, AgentType::Codex);
        assert!(target.key.starts_with("%1|Codex|"));
    }

    #[test]
    fn target_falls_back_to_sidebar_selection() {
        let mut state = state_with(test_pane("%1", AgentType::Claude), Some("%5"));
        // Selected row 0 is the only agent.
        state.global.selected_pane_row = 0;
        state.rebuild_row_targets();
        let target = UsageState::target(&state).expect("target");
        assert_eq!(target.agent, AgentType::Claude);
    }

    #[test]
    fn target_is_none_without_agents() {
        let mut state = AppState::new("%99".into());
        state.repo_groups = vec![];
        assert!(UsageState::target(&state).is_none());
    }

    #[test]
    fn request_happens_on_key_change() {
        let mut state = state_with(test_pane("%1", AgentType::Claude), Some("%1"));
        let key = UsageState::target(&state).unwrap().key;
        assert!(state.usage.should_request(&key, false));
        // Same key, tab hidden → no immediate repeat.
        assert!(!state.usage.should_request(&key, false));
        // Same key, tab visible but not stale yet → still skipped.
        assert!(!state.usage.should_request(&key, true));
    }

    #[test]
    fn request_resends_when_visible_and_stale() {
        let mut state = state_with(test_pane("%1", AgentType::Claude), Some("%1"));
        let key = UsageState::target(&state).unwrap().key;
        assert!(state.usage.should_request(&key, true));
        state.usage.last_request = Some(Instant::now() - USAGE_REFRESH_INTERVAL);
        assert!(state.usage.should_request(&key, true));
    }

    #[test]
    fn late_snapshot_for_previous_agent_is_ignored() {
        let mut state = state_with(test_pane("%1", AgentType::Claude), Some("%1"));
        state.usage.requested_key = Some("%1|Claude|".into());
        state.usage.apply(UsageSnapshot {
            key: "%2|Codex|".into(),
            title: "Codex".into(),
            fetched_at: 1,
            ..UsageSnapshot::default()
        });
        assert!(state.usage.snapshot.is_none());
    }

    #[test]
    fn current_filters_by_key() {
        let mut state = AppState::new("%99".into());
        state.usage.snapshot = Some(UsageSnapshot {
            key: "a".into(),
            title: "Claude Code".into(),
            fetched_at: 1,
            ..UsageSnapshot::default()
        });
        assert!(state.usage.current("a").is_some());
        assert!(state.usage.current("b").is_none());
    }

    #[test]
    fn pi_detected_from_prompt_prefix() {
        let mut pane = test_pane("%1", AgentType::Claude);
        pane.prompt = "[Pi] do the thing".into();
        assert!(is_pi_pane(&pane));
    }

    #[test]
    fn pi_detected_from_command() {
        let mut pane = test_pane("%1", AgentType::Claude);
        pane.current_command = "/home/me/.local/bin/pi".into();
        assert!(is_pi_pane(&pane));
        pane.current_command = "pi --continue".into();
        assert!(is_pi_pane(&pane));
    }

    #[test]
    fn claude_pane_is_not_pi() {
        assert!(!is_pi_pane(&test_pane("%1", AgentType::Claude)));
    }
}
