use agents_tmux::{tmux, worktree};
use std::process::Command;
use std::sync::Mutex;

/// The tests share one disposable tmux server, which exits when its last
/// session ends, so they must not overlap.
static SERVER: Mutex<()> = Mutex::new(());

fn command(program: &str, args: &[&str], cwd: &std::path::Path) {
    let result = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
#[ignore = "requires a dedicated disposable tmux socket via the test wrapper"]
fn single_window_agents_keep_processes_and_worktree_ownership() {
    let _server = SERVER.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(std::env::var("WORKSPACE_TEST_ISOLATED").as_deref(), Ok("1"));
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    command("git", &["init", "-q"], root);
    command(
        "git",
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--allow-empty",
            "-qm",
            "initial",
        ],
        root,
    );
    let location = root.to_str().unwrap();
    let out = tmux::run_tmux_capture(&[
        "new-session",
        "-d",
        "-s",
        "workspace-owner",
        "-n",
        "Agents",
        "-c",
        location,
        "-P",
        "-F",
        "#{window_id}|#{pane_id}",
        "sleep 600",
    ])
    .unwrap();
    let (host, original) = out.split_once('|').unwrap();
    let original_pid = tmux::display_message(original, "#{pane_pid}");
    let sidebar = tmux::run_tmux_capture(&[
        "split-window",
        "-h",
        "-t",
        host,
        "-P",
        "-F",
        "#{pane_id}",
        "sleep 600",
    ])
    .unwrap();
    tmux::run_tmux_capture(&["set-option", "-p", "-t", &sidebar, "@pane_role", "sidebar"]).unwrap();
    tmux::run_tmux_capture(&["set-option", "-g", "@sidebar_single_window", "on"]).unwrap();
    tmux::run_tmux_capture(&[
        "set-option",
        "-t",
        "workspace-owner",
        tmux::workspace::HOST,
        host,
    ])
    .unwrap();
    let folder = root.join("app");
    std::fs::create_dir(&folder).unwrap();
    worktree::spawn(&worktree::SpawnRequest {
        repo_root: root.to_path_buf(),
        directory: folder.clone(),
        session: "workspace-owner".into(),
        agent: "sleep 600".into(),
        mode: "default".into(),
    })
    .unwrap();
    assert_eq!(
        tmux::run_tmux_capture(&[
            "list-windows",
            "-t",
            "workspace-owner",
            "-F",
            "#{window_id}"
        ])
        .unwrap(),
        host
    );
    let visible =
        tmux::run_tmux_capture(&["list-panes", "-t", host, "-F", "#{pane_id}|#{@pane_role}"])
            .unwrap();
    let agent = visible
        .lines()
        .find_map(|line| {
            line.split_once('|')
                .filter(|(_, role)| *role != "sidebar")
                .map(|(p, _)| p)
        })
        .unwrap();
    assert_ne!(agent, original);
    let agent_pid = tmux::display_message(agent, "#{pane_pid}");
    assert_eq!(
        worktree::read_spawn_markers(agent).from_repo,
        folder.to_str().unwrap()
    );
    assert!(worktree::read_spawn_markers(agent).worktree_path.is_empty());
    assert!(!worktree::read_spawn_markers(original).spawned);
    tmux::workspace::show(host, original, "workspace-owner").unwrap();
    tmux::run_tmux_capture(&["select-pane", "-t", &sidebar]).unwrap();
    assert_eq!(tmux::find_active_pane(&sidebar).unwrap().0, original);
    assert_eq!(tmux::display_message(original, "#{pane_pid}"), original_pid);
    assert!(worktree::read_spawn_markers(agent).spawned);
    assert!(!worktree::read_spawn_markers(original).spawned);
    tmux::workspace::show(host, agent, "workspace-owner").unwrap();
    assert_eq!(tmux::find_active_pane(&sidebar).unwrap().0, agent);
    assert_eq!(tmux::display_message(agent, "#{pane_pid}"), agent_pid);
    // Removing a visible agent must remove only its slot, never the host.
    worktree::remove(agent, worktree::RemoveMode::WindowAndWorktree).unwrap();
    assert!(folder.is_dir(), "closing an agent never deletes its folder");
    assert!(
        !root.join(".worktrees").exists(),
        "no worktree is ever created"
    );
    assert!(!tmux::display_message(host, "#{window_id}").is_empty());
    assert_eq!(tmux::display_message(original, "#{pane_pid}"), original_pid);
    tmux::workspace::show(host, original, "workspace-owner").unwrap();
    assert_eq!(
        tmux::run_tmux_capture(&["list-panes", "-t", host, "-F", "#{pane_id}"])
            .unwrap()
            .lines()
            .count(),
        2
    );
    let parking = tmux::workspace::ensure_parking("workspace-owner", location).unwrap();
    tmux::run_tmux_capture(&["kill-session", "-t", "workspace-owner"]).unwrap();
    tmux::run_tmux_capture(&["kill-session", "-t", &parking]).unwrap();
}

#[test]
#[ignore = "requires a dedicated disposable tmux socket via the test wrapper"]
fn a_booting_agent_survives_until_its_grace_period_ends() {
    let _server = SERVER.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(std::env::var("WORKSPACE_TEST_ISOLATED").as_deref(), Ok("1"));
    let set = |pane: &str, key: &str, value: &str| {
        tmux::run_tmux_capture(&["set-option", "-p", "-t", pane, key, value]).unwrap();
    };
    let pane = tmux::run_tmux_capture(&[
        "new-session",
        "-d",
        "-s",
        "grace-test",
        "-P",
        "-F",
        "#{pane_id}",
        "bash",
    ])
    .unwrap();
    let pane = pane.trim();
    let listed = || {
        tmux::query_sessions()
            .iter()
            .flat_map(|s| &s.windows)
            .flat_map(|w| &w.panes)
            .any(|p| p.pane_id == pane)
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // Typed into a shell moments ago: the agent has not replaced it yet.
    set(pane, "@pane_agent", "codex");
    set(pane, "@pane_status", "idle");
    set(pane, tmux::PANE_SPAWN_GRACE, &now.to_string());
    assert!(listed(), "a booting agent must stay in the sidebar");

    // Long gone: the pane fell back to a shell, so the metadata is stale.
    set(pane, tmux::PANE_SPAWN_GRACE, &(now - 600).to_string());
    assert!(
        !listed(),
        "a dead agent must be dropped after the grace period"
    );
    assert!(tmux::get_pane_option_value(pane, tmux::PANE_AGENT).is_empty());
    tmux::run_tmux_capture(&["kill-session", "-t", "grace-test"]).unwrap();
}

#[test]
#[ignore = "requires a dedicated disposable tmux socket via the test wrapper"]
fn the_window_switches_to_another_agent_when_the_visible_one_exits() {
    let _server = SERVER.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(std::env::var("WORKSPACE_TEST_ISOLATED").as_deref(), Ok("1"));
    let tmux_ok = |args: &[&str]| {
        tmux::run_tmux_capture(args)
            .unwrap_or_else(|e| panic!("{args:?}: {e}"))
            .trim()
            .to_string()
    };
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let new_session = |name: &str, dir: &std::path::Path| {
        tmux_ok(&[
            "new-session",
            "-d",
            "-s",
            name,
            "-c",
            dir.to_str().unwrap(),
            "-P",
            "-F",
            "#{window_id}|#{pane_id}",
            "sleep 600",
        ])
    };
    let mark = |pane: &str, agent: &str| {
        tmux_ok(&["set-option", "-p", "-t", pane, "@pane_agent", agent]);
        tmux_ok(&["set-option", "-p", "-t", pane, "@pane_status", "idle"]);
    };

    let created = new_session("fallback-owner", first.path());
    let (host, agent_a) = created.split_once('|').unwrap();
    let sidebar = tmux_ok(&[
        "split-window",
        "-h",
        "-d",
        "-t",
        host,
        "-P",
        "-F",
        "#{pane_id}",
        "sleep 600",
    ]);
    tmux_ok(&["set-option", "-p", "-t", &sidebar, "@pane_role", "sidebar"]);
    tmux_ok(&[
        "set-option",
        "-t",
        "fallback-owner",
        tmux::workspace::HOST,
        host,
    ]);
    mark(agent_a, "claude");
    tmux_ok(&[
        "set-option",
        "-p",
        "-t",
        agent_a,
        "@agent-sidebar-spawned",
        "1",
    ]);
    let parked = new_session("fallback-park", second.path());
    let agent_b = parked.split_once('|').unwrap().1.to_string();
    mark(&agent_b, "claude");

    let mut state = agents_tmux::state::AppState::new(sidebar.clone());
    state.persistent_agent_highlight = true;
    state.refresh();
    state.follow_exited_agent();
    assert_eq!(tmux::workspace::content_pane(host).unwrap(), agent_a);

    // The agent exits: its pane drops to a shell and the hook clears the mark.
    tmux_ok(&["respawn-pane", "-k", "-t", agent_a, "bash"]);
    tmux_ok(&["set-option", "-p", "-u", "-t", agent_a, "@pane_agent"]);
    // The fast path reacts within a couple of ticks, without a refresh.
    for _ in 0..4 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        state.follow_exited_agent_fast();
    }
    let layout = tmux_ok(&[
        "list-panes",
        "-a",
        "-F",
        "#{session_name}:#{window_id} #{pane_id} #{pane_current_command} #{@pane_role}",
    ]);
    assert_eq!(
        tmux::workspace::content_pane(host).unwrap(),
        agent_b,
        "{layout}"
    );
    let panes = tmux_ok(&["list-panes", "-a", "-F", "#{pane_id}"]);
    assert!(
        !panes.lines().any(|p| p == agent_a),
        "the leftover shell is closed"
    );
    // The holding session ends on its own once its only pane is closed.
    let _ = tmux::run_tmux_capture(&["kill-session", "-t", "fallback-owner"]);
    let _ = tmux::run_tmux_capture(&["kill-session", "-t", "fallback-park"]);
}
