use agents_tmux::state::AppState;
use agents_tmux::tmux::{AgentType, PaneInfo, PaneStatus, SessionInfo, WorktreeMetadata};
use agents_tmux::ui;
#[allow(unused_imports)]
use ratatui::style::{Color, Modifier};
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};

pub const FIXED_NOW: u64 = 1_700_000_000;

/// Convert buffer to a human-readable UI snapshot WITHOUT style annotations.
/// This keeps the visible UI structure but drops empty spacer rows so snapshots
/// read more like a compact screenshot than a full terminal-sized cell dump.
pub fn buffer_to_string(buf: &Buffer) -> String {
    let area = buf.area;
    let mut lines = Vec::new();
    for y in area.y..area.y + area.height {
        let mut line = String::new();
        let mut has_content = false;
        let mut has_border_cap = false;

        for x in area.x..area.x + area.width {
            let cell = &buf[(x, y)];
            let symbol = cell.symbol();
            if symbol != " " {
                if !matches!(symbol, "│" | "╭" | "╮" | "╰" | "╯") {
                    has_content = true;
                }
                if matches!(symbol, "╭" | "╮" | "╰" | "╯") {
                    has_border_cap = true;
                }
            }
            line.push_str(symbol);
        }

        if has_content || has_border_cap {
            lines.push(line.trim_end().to_string());
        }
    }
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

/// Convert buffer to string WITH style annotations for color/modifier verification
/// Format: each cell's symbol is followed by style info if non-default
/// e.g., "○[fg:250]" or " [rev]" or "c[fg:174,bold]"
pub fn buffer_to_styled_string(buf: &Buffer) -> String {
    let area = buf.area;
    let mut lines = Vec::new();
    for y in area.y..area.y + area.height {
        let mut line = String::new();
        for x in area.x..area.x + area.width {
            let cell = &buf[(x, y)];
            line.push_str(cell.symbol());

            let mut attrs = Vec::new();
            if let Color::Indexed(n) = cell.fg {
                attrs.push(format!("fg:{n}"));
            }
            if let Color::Indexed(n) = cell.bg {
                attrs.push(format!("bg:{n}"));
            }
            if let Color::Indexed(n) = cell.underline_color {
                attrs.push(format!("ul:{n}"));
            }
            if cell.modifier.contains(Modifier::BOLD) {
                attrs.push("bold".into());
            }
            if cell.modifier.contains(Modifier::REVERSED) {
                attrs.push("rev".into());
            }
            if cell.modifier.contains(Modifier::UNDERLINED) {
                attrs.push("underline".into());
            }
            if !attrs.is_empty() {
                line.push_str(&format!("[{}]", attrs.join(",")));
            }
        }
        lines.push(line.trim_end().to_string());
    }
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

pub fn render_to_string(state: &mut AppState, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| ui::draw(frame, state)).unwrap();
    let buf = terminal.backend().buffer().clone();
    buffer_to_string(&buf)
}

pub fn render_to_styled_string(state: &mut AppState, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| ui::draw(frame, state)).unwrap();
    let buf = terminal.backend().buffer().clone();
    buffer_to_styled_string(&buf)
}

pub fn make_pane(agent: AgentType, status: PaneStatus) -> PaneInfo {
    PaneInfo {
        pane_id: "%1".into(),
        pane_active: true,
        status,
        attention: false,
        agent,
        path: "/home/user/project".into(),
        current_command: String::new(),
        prompt: String::new(),
        prompt_is_response: false,
        started_at: None,
        wait_reason: String::new(),
        permission_mode: agents_tmux::tmux::PermissionMode::Default,
        subagents: vec![],
        pane_pid: None,
        worktree: WorktreeMetadata::default(),
        session_id: None,
        session_name: String::new(),
        sidebar_spawned: false,
        bg_shell_cmd: None,
    }
}

pub fn make_repo_group(name: &str, panes: Vec<PaneInfo>) -> agents_tmux::group::RepoGroup {
    agents_tmux::group::RepoGroup {
        name: name.into(),
        has_focus: true,
        panes: panes
            .into_iter()
            .map(|p| (p, agents_tmux::group::PaneGitInfo::default()))
            .collect(),
    }
}

/// Build a baseline `AppState` for UI/render tests.
///
/// The `_sessions` argument is kept so the call sites can document the
/// intended pane topology, but the field itself was removed from
/// `AppState`. Callers that need a populated tree should set
/// `state.repo_groups` directly (which they already do).
pub fn make_state(_sessions: Vec<SessionInfo>) -> AppState {
    let mut state = AppState::new("%99".into());
    state.now = FIXED_NOW;
    state.focus_state.sidebar_focused = true;
    state.focus_state.focused_pane_id = Some("%1".into());
    state.notices.missing_hook_groups = vec![agents_tmux::state::NoticesMissingHookGroup {
        agent: "claude".into(),
        hooks: vec!["Stop".into()],
    }];
    state
}

pub fn assert_right_border_intact(output: &str) {
    for (i, line) in output.lines().enumerate() {
        if line.starts_with('│') || line.starts_with('╭') || line.starts_with('╰') {
            let trimmed = line.trim_end();
            assert!(
                trimmed.ends_with('│') || trimmed.ends_with('╮') || trimmed.ends_with('╯'),
                "Line {} missing right border: {:?}",
                i + 1,
                trimmed,
            );
        }
    }
}

// ─── Usage tab fixtures ─────────────────────────────────────────────

/// Key `UsageState::target` derives for the default test pane (`%1`, Claude,
/// no session id).
pub const USAGE_KEY: &str = "%1|Claude|";

/// A Claude snapshot with a nearly exhausted session window and a healthy
/// weekly one, used by the Usage tab snapshots.
pub fn claude_usage() -> agents_tmux::usage::UsageSnapshot {
    agents_tmux::usage::UsageSnapshot {
        key: USAGE_KEY.into(),
        title: "Claude Code".into(),
        plan: Some("max".into()),
        windows: vec![
            agents_tmux::usage::UsageWindow {
                label: "5h".into(),
                percent: 91.0,
                resets_at: Some(FIXED_NOW + 2 * 3600),
            },
            agents_tmux::usage::UsageWindow {
                label: "1w".into(),
                percent: 51.0,
                resets_at: Some(FIXED_NOW + 3 * 86_400),
            },
        ],
        extras: vec!["Extra $22.24 / $20.00".into()],
        fetched_at: FIXED_NOW,
        ..agents_tmux::usage::UsageSnapshot::default()
    }
}

/// An OpenCode snapshot: local session spend plus the provider fallback line.
pub fn opencode_usage() -> agents_tmux::usage::UsageSnapshot {
    agents_tmux::usage::UsageSnapshot {
        key: USAGE_KEY.into(),
        title: "OpenCode".into(),
        local: vec![
            ("Using".into(), "commandcode".into()),
            ("Model".into(), "deepseek-v4.1-flash".into()),
            ("Session".into(), "$15.22 · 1.2M tok".into()),
            ("Today".into(), "$20.50".into()),
        ],
        message: Some("No limits for commandcode".into()),
        fetched_at: FIXED_NOW,
        ..agents_tmux::usage::UsageSnapshot::default()
    }
}

/// Install a snapshot and mark it as the requested key so the renderer
/// treats it as the active agent's data.
pub fn attach_usage(state: &mut AppState, mut snapshot: agents_tmux::usage::UsageSnapshot) {
    // Bind the fixture to whichever agent the state currently focuses so
    // `UsageState::current` accepts it.
    let key = agents_tmux::state::UsageState::target(state)
        .map(|target| target.key)
        .unwrap_or_default();
    snapshot.key = key.clone();
    state.usage.requested_key = Some(key);
    state.usage.snapshot = Some(snapshot);
}
