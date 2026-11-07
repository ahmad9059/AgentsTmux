//! Live checks against the real provider endpoints. Ignored by default: they
//! need network access and the credentials of whoever runs them.
//!
//! ```text
//! cargo test --test usage_live -- --ignored --nocapture
//! ```

use agents_tmux::tmux::AgentType;
use agents_tmux::usage::{UsageFetcher, UsageTarget};

fn target(agent: AgentType) -> UsageTarget {
    UsageTarget {
        key: format!("{agent:?}"),
        agent,
        is_pi: false,
        session_id: None,
        cwd: std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
    }
}

#[test]
#[ignore]
fn claude_usage_fetches_real_windows() {
    let snapshot = UsageFetcher::new().fetch(&target(AgentType::Claude));
    println!("{snapshot:#?}");
    assert!(
        !snapshot.windows.is_empty(),
        "expected windows, message={:?}",
        snapshot.message
    );
    for window in &snapshot.windows {
        assert!((0.0..=100.0).contains(&window.percent));
        assert!(window.resets_at.is_some(), "missing reset for {window:?}");
    }
}

#[test]
#[ignore]
fn codex_usage_fetches_real_windows() {
    let snapshot = UsageFetcher::new().fetch(&target(AgentType::Codex));
    println!("{snapshot:#?}");
    assert!(
        !snapshot.windows.is_empty(),
        "expected windows, message={:?}",
        snapshot.message
    );
}

#[test]
#[ignore]
fn opencode_usage_reports_local_spend() {
    let snapshot = UsageFetcher::new().fetch(&target(AgentType::OpenCode));
    println!("{snapshot:#?}");
    assert!(
        !snapshot.local.is_empty() || snapshot.message.is_some(),
        "expected local usage or a message"
    );
}
