//! One visible agent slot with live panes parked in a detached session.
use super::commands::{display_message, run_tmux, run_tmux_capture};

pub const HOST: &str = "@agent_sidebar_host_window";
pub const OWNER: &str = "@agent_sidebar_owner_session";
pub const PARKING_PREFIX: &str = "__agent-sidebar-";
const MARKERS: [&str; 4] = [
    "@agent-sidebar-spawned",
    "@agent-sidebar-spawned-from",
    "@agent-sidebar-spawned-worktree",
    "@agent-sidebar-spawned-branch",
];

pub fn enabled() -> bool {
    super::options::get_all_global_options()
        .get("@sidebar_single_window")
        .is_some_and(|v| v == "on")
}

pub fn is_parking_window(window: &str) -> bool {
    display_message(window, "#{session_name}").starts_with(PARKING_PREFIX)
}

pub fn host_for_session(session: &str) -> Result<String, String> {
    let saved = display_message(session, &format!("#{{{HOST}}}"));
    if !saved.is_empty() && display_message(&saved, "#{session_name}") == session {
        return Ok(saved);
    }
    let caller = std::env::var("TMUX_PANE").unwrap_or_default();
    if caller.is_empty() || display_message(&caller, "#{session_name}") != session {
        return Err("single-window mode needs an Agents host window".into());
    }
    let host = display_message(&caller, "#{window_id}");
    run_tmux_capture(&["set-option", "-t", session, HOST, &host])?;
    Ok(host)
}

pub fn ensure_parking(owner: &str, cwd: &str) -> Result<String, String> {
    let id = display_message(owner, "#{session_id}");
    if id.is_empty() {
        return Err("owner session no longer exists".into());
    }
    let parking = format!("{PARKING_PREFIX}{}", id.trim_start_matches('$'));
    if run_tmux(&["has-session", "-t", &format!("={parking}")]).is_none() {
        // Keep an anchor so swapping or deleting the last agent cannot remove
        // the holding session. This process never touches an agent's terminal.
        run_tmux_capture(&[
            "new-session",
            "-d",
            "-s",
            &parking,
            "-n",
            "holding",
            "-c",
            cwd,
            "sleep 2147483647",
        ])?;
        run_tmux_capture(&[
            "set-option",
            "-t",
            &parking,
            "@agent_sidebar_parking_owner",
            owner,
        ])?;
    }
    Ok(parking)
}

/// Move worktree ownership from the old window to its agent pane before a swap.
pub fn adopt(pane: &str, host: &str, owner: &str) -> Result<(), String> {
    let window = display_message(pane, "#{window_id}");
    if window.is_empty() {
        return Err("agent pane no longer exists".into());
    }
    for key in MARKERS {
        let value = display_message(pane, &format!("#{{{key}}}"));
        if !value.is_empty() {
            run_tmux_capture(&["set-option", "-p", "-t", pane, key, &value])?;
        }
    }
    for key in MARKERS {
        let _ = run_tmux(&["set-option", "-wu", "-t", &window, key]);
    }
    run_tmux_capture(&["set-option", "-p", "-t", pane, HOST, host])?;
    run_tmux_capture(&["set-option", "-p", "-t", pane, OWNER, owner])?;
    Ok(())
}

fn content_pane(host: &str) -> Result<String, String> {
    let panes = run_tmux_capture(&["list-panes", "-t", host, "-F", "#{pane_id}|#{@pane_role}"])?;
    let content: Vec<_> = panes
        .lines()
        .filter_map(|line| {
            let (pane, role) = line.split_once('|')?;
            (role != "sidebar").then_some(pane)
        })
        .collect();
    if content.len() != 1 {
        return Err("Agents host must contain one sidebar and one content pane".into());
    }
    Ok(content[0].to_string())
}

pub fn show(host: &str, pane: &str, owner: &str) -> Result<(), String> {
    let current = content_pane(host)?;
    if current != pane {
        let placeholder = display_message(&current, "#{@pane_role}") == "agent-placeholder";
        adopt(&current, host, owner)?;
        adopt(pane, host, owner)?;
        // Pane IDs, PTYs, processes, hooks, and scrollback all survive the swap.
        run_tmux_capture(&["swap-pane", "-d", "-s", pane, "-t", &current])?;
        if placeholder {
            run_tmux_capture(&["kill-pane", "-t", &current])?;
        }
    }
    run_tmux_capture(&["select-window", "-t", host])?;
    run_tmux_capture(&["select-pane", "-t", pane])?;
    Ok(())
}

pub fn select_from_sidebar(pane: &str) -> Result<bool, String> {
    if !enabled() {
        return Ok(false);
    }
    let caller = std::env::var("TMUX_PANE").unwrap_or_default();
    if caller.is_empty() || display_message(&caller, "#{@pane_role}") != "sidebar" {
        return Ok(false);
    }
    let host = display_message(&caller, "#{window_id}");
    let owner = display_message(&caller, "#{session_name}");
    let pane_owner = display_message(pane, &format!("#{{{OWNER}}}"));
    if pane_owner != owner && display_message(pane, "#{session_name}") != owner {
        return Ok(false);
    }
    show(&host, pane, &owner)?;
    Ok(true)
}

/// Worktree removal must never kill the shared Agents window. Park its target
/// first; the existing cleanup then operates only on that agent's private slot.
pub fn park_before_remove(pane: &str) -> Result<(), String> {
    let host = display_message(pane, &format!("#{{{HOST}}}"));
    if host.is_empty() || display_message(pane, "#{window_id}") != host {
        return Ok(());
    }
    let owner = display_message(pane, &format!("#{{{OWNER}}}"));
    let repo = display_message(pane, "#{@agent-sidebar-spawned-from}");
    let cwd = if repo.is_empty() {
        display_message(pane, "#{pane_current_path}")
    } else {
        repo
    };
    let parking = ensure_parking(&owner, &cwd)?;
    let placeholder = run_tmux_capture(&[
        "new-window",
        "-d",
        "-t",
        &parking,
        "-n",
        "empty",
        "-c",
        &cwd,
        "-P",
        "-F",
        "#{pane_id}",
    ])?;
    run_tmux_capture(&[
        "set-option",
        "-p",
        "-t",
        &placeholder,
        "@pane_role",
        "agent-placeholder",
    ])?;
    run_tmux_capture(&["swap-pane", "-d", "-s", pane, "-t", &placeholder])?;
    Ok(())
}
