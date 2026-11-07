//! `new-agent` subcommand — the form for starting an agent in a folder. The
//! sidebar opens it in a tmux popup so it is centered over the whole window
//! rather than squeezed into the narrow sidebar pane.

use std::io;
use std::process::{Command, Stdio};
use std::time::Duration;

use crossterm::event::{self, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::{Terminal, backend::CrosstermBackend};

use super::setup::shell_quote;
use crate::state::AppState;
use crate::ui;

const POPUP_WIDTH: &str = "66";
const POPUP_HEIGHT: &str = "15";

/// Open the form as a tmux popup over `sidebar_pane`'s window. Fails when
/// tmux cannot show a popup (no client attached, tmux older than 3.2), so
/// the caller can fall back to the in-sidebar form.
pub fn launch(sidebar_pane: &str, repo_name: &str, repo_root: &str) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let command = format!(
        "{} new-agent --pane {} --name {} --root {}",
        shell_quote(&exe.to_string_lossy()),
        shell_quote(sidebar_pane),
        shell_quote(repo_name),
        shell_quote(repo_root),
    );
    let mut child = Command::new("tmux")
        .args([
            "display-popup",
            "-E",
            "-B",
            // Themes (e.g. dracula) give popups a solid background, and
            // `bg=default` would still inherit it. `terminal` is the
            // terminal's own colour, so a transparent terminal stays so.
            "-s",
            "bg=terminal,fg=terminal",
            "-S",
            "bg=terminal",
            "-t",
            sidebar_pane,
            "-w",
            POPUP_WIDTH,
            "-h",
            POPUP_HEIGHT,
            "-d",
            repo_root,
            &command,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    // `display-popup -E` blocks until the popup closes, so an exit within a
    // moment means tmux refused to show it.
    std::thread::sleep(Duration::from_millis(120));
    match child.try_wait() {
        Ok(Some(status)) if !status.success() => Err("tmux could not show the popup".into()),
        Ok(Some(_)) => Ok(()),
        _ => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            Ok(())
        }
    }
}

struct Flag {
    pane: String,
    name: String,
    root: String,
}

fn parse(args: &[String]) -> Option<Flag> {
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    Some(Flag {
        pane: value("--pane")?,
        name: value("--name")?,
        root: value("--root")?,
    })
}

pub(super) fn cmd_new_agent(args: &[String]) -> i32 {
    let Some(flag) = parse(args) else {
        eprintln!("usage: agents-tmux new-agent --pane <pane> --name <name> --root <folder>");
        return 2;
    };
    match run(flag) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("new-agent: {error}");
            1
        }
    }
}

fn run(flag: Flag) -> io::Result<()> {
    let mut state = AppState::new(flag.pane);
    state.theme = ui::colors::ColorTheme::from_tmux();
    state.open_spawn_input_for_repo(flag.name, flag.root, None);

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let result = (|| {
        let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
        while state.is_spawn_input_open() {
            terminal.draw(|frame| ui::panes::render_new_agent_form(frame, &mut state))?;
            if let Event::Key(key) = event::read()?
                && key.kind != KeyEventKind::Release
            {
                state.handle_spawn_input_key(key);
            }
        }
        Ok(())
    })();
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), LeaveAlternateScreen);
    result
}
