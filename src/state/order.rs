use super::AppState;

impl AppState {
    pub(super) fn apply_agent_order(&mut self) {
        if self.global.agent_order.is_empty() {
            return;
        }
        let ranks: std::collections::HashMap<_, _> = self
            .global
            .agent_order
            .iter()
            .enumerate()
            .map(|(index, id)| (id.as_str(), index))
            .collect();
        for group in &mut self.repo_groups {
            group.panes.sort_by_key(|(pane, _)| {
                ranks
                    .get(pane.pane_id.as_str())
                    .copied()
                    .unwrap_or(usize::MAX)
            });
        }
    }

    /// Move within the folder without changing the visible tmux pane.
    pub fn move_agent(&mut self, pane_id: &str, direction: isize) {
        let filter = self.global.status_filter;
        let Some(group) = self
            .repo_groups
            .iter_mut()
            .find(|group| group.panes.iter().any(|(pane, _)| pane.pane_id == pane_id))
        else {
            return;
        };
        let visible: Vec<usize> = group
            .panes
            .iter()
            .enumerate()
            .filter(|(_, (pane, _))| filter.matches(&pane.status))
            .map(|(i, _)| i)
            .collect();
        let Some(position) = visible
            .iter()
            .position(|i| group.panes[*i].0.pane_id == pane_id)
        else {
            return;
        };
        let next = position as isize + direction.signum();
        if next < 0 || next as usize >= visible.len() {
            return;
        }
        group.panes.swap(visible[position], visible[next as usize]);
        self.global.agent_order = self
            .repo_groups
            .iter()
            .flat_map(|g| g.panes.iter().map(|(pane, _)| pane.pane_id.clone()))
            .collect();
        self.global.save_agent_order();
        self.rebuild_row_targets();
        self.sync_selection_to_pane(pane_id);
        self.global.queue_cursor_save();
    }

    pub fn move_selected_agent(&mut self, direction: isize) {
        if let Some(id) = self.selected_pane().map(|pane| pane.pane_id.clone()) {
            self.move_agent(&id, direction);
        }
    }
}
