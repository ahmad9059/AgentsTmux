use std::path::{Path, PathBuf};

use super::config::RemoveMode;
use super::env::{RealEnv, SpawnEnv};
use super::markers::{SPAWNED_FROM_OPTION, SPAWNED_OPTION, SpawnMarkers, spawn_markers_template};

#[derive(Debug, Clone)]
pub struct SpawnRequest {
    /// Folder the request originated from; the base for relative paths.
    pub repo_root: PathBuf,
    /// Folder the agent starts in. Already validated by
    /// [`resolve_directory`].
    pub directory: PathBuf,
    pub session: String,
    pub agent: String,
    pub mode: String,
}

/// Turn what the user typed into an existing directory. `~` expands to the
/// home directory and relative paths are taken from `base`. An empty input
/// means `base` itself, so the agent opens in the current folder.
pub fn resolve_directory(input: &str, base: &Path) -> Result<PathBuf, String> {
    let input = input.trim();
    let expanded = if input == "~" || input.starts_with("~/") {
        let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
        PathBuf::from(home).join(input.trim_start_matches('~').trim_start_matches('/'))
    } else if input.is_empty() {
        base.to_path_buf()
    } else {
        let path = PathBuf::from(input);
        if path.is_absolute() {
            path
        } else {
            base.join(path)
        }
    };
    if !expanded.is_dir() {
        return Err(format!("not a folder: {}", expanded.display()));
    }
    Ok(expanded.canonicalize().unwrap_or(expanded))
}

fn window_name(directory: &Path) -> String {
    directory
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "agent".into())
}

/// Open a new tmux window in `req.directory`, launch the agent there, and
/// mark the window so the matching `x` flow can find it later. No git
/// worktree or branch is created. Returns the new pane id.
pub fn spawn(req: &SpawnRequest) -> Result<String, String> {
    let caller = std::env::var("TMUX_PANE").unwrap_or_default();
    spawn_from_pane(req, &caller)
}

/// Start from a known pane instead of relying on the popup's environment.
pub fn spawn_from_pane(req: &SpawnRequest, caller: &str) -> Result<String, String> {
    if !crate::tmux::workspace::enabled() {
        return spawn_with(&RealEnv, req).map(|(pane, _)| pane);
    }
    let host = crate::tmux::workspace::host_for_session_from_pane(&req.session, caller)?;
    let parking =
        crate::tmux::workspace::ensure_parking(&req.session, &req.repo_root.to_string_lossy())?;
    let mut parked = req.clone();
    parked.session = parking;
    let (pane, _) = spawn_with(&RealEnv, &parked)?;
    // Keep a slot selectable during startup or first-run hook approval.
    if super::AGENTS.contains(&req.agent.as_str())
        && crate::tmux::get_pane_option_value(&pane, crate::tmux::PANE_AGENT).is_empty()
    {
        crate::tmux::set_pane_option(&pane, crate::tmux::PANE_AGENT, &req.agent);
        crate::tmux::set_pane_option(&pane, crate::tmux::PANE_STATUS, "idle");
        crate::tmux::set_pane_option(
            &pane,
            crate::tmux::PANE_SPAWN_GRACE,
            &crate::time::now_epoch_secs().to_string(),
        );
    }
    crate::tmux::workspace::show(&host, &pane, &req.session)
        .map_err(|e| format!("agent started in the holding session; could not display it: {e}"))?;
    Ok(pane)
}

pub(crate) fn spawn_with<E: SpawnEnv>(
    env: &E,
    req: &SpawnRequest,
) -> Result<(String, String), String> {
    let directory = req
        .directory
        .to_str()
        .ok_or("folder path is not valid UTF-8")?;
    let (pane_id, window_id) = env
        .new_window(&req.session, directory, &window_name(&req.directory))
        .map_err(|e| format!("tmux: {e}"))?;

    // Window scope so sub panes (e.g. Claude Code subagents split from
    // the original) inherit the markers via tmux's option fall-through.
    for (key, value) in [(SPAWNED_OPTION, "1"), (SPAWNED_FROM_OPTION, directory)] {
        if let Err(e) = env.set_window_option(&window_id, key, value) {
            return Err(rollback(
                format!("tmux: failed to set {key}: {e}"),
                env,
                &window_id,
            ));
        }
    }
    if let Err(e) = env.send_command(
        &pane_id,
        &super::config::agent_command(&req.agent, &req.mode),
    ) {
        return Err(rollback(format!("tmux: {e}"), env, &window_id));
    }
    Ok((pane_id, window_id))
}

/// Kill the half-created window and report both the cause and any failure
/// to clean up.
fn rollback<E: SpawnEnv>(primary: String, env: &E, window_id: &str) -> String {
    match env.kill_window(window_id) {
        Ok(()) => primary,
        Err(e) => format!("{primary} (rollback incomplete: kill_window: {e})"),
    }
}

/// Close a previously spawned agent. With `WindowAndWorktree`, a git
/// worktree created by an earlier AgentsTmux version is removed too, along
/// with its branch, BEFORE the window is killed so a git failure leaves the
/// window (and its markers) as the handle for a retry. Agents started in a
/// plain folder have no worktree, so only their window is closed.
pub fn remove(pane_id: &str, mode: RemoveMode) -> Result<(), String> {
    crate::tmux::workspace::park_before_remove(pane_id)?;
    remove_with(&RealEnv, pane_id, mode)
}

pub(crate) fn remove_with<E: SpawnEnv>(
    env: &E,
    pane_id: &str,
    mode: RemoveMode,
) -> Result<(), String> {
    let markers = SpawnMarkers::parse(&env.display_message(pane_id, &spawn_markers_template()));
    if !markers.is_spawned() {
        return Err("pane was not created by sidebar spawn".into());
    }
    if markers.window_id.is_empty() {
        return Err("could not resolve window id".into());
    }

    if mode == RemoveMode::WindowAndWorktree && !markers.worktree_path.is_empty() {
        if markers.branch.is_empty() {
            return Err("spawned branch is unset".into());
        }
        if env.worktree_path_exists(&markers.worktree_path) {
            env.worktree_remove(&markers.from_repo, &markers.worktree_path)
                .map_err(|e| format!("git: {e}"))?;
        }
        // `git worktree remove` leaves the branch ref behind.
        if env.branch_exists(&markers.from_repo, &markers.branch) {
            env.branch_delete(&markers.from_repo, &markers.branch)
                .map_err(|e| format!("git: {e}"))?;
        }
    }
    env.kill_window(&markers.window_id)
        .map_err(|e| format!("tmux: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod env_tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct FakeEnv {
        calls: RefCell<Vec<String>>,
        set_option_calls: RefCell<usize>,
        fail_set_option_at: Option<usize>,
        fail_kill_window: bool,
        fail_worktree_remove: bool,
        fail_branch_delete: bool,
        fail_send_command: bool,
        fail_new_window: bool,
        display_output: Option<String>,
        worktree_path_already_gone: Option<bool>,
        branch_already_gone: Option<bool>,
    }

    impl FakeEnv {
        fn log(&self, s: String) {
            self.calls.borrow_mut().push(s);
        }
        fn calls(&self) -> Vec<String> {
            self.calls.borrow().clone()
        }
    }

    impl SpawnEnv for FakeEnv {
        fn branch_exists(&self, _repo: &str, _branch: &str) -> bool {
            !self.branch_already_gone.unwrap_or(false)
        }
        fn worktree_path_exists(&self, _path: &str) -> bool {
            !self.worktree_path_already_gone.unwrap_or(false)
        }
        fn worktree_remove(&self, repo: &str, path: &str) -> Result<(), String> {
            self.log(format!("worktree_remove({repo},{path})"));
            if self.fail_worktree_remove {
                Err("worktree_remove failed".into())
            } else {
                Ok(())
            }
        }
        fn branch_delete(&self, repo: &str, branch: &str) -> Result<(), String> {
            self.log(format!("branch_delete({repo},{branch})"));
            if self.fail_branch_delete {
                Err("branch_delete failed".into())
            } else {
                Ok(())
            }
        }
        fn new_window(
            &self,
            session: &str,
            cwd: &str,
            name: &str,
        ) -> Result<(String, String), String> {
            self.log(format!("new_window({session},{cwd},{name})"));
            if self.fail_new_window {
                Err("new_window failed".into())
            } else {
                Ok(("%1".into(), "@1".into()))
            }
        }
        fn kill_window(&self, window_id: &str) -> Result<(), String> {
            self.log(format!("kill_window({window_id})"));
            if self.fail_kill_window {
                Err("kill_window failed".into())
            } else {
                Ok(())
            }
        }
        fn set_window_option(&self, window: &str, key: &str, value: &str) -> Result<(), String> {
            let idx = *self.set_option_calls.borrow();
            *self.set_option_calls.borrow_mut() += 1;
            self.log(format!("set_window_option({window},{key},{value})"));
            if Some(idx) == self.fail_set_option_at {
                Err(format!("set {key} failed"))
            } else {
                Ok(())
            }
        }
        fn send_command(&self, target: &str, command: &str) -> Result<(), String> {
            self.log(format!("send_command({target},{command})"));
            if self.fail_send_command {
                Err("send_command failed".into())
            } else {
                Ok(())
            }
        }
        fn display_message(&self, _pane_id: &str, _template: &str) -> String {
            self.display_output
                .clone()
                .unwrap_or_else(|| "1\n/r\n/r/.worktrees/task\nagent/task\n@1\n".into())
        }
    }

    fn sample_req() -> SpawnRequest {
        SpawnRequest {
            repo_root: PathBuf::from("/r"),
            directory: PathBuf::from("/r/app"),
            session: "sess".into(),
            agent: "claude".into(),
            mode: "default".into(),
        }
    }

    fn has_call(calls: &[String], prefix: &str) -> bool {
        calls.iter().any(|c| c.starts_with(prefix))
    }

    #[test]
    fn spawn_opens_the_agent_in_the_given_folder_without_git_changes() {
        let env = FakeEnv::default();
        let (pane, window) = spawn_with(&env, &sample_req()).expect("spawn should succeed");
        assert_eq!((pane.as_str(), window.as_str()), ("%1", "@1"));
        let calls = env.calls();
        assert!(has_call(&calls, "new_window(sess,/r/app,app)"));
        assert!(has_call(
            &calls,
            "set_window_option(@1,@agent-sidebar-spawned,1)"
        ));
        assert!(has_call(
            &calls,
            "set_window_option(@1,@agent-sidebar-spawned-from,/r/app)"
        ));
        assert_eq!(*env.set_option_calls.borrow(), 2);
        assert!(has_call(&calls, "send_command(%1,claude"));
        assert!(
            calls
                .iter()
                .all(|c| !c.contains("worktree") || c.contains("spawned")),
            "no worktree or branch may be created: {calls:?}"
        );
        assert!(!has_call(&calls, "kill_window("), "no rollback on success");
    }

    #[test]
    fn spawn_reports_new_window_failure_without_rollback() {
        let env = FakeEnv {
            fail_new_window: true,
            ..FakeEnv::default()
        };
        let err = spawn_with(&env, &sample_req()).expect_err("spawn must fail");
        assert!(err.contains("new_window failed"), "error: {err}");
        assert!(!has_call(&env.calls(), "kill_window("));
    }

    #[test]
    fn spawn_kills_window_when_a_marker_fails() {
        let env = FakeEnv {
            fail_set_option_at: Some(1),
            ..FakeEnv::default()
        };
        let err = spawn_with(&env, &sample_req()).expect_err("spawn must fail");
        assert!(err.contains(SPAWNED_FROM_OPTION), "error: {err}");
        let calls = env.calls();
        assert!(has_call(&calls, "kill_window(@1)"));
        assert!(!has_call(&calls, "send_command("));
    }

    #[test]
    fn spawn_kills_window_when_launching_the_agent_fails() {
        let env = FakeEnv {
            fail_send_command: true,
            ..FakeEnv::default()
        };
        let err = spawn_with(&env, &sample_req()).expect_err("spawn must fail");
        assert!(err.contains("send_command failed"), "error: {err}");
        assert!(has_call(&env.calls(), "kill_window(@1)"));
    }

    #[test]
    fn spawn_surfaces_rollback_failure() {
        let env = FakeEnv {
            fail_send_command: true,
            fail_kill_window: true,
            ..FakeEnv::default()
        };
        let err = spawn_with(&env, &sample_req()).expect_err("spawn must fail");
        assert!(err.contains("rollback incomplete"), "error: {err}");
        assert!(err.contains("kill_window failed"), "error: {err}");
    }

    #[test]
    fn resolve_directory_handles_empty_relative_absolute_and_missing() {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().canonicalize().unwrap();
        std::fs::create_dir(base.join("app")).unwrap();
        assert_eq!(resolve_directory("", &base).unwrap(), base);
        assert_eq!(
            resolve_directory("  app ", &base).unwrap(),
            base.join("app")
        );
        assert_eq!(
            resolve_directory(base.join("app").to_str().unwrap(), &base).unwrap(),
            base.join("app")
        );
        assert!(
            resolve_directory("missing", &base)
                .unwrap_err()
                .contains("not a folder")
        );
        std::fs::write(base.join("file"), "").unwrap();
        assert!(resolve_directory("file", &base).is_err());
    }

    #[test]
    fn window_name_uses_the_folder_name() {
        assert_eq!(window_name(Path::new("/r/app")), "app");
        assert_eq!(window_name(Path::new("/")), "agent");
    }

    #[test]
    fn remove_runs_all_git_steps_before_kill_window() {
        let env = FakeEnv::default();
        remove_with(&env, "%1", RemoveMode::WindowAndWorktree).expect("remove should succeed");
        let calls = env.calls();
        let position = |prefix: &str| {
            calls
                .iter()
                .position(|c| c.starts_with(prefix))
                .unwrap_or_else(|| panic!("{prefix} not called: {calls:?}"))
        };
        assert!(
            position("worktree_remove") < position("branch_delete")
                && position("branch_delete") < position("kill_window"),
            "git cleanup must precede kill_window: {calls:?}"
        );
    }

    #[test]
    fn remove_closes_only_the_window_for_a_plain_folder_agent() {
        let env = FakeEnv {
            display_output: Some("1\n/r/app\n\n\n@1\n".into()),
            ..FakeEnv::default()
        };
        remove_with(&env, "%1", RemoveMode::WindowAndWorktree).expect("remove should succeed");
        assert_eq!(env.calls(), vec!["kill_window(@1)".to_string()]);
    }

    #[test]
    fn remove_does_not_kill_window_when_worktree_remove_fails() {
        let env = FakeEnv {
            fail_worktree_remove: true,
            ..FakeEnv::default()
        };
        let err =
            remove_with(&env, "%1", RemoveMode::WindowAndWorktree).expect_err("remove must fail");
        assert!(err.contains("worktree_remove failed"), "error: {err}");
        assert!(!has_call(&env.calls(), "kill_window("));
    }

    #[test]
    fn remove_skips_git_step_when_worktree_already_gone() {
        let env = FakeEnv {
            worktree_path_already_gone: Some(true),
            fail_worktree_remove: true,
            ..FakeEnv::default()
        };
        remove_with(&env, "%1", RemoveMode::WindowAndWorktree).expect("remove should succeed");
        let calls = env.calls();
        assert!(!has_call(&calls, "worktree_remove("));
        assert!(has_call(&calls, "kill_window(@1)"));
    }

    #[test]
    fn remove_window_only_skips_all_git_steps() {
        let env = FakeEnv::default();
        remove_with(&env, "%1", RemoveMode::WindowOnly).expect("remove should succeed");
        assert_eq!(env.calls(), vec!["kill_window(@1)".to_string()]);
    }

    #[test]
    fn remove_rejects_pane_missing_spawned_marker() {
        let env = FakeEnv {
            display_output: Some("\n\n\n\n@1\n".into()),
            ..FakeEnv::default()
        };
        let err = remove_with(&env, "%1", RemoveMode::WindowOnly).expect_err("must fail");
        assert!(err.contains("not created by sidebar spawn"), "error: {err}");
        assert!(!has_call(&env.calls(), "kill_window("));
    }

    #[test]
    fn remove_rejects_worktree_pane_with_empty_branch_marker() {
        let env = FakeEnv {
            display_output: Some("1\n/r\n/r/.worktrees/task\n\n@1\n".into()),
            ..FakeEnv::default()
        };
        let err = remove_with(&env, "%1", RemoveMode::WindowAndWorktree).expect_err("must fail");
        assert!(err.contains("branch is unset"), "error: {err}");
        assert!(env.calls().is_empty());
    }

    #[test]
    fn remove_does_not_kill_window_when_branch_delete_fails() {
        let env = FakeEnv {
            fail_branch_delete: true,
            ..FakeEnv::default()
        };
        let err = remove_with(&env, "%1", RemoveMode::WindowAndWorktree).expect_err("must fail");
        assert!(err.contains("branch_delete failed"), "error: {err}");
        assert!(!has_call(&env.calls(), "kill_window("));
    }

    #[test]
    fn remove_skips_branch_delete_when_branch_already_gone() {
        let env = FakeEnv {
            branch_already_gone: Some(true),
            ..FakeEnv::default()
        };
        remove_with(&env, "%1", RemoveMode::WindowAndWorktree).expect("remove should succeed");
        assert!(!has_call(&env.calls(), "branch_delete("));
    }
}
