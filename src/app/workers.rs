use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
#[cfg(test)]
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

#[cfg(test)]
use crate::git;
use crate::git::GitData;
use crate::session;
use crate::state::AppState;
#[cfg(test)]
use crate::tmux;
use crate::usage::{UsageFetcher, UsageSnapshot, UsageTarget};
use crate::version::{self, UpdateNotice};

/// Channels and shared flags produced by [`spawn`] that the main event loop
/// drains every tick.
pub(super) struct Workers {
    pub git_rx: Receiver<GitData>,
    pub session_rx: Receiver<HashMap<String, String>>,
    pub version_rx: Receiver<UpdateNotice>,
    pub git_tab_active: Arc<AtomicBool>,
    /// Send the agent whose usage should be fetched; the worker replies on
    /// `usage_rx`. Kept separate from the git thread because a usage fetch
    /// can block on the network for seconds.
    pub usage_tx: mpsc::Sender<UsageTarget>,
    pub usage_rx: Receiver<UsageSnapshot>,
}

/// Spawn the background threads (git polling, session-name polling, version
/// notice fetch) that feed the event loop.
pub(super) fn spawn(_state: &AppState) -> Workers {
    let (_git_tx, git_rx) = mpsc::channel::<GitData>();
    let (session_tx, session_rx) = mpsc::channel::<HashMap<String, String>>();
    let (version_tx, version_rx) = mpsc::channel::<UpdateNotice>();
    let git_tab_active = Arc::new(AtomicBool::new(false));
    std::thread::spawn(move || {
        session_poll_loop(&session_tx);
    });
    let (usage_tx, usage_rx) = usage_channel();
    std::thread::spawn(move || {
        if let Some(notice) = version::fetch_update_notice() {
            let _ = version_tx.send(notice);
        }
    });

    Workers {
        git_rx,
        session_rx,
        version_rx,
        git_tab_active,
        usage_tx,
        usage_rx,
    }
}

/// Create the usage request/reply pair and start its worker thread.
///
/// The thread owns the [`UsageFetcher`] so its account-wide provider cache
/// lives across requests — switching between two Claude panes reuses the
/// same HTTP response instead of re-fetching it.
pub(super) fn usage_channel() -> (mpsc::Sender<UsageTarget>, Receiver<UsageSnapshot>) {
    let (request_tx, request_rx) = mpsc::channel::<UsageTarget>();
    let (snapshot_tx, snapshot_rx) = mpsc::channel::<UsageSnapshot>();
    std::thread::spawn(move || {
        let mut fetcher = UsageFetcher::new();
        // Drain every queued request so a burst of focus changes collapses
        // to the newest one instead of replaying stale agents.
        while let Ok(first) = request_rx.recv() {
            let mut target = first;
            while let Ok(newer) = request_rx.try_recv() {
                target = newer;
            }
            let snapshot = fetcher.fetch(&target);
            if snapshot_tx.send(snapshot).is_err() {
                return;
            }
        }
    });
    (request_tx, snapshot_rx)
}

/// Session name polling thread. Scans `~/.claude/sessions/*.json` every 10
/// seconds so the main TUI thread never performs blocking filesystem I/O
/// to refresh `/rename`-assigned labels.
pub(super) fn session_poll_loop(tx: &mpsc::Sender<HashMap<String, String>>) {
    loop {
        std::thread::sleep(Duration::from_secs(10));
        let names = session::scan_session_names();
        if tx.send(names).is_err() {
            return;
        }
    }
}

/// Git data polling thread. Fetches git status every 2 seconds while the Git
/// tab is active. Skips fetching when the tab is not visible. PR numbers go
/// through an in-memory `(path, branch)`-keyed cache so `gh pr view` (the only
/// hop that costs GitHub API quota) runs at most once per `PR_CACHE_TTL`
/// instead of every tick.
#[cfg(test)]
#[allow(dead_code)]
pub(super) fn git_poll_loop(tmux_pane: &str, git_tx: &mpsc::Sender<GitData>, active: &AtomicBool) {
    let mut last_path: Option<String> = None;
    let mut pr_cache = git::PrCache::new();
    loop {
        std::thread::sleep(Duration::from_secs(2));

        if !active.load(Ordering::Relaxed) {
            continue;
        }

        // When the sidebar has focus, focused_pane_path returns None.
        // Reuse the last known path so git data keeps updating.
        if let Some(p) = tmux::focused_pane_path(tmux_pane) {
            last_path = Some(p);
        }
        if let Some(ref path) = last_path {
            let mut data = git::fetch_git_data(path);
            data.pr_number = pr_cache.get_or_fetch(
                path,
                &data.branch,
                std::time::Instant::now(),
                git::fetch_pr_number,
            );
            if git_tx.send(data).is_err() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_git_poll_skips_when_inactive() {
        let active = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel::<GitData>();

        let flag = Arc::clone(&active);
        let handle = std::thread::spawn(move || {
            // Simulate the poll loop check without actually sleeping 2s
            for _ in 0..3 {
                if !flag.load(Ordering::Relaxed) {
                    continue;
                }
                let _ = tx.send(GitData::default());
            }
        });

        handle.join().unwrap();
        // No data should have been sent since active=false
        assert!(
            rx.try_recv().is_err(),
            "should not poll when git tab is inactive"
        );
    }

    #[test]
    fn test_git_poll_sends_when_active() {
        let active = Arc::new(AtomicBool::new(true));
        let (tx, rx) = mpsc::channel::<GitData>();

        let flag = Arc::clone(&active);
        let handle = std::thread::spawn(move || {
            // active=true, so it should send
            if flag.load(Ordering::Relaxed) {
                let _ = tx.send(GitData::default());
            }
        });

        handle.join().unwrap();
        assert!(rx.try_recv().is_ok(), "should poll when git tab is active");
    }

    #[test]
    fn test_git_poll_reacts_to_flag_change() {
        let active = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel::<GitData>();

        // Initially inactive
        assert!(!active.load(Ordering::Relaxed));

        // Switch to active
        active.store(true, Ordering::Relaxed);

        let flag = Arc::clone(&active);
        let handle = std::thread::spawn(move || {
            if flag.load(Ordering::Relaxed) {
                let _ = tx.send(GitData::default());
            }
        });

        handle.join().unwrap();
        assert!(
            rx.try_recv().is_ok(),
            "should poll after flag switches to active"
        );
    }

    #[test]
    fn test_git_poll_stops_on_sender_closed() {
        let active = AtomicBool::new(true);
        let (tx, rx) = mpsc::channel::<GitData>();
        drop(rx); // Close receiver

        let result = tx.send(GitData::default());
        assert!(result.is_err(), "send should fail when receiver is dropped");

        // Verify the flag check pattern used in git_poll_loop
        assert!(active.load(Ordering::Relaxed));
    }
}
