use ratatui::{
    style::Style,
    text::{Line, Span},
};

use super::row;
use crate::state::{AppState, Focus};
use crate::ui::text::truncate_to_width;

#[derive(Debug, Default)]
pub(super) struct CollectedRows {
    pub lines: Vec<Line<'static>>,
    pub line_to_row: Vec<Option<usize>>,
    pub pending_spawn: Vec<(usize, String, String)>,
    pub pending_remove: Vec<(usize, u16, String)>,
    pub pending_actions: Vec<(usize, String, usize)>,
}

pub(super) fn collect(state: &AppState, width: u16) -> CollectedRows {
    let width = width as usize;
    let theme = &state.theme;

    let mut collected = CollectedRows::default();
    let filter = state.global.status_filter;
    let mut first_group = true;
    let mut row_index: usize = 0;

    for group in &state.repo_groups {
        if !state.global.repo_filter.matches_group(&group.name) {
            continue;
        }
        let filtered_panes: Vec<_> = group
            .panes
            .iter()
            .filter(|(pane, _)| filter.matches(&pane.status))
            .collect();
        if filtered_panes.is_empty() {
            continue;
        }

        if !first_group {
            // Separate repo groups, but do not add a leading blank before
            // the first repo so the list starts immediately below the header.
            collected.lines.push(Line::from(""));
            collected.line_to_row.push(None);
        }
        first_group = false;

        let group_has_focused_pane = state
            .focus_state
            .focused_pane_id
            .as_ref()
            .is_some_and(|fid| group.panes.iter().any(|(p, _)| p.pane_id == *fid));

        // Creation has one entry point in the header, not a duplicate per folder.
        let title = &group.name;
        let title_color = if group_has_focused_pane {
            theme.accent
        } else {
            theme.text_active
        };
        let spans = vec![Span::styled(
            truncate_to_width(title, width),
            Style::default().fg(title_color),
        )];
        collected.lines.push(Line::from(spans));
        collected.line_to_row.push(None);

        for (pane, git_info) in filtered_panes.iter() {
            let is_active = state.focus_state.focused_pane_id.as_ref() == Some(&pane.pane_id);
            let navigating =
                state.focus_state.sidebar_focused && state.focus_state.focus == Focus::Panes;
            let is_selected = if navigating {
                row_index == state.global.selected_pane_row
            } else {
                state.persistent_agent_highlight && is_active
            };

            let pane_state = state.pane_state(&pane.pane_id);
            let ports = pane_state.map(|s| s.ports.as_slice());
            let task_progress = pane_state.and_then(|s| s.task_progress.as_ref());
            let card_start = collected.lines.len();
            let padding = Line::from(Span::raw(" ".repeat(width)));
            let status_line_idx = collected.lines.len();
            let mut pane_lines = row::render_pane_lines_with_ports(
                pane,
                git_info,
                ports,
                task_progress,
                is_selected,
                is_active,
                width,
                &state.icons,
                theme,
                state.spinner_frame,
                state.now,
            );
            let pane_line_count = pane_lines.len();
            if is_selected {
                for line in &mut pane_lines {
                    line.style = line.style.bg(theme.selection_bg);
                }
            }
            if row::has_agent_controls(is_selected, width) {
                // Expand into the padding, but never steal branch/detail clicks.
                let hit_height = if pane_line_count == 1 { 2 } else { 1 };
                collected
                    .pending_actions
                    .push((card_start, pane.pane_id.clone(), hit_height));
            }
            collected.lines.extend(pane_lines);
            for _ in 0..pane_line_count {
                collected.line_to_row.push(Some(row_index));
            }
            collected.lines.push(padding);
            collected.line_to_row.push(Some(row_index));

            // The branch row is always `status_line_idx + 1` when
            // `branch_ports_row` emits a line (which requires a
            // non-empty branch). Look up the exact column of the
            // trailing `×` from the row helper so the click target
            // lines up with the rendered glyph even when the branch
            // name truncates.
            if pane.sidebar_spawned
                && git_info.is_worktree
                && pane_line_count >= 2
                && let Some(x) =
                    row::sidebar_remove_marker_col(git_info, ports, true, width.saturating_sub(2))
            {
                collected
                    .pending_remove
                    .push((status_line_idx + 1, x, pane.pane_id.clone()));
            }

            row_index += 1;
        }
    }

    collected
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::group::{PaneGitInfo, RepoGroup};
    use crate::state::{AppState, StatusFilter};
    use crate::tmux::{AgentType, PaneInfo, PaneStatus, PermissionMode, WorktreeMetadata};

    fn make_pane(id: &str, status: PaneStatus) -> PaneInfo {
        PaneInfo {
            pane_id: id.into(),
            pane_active: false,
            status,
            attention: false,
            agent: AgentType::Claude,
            path: "/tmp/repo".into(),
            current_command: String::new(),
            prompt: String::new(),
            prompt_is_response: false,
            started_at: None,
            wait_reason: String::new(),
            permission_mode: PermissionMode::Default,
            subagents: vec![],
            pane_pid: None,
            worktree: WorktreeMetadata::default(),
            session_id: None,
            session_name: String::new(),
            sidebar_spawned: false,
            bg_shell_cmd: None,
        }
    }

    #[test]
    fn persistent_highlight_follows_visible_agent_without_sidebar_focus() {
        let mut state = AppState::new("%sidebar".into());
        state.persistent_agent_highlight = true;
        state.focus_state.sidebar_focused = false;
        state.focus_state.focused_pane_id = Some("%2".into());
        state.global.selected_pane_row = 0;
        state.repo_groups = vec![RepoGroup {
            name: "repo".into(),
            has_focus: true,
            panes: vec![
                (make_pane("%1", PaneStatus::Running), PaneGitInfo::default()),
                (make_pane("%2", PaneStatus::Running), PaneGitInfo::default()),
            ],
        }];
        let snapshot = collect(&state, 24)
            .lines
            .iter()
            .map(|line| {
                format!(
                    "{} [highlight={}]",
                    line.to_string().trim_end(),
                    line.spans
                        .iter()
                        .any(|span| span.style.bg == Some(state.theme.selection_bg))
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        insta::assert_snapshot!(snapshot, @"
        repo [highlight=false]
          ⠋ claude [highlight=false]
         [highlight=false]
        ┃ ⠋ claude        ↑ ↓ 󰆴 [highlight=true]
         [highlight=false]
        ");
    }

    #[test]
    fn collect_empty_repo_groups_produces_no_lines() {
        let state = AppState::new("%0".into());
        let collected = collect(&state, 40);
        assert!(collected.lines.is_empty());
        assert!(collected.line_to_row.is_empty());
        assert!(collected.pending_spawn.is_empty());
        assert!(collected.pending_remove.is_empty());
    }

    #[test]
    fn collect_skips_group_when_status_filter_excludes_all_panes() {
        let mut state = AppState::new("%0".into());
        // The group has only Running panes, so filter to Waiting to drop them all.
        state.global.status_filter = StatusFilter::Waiting;
        state.repo_groups = vec![RepoGroup {
            name: "repo".into(),
            has_focus: false,
            panes: vec![(make_pane("%1", PaneStatus::Running), PaneGitInfo::default())],
        }];
        let collected = collect(&state, 40);
        assert!(collected.lines.is_empty());
        assert!(collected.pending_spawn.is_empty());
    }

    #[test]
    fn git_folder_headers_do_not_duplicate_the_universal_spawn_button() {
        let mut state = AppState::new("%0".into());
        let git_info = PaneGitInfo {
            repo_root: Some("/tmp/repo".into()),
            branch: None,
            is_worktree: false,
            worktree_name: None,
        };
        state.repo_groups = vec![RepoGroup {
            name: "repo".into(),
            has_focus: false,
            panes: vec![(make_pane("%1", PaneStatus::Running), git_info)],
        }];
        let collected = collect(&state, 40);
        assert!(collected.pending_spawn.is_empty());
        // At least the header plus one pane row should have been pushed.
        assert!(!collected.lines.is_empty());
    }

    #[test]
    fn plain_folder_headers_do_not_duplicate_the_universal_spawn_button() {
        let mut state = AppState::new("%0".into());
        state.repo_groups = vec![RepoGroup {
            name: "raw-path".into(),
            has_focus: false,
            panes: vec![(make_pane("%1", PaneStatus::Running), PaneGitInfo::default())],
        }];
        let collected = collect(&state, 40);
        assert!(collected.pending_spawn.is_empty());
    }

    #[test]
    fn multiple_folders_do_not_add_duplicate_spawn_buttons() {
        let mut state = AppState::new("%0".into());
        let with_root = |root: &str, name: &str, pane_id: &str| RepoGroup {
            name: name.into(),
            has_focus: false,
            panes: vec![(
                make_pane(pane_id, PaneStatus::Running),
                PaneGitInfo {
                    repo_root: Some(root.into()),
                    branch: None,
                    is_worktree: false,
                    worktree_name: None,
                },
            )],
        };
        state.repo_groups = vec![
            with_root("/repo/a", "a", "%1"),
            with_root("/repo/b", "b", "%2"),
            with_root("/repo/c", "c", "%3"),
        ];
        let collected = collect(&state, 40);
        assert!(collected.pending_spawn.is_empty());
    }
}
