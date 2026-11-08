//! The mod emits the sidebar's normalized lifecycle vocabulary. Command Code
//! settings hooks alone cannot report prompts, permission waits or shutdown.
use serde_json::Value;

use crate::event::{AgentEvent, EventAdapter};
use crate::tmux::COMMANDCODE_AGENT;

pub struct CommandCodeAdapter;

impl EventAdapter for CommandCodeAdapter {
    fn parse(&self, name: &str, input: &Value) -> Option<AgentEvent> {
        let mut input = input.clone();
        if let Some(object) = input.as_object_mut() {
            let mode = match object
                .get("permission_mode")
                .and_then(Value::as_str)
                .unwrap_or("")
            {
                "auto-accept" | "accept-edits" => "acceptEdits",
                "bypass" | "yolo" => "bypassPermissions",
                "dont-ask" => "dontAsk",
                other => other,
            }
            .to_string();
            object.insert("permission_mode".into(), Value::String(mode));
            if let Some(raw) = object.get("tool_name").and_then(Value::as_str) {
                let tool = match raw {
                    "shell_command" => "Bash",
                    "read_file" => "Read",
                    "write_file" => "Write",
                    "edit_file" => "Edit",
                    "ask_user" | "ask_user_question" => "AskUserQuestion",
                    "agent" => "Agent",
                    other => other,
                }
                .to_string();
                object.insert("tool_name".into(), Value::String(tool));
            }
        }
        let mut event = super::claude::ClaudeAdapter.parse(name, &input)?;
        match &mut event {
            AgentEvent::SessionStart { agent, .. }
            | AgentEvent::UserPromptSubmit { agent, .. }
            | AgentEvent::Notification { agent, .. }
            | AgentEvent::Stop { agent, .. }
            | AgentEvent::StopFailure { agent, .. }
            | AgentEvent::PermissionDenied { agent, .. } => *agent = COMMANDCODE_AGENT.into(),
            _ => {}
        }
        Some(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lifecycle_keeps_commandcode_identity_and_normalizes_modes() {
        let event = CommandCodeAdapter
            .parse(
                "session-start",
                &json!({
                    "cwd":"/work", "session_id":"cmd-1", "permission_mode":"auto-accept"
                }),
            )
            .unwrap();
        assert!(
            matches!(event, AgentEvent::SessionStart {agent, permission_mode, session_id, ..}
            if agent == "commandcode" && permission_mode == "acceptEdits" && session_id.as_deref() == Some("cmd-1"))
        );
        for name in [
            "user-prompt-submit",
            "notification",
            "stop",
            "stop-failure",
            "permission-denied",
            "session-end",
        ] {
            assert!(
                CommandCodeAdapter.parse(name, &json!({})).is_some(),
                "{name}"
            );
        }
        assert!(
            CommandCodeAdapter
                .parse("not-an-event", &json!({}))
                .is_none()
        );
    }

    #[test]
    fn native_tool_names_use_shared_vocabulary() {
        for (raw, expected) in [
            ("shell_command", "Bash"),
            ("read_file", "Read"),
            ("write_file", "Write"),
            ("edit_file", "Edit"),
        ] {
            let event = CommandCodeAdapter
                .parse(
                    "activity-log",
                    &json!({"tool_name":raw,"tool_input":{"file_path":"a"}}),
                )
                .unwrap();
            assert!(
                matches!(event, AgentEvent::ActivityLog { tool_name, tool_input, .. }
                if tool_name == expected && tool_input["file_path"] == "a")
            );
        }
    }
}
