//! `spawn` subcommand — open a new tmux window running an agent in a folder.

use std::path::PathBuf;

use crate::tmux;
use crate::worktree::{self, SpawnRequest};

pub fn cmd_spawn(args: &[String]) -> i32 {
    let pane = std::env::var("TMUX_PANE").unwrap_or_default();
    if pane.is_empty() {
        eprintln!("error: TMUX_PANE is not set; spawn must be run from inside tmux");
        return 1;
    }
    let Some(cwd) = tmux::get_pane_path(&pane).filter(|s| !s.is_empty()) else {
        eprintln!("error: could not resolve current pane path");
        return 1;
    };
    let base = PathBuf::from(&cwd);
    let directory = match worktree::resolve_directory(&args.join(" "), &base) {
        Ok(directory) => directory,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };
    let Some(session) = tmux::pane_session_name(&pane) else {
        eprintln!("error: could not resolve current tmux session");
        return 1;
    };

    let opts = tmux::get_all_global_options();
    let agent = opts
        .get(worktree::AGENT_OPTION)
        .filter(|s| !s.is_empty())
        .cloned()
        .unwrap_or_else(|| worktree::DEFAULT_AGENT.into());

    let req = SpawnRequest {
        repo_root: base,
        directory: directory.clone(),
        session,
        agent,
        mode: worktree::DEFAULT_MODE.into(),
    };
    match worktree::spawn(&req) {
        Ok(_) => {
            println!("started agent in {}", directory.display());
            0
        }
        Err(e) => {
            eprintln!("spawn failed: {e}");
            1
        }
    }
}
