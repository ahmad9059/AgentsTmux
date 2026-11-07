use std::time::Instant;

use super::AppState;

impl AppState {
    // ─── Flash banner ────────────────────────────────────────────────

    pub fn set_flash(&mut self, msg: impl Into<String>) {
        self.flash = Some((
            msg.into(),
            Instant::now() + std::time::Duration::from_secs(4),
        ));
    }

    /// Return the current flash text if still valid, clearing it once the
    /// deadline passes. Called by the UI once per frame.
    pub fn take_flash(&mut self) -> Option<String> {
        match &self.flash {
            Some((text, exp)) if Instant::now() < *exp => Some(text.clone()),
            Some(_) => {
                self.flash = None;
                None
            }
            None => None,
        }
    }

    pub fn apply_git_data(&mut self, data: crate::git::GitData) {
        self.git = data;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_flash_stores_message_with_future_expiry() {
        let mut state = AppState::new("%99".into());
        state.set_flash("hello");
        let (msg, exp) = state.flash.as_ref().expect("flash must be set");
        assert_eq!(msg, "hello");
        assert!(*exp > Instant::now());
    }

    #[test]
    fn take_flash_returns_message_then_noop_on_expiry() {
        let mut state = AppState::new("%99".into());
        state.set_flash("msg");
        assert_eq!(state.take_flash().as_deref(), Some("msg"));
        // Force-expire by rewinding the deadline into the past.
        state.flash = Some((
            "stale".into(),
            Instant::now() - std::time::Duration::from_secs(1),
        ));
        assert!(state.take_flash().is_none());
        assert!(state.flash.is_none());
    }

    #[test]
    fn take_flash_none_when_unset() {
        let mut state = AppState::new("%99".into());
        assert!(state.take_flash().is_none());
    }

    #[test]
    fn apply_git_data_overwrites_previous_state() {
        let mut state = AppState::new("%99".into());
        state.git.branch = "old".into();
        state.apply_git_data(crate::git::GitData {
            branch: "new".into(),
            diff_stat: Some((3, 1)),
            ..Default::default()
        });
        assert_eq!(state.git.branch, "new");
        assert_eq!(state.git.diff_stat, Some((3, 1)));
    }
}
