use agents_tmux::{tmux, worktree};
use std::process::Command;

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
    let branch = worktree::spawn(&worktree::SpawnRequest {
        repo_root: root.to_path_buf(),
        task_name: "first".into(),
        session: "workspace-owner".into(),
        agent: "sleep 600".into(),
        mode: "default".into(),
    })
    .unwrap();
    assert_eq!(branch, "agent/first");
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
    assert_eq!(worktree::read_spawn_markers(agent).branch, "agent/first");
    assert!(!worktree::read_spawn_markers(original).spawned);
    tmux::workspace::show(host, original, "workspace-owner").unwrap();
    tmux::run_tmux_capture(&["select-pane", "-t", &sidebar]).unwrap();
    assert_eq!(tmux::find_active_pane(&sidebar).unwrap().0, original);
    assert_eq!(tmux::display_message(original, "#{pane_pid}"), original_pid);
    assert_eq!(worktree::read_spawn_markers(agent).branch, "agent/first");
    assert!(!worktree::read_spawn_markers(original).spawned);
    tmux::workspace::show(host, agent, "workspace-owner").unwrap();
    assert_eq!(tmux::find_active_pane(&sidebar).unwrap().0, agent);
    assert_eq!(tmux::display_message(agent, "#{pane_pid}"), agent_pid);
    // Removing a visible agent must remove only its slot, never the host.
    worktree::remove(agent, worktree::RemoveMode::WindowAndWorktree).unwrap();
    assert!(!root.join(".worktrees/first").exists());
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
