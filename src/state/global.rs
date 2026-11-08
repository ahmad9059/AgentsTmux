use std::collections::HashMap;
use std::time::Instant;

use crate::tmux;

use super::filter::{RepoFilter, StatusFilter};

/// View state shared only by sidebars in the same tmux session.
/// Synced from tmux at startup and on pane focus change (SIGUSR1).
pub struct GlobalState {
    session: Option<String>,
    pub status_filter: StatusFilter,
    pub selected_pane_row: usize,
    pub repo_filter: RepoFilter,
    /// Last filter value successfully written to tmux.
    last_saved_filter: StatusFilter,
    /// Last cursor value successfully written to tmux.
    last_saved_cursor: usize,
    /// Last repo filter value successfully written to tmux.
    last_saved_repo_filter: RepoFilter,
    /// When the selected cursor was last changed and still needs persisting.
    pending_cursor_save_since: Option<Instant>,
}

impl Default for GlobalState {
    fn default() -> Self {
        Self::new()
    }
}

impl GlobalState {
    pub fn new() -> Self {
        Self {
            session: None,
            status_filter: StatusFilter::All,
            selected_pane_row: 0,
            repo_filter: RepoFilter::All,
            last_saved_filter: StatusFilter::All,
            last_saved_cursor: 0,
            last_saved_repo_filter: RepoFilter::All,
            pending_cursor_save_since: None,
        }
    }

    /// Stable tmux session IDs keep view state isolated through renames.
    /// Return true when a sidebar starts or moves into another session.
    pub fn bind_session(&mut self, session_id: &str) -> bool {
        if self.session.as_deref() == Some(session_id) {
            return false;
        }
        *self = Self::new();
        self.session = Some(session_id.to_string());
        true
    }

    fn write_option(&self, key: &str, value: &str) -> bool {
        match self.session.as_deref() {
            Some(session) => tmux::run_tmux(&["set-option", "-t", session, key, value]),
            None => None,
        }
        .is_some()
    }

    /// Save the filter in this session's tmux options.
    /// Only updates `last_saved_filter` on success so that a failed write
    /// does not cause sync to overwrite the user's choice.
    pub fn save_filter(&mut self) {
        if self.write_option(tmux::SIDEBAR_FILTER, self.status_filter.as_str()) {
            self.last_saved_filter = self.status_filter;
        }
    }

    /// Save the cursor in this session's tmux options. Returns `true` when
    /// tmux accepted the write so callers can decide whether to clear a
    /// queued save or keep retrying.
    pub fn save_cursor(&mut self) -> bool {
        if self.write_option(tmux::SIDEBAR_CURSOR, &self.selected_pane_row.to_string()) {
            self.last_saved_cursor = self.selected_pane_row;
            true
        } else {
            false
        }
    }

    /// Mark the cursor as dirty so the main loop can persist it once the
    /// user pauses navigation.
    pub fn queue_cursor_save(&mut self) {
        self.pending_cursor_save_since = Some(Instant::now());
    }

    /// Persist a queued cursor update after it has been idle for at least the
    /// requested debounce duration. Returns true when the queue was consumed.
    pub fn flush_pending_cursor_save(&mut self, debounce: std::time::Duration) -> bool {
        let Some(queued_at) = self.pending_cursor_save_since else {
            return false;
        };
        if queued_at.elapsed() < debounce {
            return false;
        }
        // Only clear the pending marker on successful tmux write — otherwise
        // a transient failure would silently drop the queued save instead of
        // retrying on the next flush tick.
        if self.save_cursor() {
            self.pending_cursor_save_since = None;
            true
        } else {
            false
        }
    }

    /// Save the repo filter in this session's tmux options.
    pub fn save_repo_filter(&mut self) {
        if self.write_option(tmux::SIDEBAR_REPO_FILTER, self.repo_filter.as_str()) {
            self.last_saved_repo_filter = self.repo_filter.clone();
        }
    }

    /// Load view state without inheriting other sessions' legacy values.
    /// Called at startup and on SIGUSR1 (pane focus change).
    pub fn load_from_tmux(&mut self) {
        let opts = match self.session.as_deref() {
            Some(session) => tmux::run_tmux(&["show-options", "-t", session])
                .map(|output| {
                    output
                        .lines()
                        .filter_map(|line| line.split_once(' '))
                        .map(|(key, value)| (key.to_string(), value.trim_matches('"').to_string()))
                        .collect()
                })
                .unwrap_or_default(),
            None => HashMap::new(),
        };
        self.apply_all(&opts);
    }

    /// Apply all global options from tmux (filter, cursor, repo filter).
    pub fn apply_all(&mut self, opts: &HashMap<String, String>) {
        if let Some(filter_str) = opts.get(tmux::SIDEBAR_FILTER) {
            let tmux_filter = StatusFilter::from_label(filter_str);
            if tmux_filter != self.last_saved_filter {
                self.status_filter = tmux_filter;
                self.last_saved_filter = tmux_filter;
            }
        }
        if let Some(cursor_str) = opts.get(tmux::SIDEBAR_CURSOR)
            && let Ok(n) = cursor_str.parse::<usize>()
            && n != self.last_saved_cursor
        {
            self.selected_pane_row = n;
            self.last_saved_cursor = n;
        }
        if let Some(repo_str) = opts.get(tmux::SIDEBAR_REPO_FILTER) {
            let tmux_repo = RepoFilter::from_label(repo_str);
            if tmux_repo != self.last_saved_repo_filter {
                self.repo_filter = tmux_repo.clone();
                self.last_saved_repo_filter = tmux_repo;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changing_session_resets_local_view_and_pending_writes() {
        let mut state = GlobalState::new();
        assert!(state.bind_session("$1"));
        state.selected_pane_row = 3;
        state.repo_filter = RepoFilter::Repo("first".into());
        state.queue_cursor_save();
        assert!(!state.bind_session("$1"));
        assert_eq!(state.selected_pane_row, 3);
        assert!(state.bind_session("$2"));
        assert_eq!(state.selected_pane_row, 0);
        assert_eq!(state.repo_filter, RepoFilter::All);
        assert!(state.pending_cursor_save_since.is_none());
    }
}
