//! Compatibility for already-running Command Code processes whose mod did not
//! load. Prefer hooks as soon as they supply a session ID. Never restart agents.
use super::AppState;
use crate::event::{AgentEvent, EventAdapter};
use crate::tmux::{self, AgentType, PaneStatus, SessionInfo};

fn screen_status(screen: &str) -> Option<PaneStatus> {
    let footer: Vec<_> = screen.lines().rev().take(16).map(str::trim).collect();
    if footer
        .iter()
        .any(|line| line.contains("esc to interrupt") && line.contains('•'))
    {
        return Some(PaneStatus::Running);
    }
    if footer.iter().any(|line| line.starts_with('❯'))
        && footer.iter().any(|line| line.contains("? for shortcuts"))
    {
        return Some(PaneStatus::Idle);
    }
    None
}

impl AppState {
    pub(super) fn refresh_unhooked_commandcode(&mut self, sessions: &mut [SessionInfo]) {
        for pane in sessions
            .iter_mut()
            .flat_map(|s| &mut s.windows)
            .flat_map(|w| &mut w.panes)
        {
            if pane.agent != AgentType::CommandCode || pane.session_id.is_some() {
                continue;
            }
            let Ok(screen) = tmux::run_tmux_capture(&["capture-pane", "-p", "-t", &pane.pane_id])
            else {
                continue;
            };
            let Some(status) = screen_status(&screen) else {
                continue;
            };
            let runtime = self.pane_state_mut(&pane.pane_id);
            let previous = runtime.commandcode_screen_status.clone();
            if status == PaneStatus::Idle && previous == Some(PaneStatus::Running) {
                runtime.commandcode_idle_ticks += 1;
                if runtime.commandcode_idle_ticks < 2 {
                    continue;
                }
            } else {
                runtime.commandcode_idle_ticks = 0;
            }
            runtime.commandcode_screen_status = Some(status.clone());
            if previous.as_ref() != Some(&status) {
                let event_name = if status == PaneStatus::Running {
                    Some("user-prompt-submit")
                } else if previous == Some(PaneStatus::Running) {
                    Some("stop")
                } else {
                    None
                };
                if let Some(name) = event_name {
                    let event: Option<AgentEvent> = crate::adapter::commandcode::CommandCodeAdapter
                        .parse(name, &serde_json::json!({"cwd":pane.path}));
                    if let Some(event) = event {
                        crate::cli::handle_event(&pane.pane_id, "commandcode", event);
                    }
                }
                if status == PaneStatus::Running {
                    pane.started_at = Some(self.now);
                } else {
                    pane.started_at = None;
                }
            }
            pane.status = status;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn live_chrome_distinguishes_work_from_an_input_prompt() {
        assert_eq!(
            screen_status(
                " ☆ Reviewing…  esc to interrupt • 1m 19s • ↓ 5.7k\n❯ Ask your question...\n? for shortcuts · taste on"
            ),
            Some(PaneStatus::Running)
        );
        assert_eq!(
            screen_status("❯ Ask your question...\n? for shortcuts · taste on"),
            Some(PaneStatus::Idle)
        );
        assert_eq!(screen_status("a tool printed a prompt"), None);
    }
}
