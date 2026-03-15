use crate::output::events::AgentEvent;

use super::app::{AppState, ChatMessage, MessageRole, ToolCallDisplay, ToolCallStatus, TuiState};

/// Process an AgentEvent and update the AppState accordingly.
pub fn handle_agent_event(event: AgentEvent, state: &mut AppState) {
    match event {
        AgentEvent::AgentStarted { .. } => {
            state.state = TuiState::Running;
        }
        AgentEvent::IterationStarted { iteration } => {
            state.status.iteration = iteration;
        }
        AgentEvent::TextDelta { text } => {
            let msg = state.ensure_assistant_message();
            msg.text.push_str(&text);
            // Auto-scroll to bottom when new text arrives
            state.scroll_offset = 0;
        }
        AgentEvent::ThinkingDelta { text } => {
            state.thinking_text.push_str(&text);
        }
        AgentEvent::ThinkingComplete { thinking } => {
            state.thinking_text = thinking;
        }
        AgentEvent::ToolUseStart { id, name, input } => {
            // If the current assistant message has text, finalize it and start
            // a new one for tool calls — this interleaves tool calls between
            // text chunks like Claude Code does.
            let needs_new = state
                .messages
                .last()
                .is_some_and(|m| m.role == MessageRole::Assistant && !m.text.is_empty());
            if needs_new {
                state.messages.push(ChatMessage {
                    role: MessageRole::Assistant,
                    text: String::new(),
                    tool_calls: Vec::new(),
                });
            }
            let msg = state.ensure_assistant_message();
            msg.tool_calls.push(ToolCallDisplay {
                id,
                name,
                input,
                output: None,
                duration_ms: None,
                collapsed: true,
                status: ToolCallStatus::Running,
            });
            state.scroll_offset = 0;
        }
        AgentEvent::ToolUseResult {
            id,
            name: _,
            output,
            duration_ms,
            is_error,
        } => {
            update_tool_call(state, &id, output, duration_ms, is_error);
            state.scroll_offset = 0;
        }
        AgentEvent::TokenUsage {
            input_tokens,
            output_tokens,
            total_cost_usd,
            ..
        } => {
            // input_tokens reflects the latest request's full context size
            // (all prior messages + new), so use it directly for context %
            state.status.input_tokens = input_tokens;
            // Accumulate output tokens across the session
            state.status.output_tokens += output_tokens;
            if let Some(cost) = total_cost_usd {
                state.status.cost_usd += cost;
            }
        }
        AgentEvent::AwaitingInput => {
            state.state = TuiState::Idle;
            state.thinking_text.clear();
        }
        AgentEvent::AgentCompleted { result, .. } => {
            state.state = TuiState::Idle;
            state.thinking_text.clear();
            if let Some(text) = result {
                if !text.is_empty() {
                    let msg = state.ensure_assistant_message();
                    if !msg.text.is_empty() && !msg.text.ends_with('\n') {
                        msg.text.push('\n');
                    }
                    msg.text.push_str(&text);
                }
            }
        }
        AgentEvent::AgentError { error } => {
            state.state = TuiState::Idle;
            state.messages.push(ChatMessage {
                role: MessageRole::System,
                text: format!("Error: {error}"),
                tool_calls: Vec::new(),
            });
            state.scroll_offset = 0;
        }
    }
}

/// Find a tool call by ID across all messages and update it with the result.
fn update_tool_call(
    state: &mut AppState,
    id: &str,
    output: serde_json::Value,
    duration_ms: u64,
    is_error: bool,
) {
    for msg in state.messages.iter_mut().rev() {
        for tc in msg.tool_calls.iter_mut().rev() {
            if tc.id == id {
                tc.output = Some(output);
                tc.duration_ms = Some(duration_ms);
                tc.status = if is_error {
                    ToolCallStatus::Error
                } else {
                    ToolCallStatus::Success
                };
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::result::AgentStatus;
    use serde_json::json;

    #[test]
    fn test_agent_started_sets_running() {
        let mut state = AppState::new();
        handle_agent_event(
            AgentEvent::AgentStarted {
                task: "test".to_string(),
                timestamp_ms: 0,
            },
            &mut state,
        );
        assert_eq!(state.state, TuiState::Running);
    }

    #[test]
    fn test_iteration_started_updates_counter() {
        let mut state = AppState::new();
        handle_agent_event(AgentEvent::IterationStarted { iteration: 3 }, &mut state);
        assert_eq!(state.status.iteration, 3);
    }

    #[test]
    fn test_text_delta_appends() {
        let mut state = AppState::new();
        handle_agent_event(
            AgentEvent::TextDelta {
                text: "Hello".to_string(),
            },
            &mut state,
        );
        handle_agent_event(
            AgentEvent::TextDelta {
                text: " world".to_string(),
            },
            &mut state,
        );
        assert_eq!(state.messages.len(), 1);
        assert_eq!(state.messages[0].text, "Hello world");
        assert_eq!(state.messages[0].role, MessageRole::Assistant);
    }

    #[test]
    fn test_text_delta_resets_scroll() {
        let mut state = AppState::new();
        state.scroll_offset = 5;
        handle_agent_event(
            AgentEvent::TextDelta {
                text: "hi".to_string(),
            },
            &mut state,
        );
        assert_eq!(state.scroll_offset, 0);
    }

    #[test]
    fn test_thinking_delta_appends() {
        let mut state = AppState::new();
        handle_agent_event(
            AgentEvent::ThinkingDelta {
                text: "Let me ".to_string(),
            },
            &mut state,
        );
        handle_agent_event(
            AgentEvent::ThinkingDelta {
                text: "think...".to_string(),
            },
            &mut state,
        );
        assert_eq!(state.thinking_text, "Let me think...");
    }

    #[test]
    fn test_thinking_complete_replaces() {
        let mut state = AppState::new();
        state.thinking_text = "partial".to_string();
        handle_agent_event(
            AgentEvent::ThinkingComplete {
                thinking: "full thought".to_string(),
            },
            &mut state,
        );
        assert_eq!(state.thinking_text, "full thought");
    }

    #[test]
    fn test_tool_use_start_adds_entry() {
        let mut state = AppState::new();
        handle_agent_event(
            AgentEvent::ToolUseStart {
                id: "tc1".to_string(),
                name: "read_file".to_string(),
                input: json!({"path": "/tmp/test"}),
            },
            &mut state,
        );
        assert_eq!(state.messages.len(), 1);
        assert_eq!(state.messages[0].tool_calls.len(), 1);
        let tc = &state.messages[0].tool_calls[0];
        assert_eq!(tc.id, "tc1");
        assert_eq!(tc.name, "read_file");
        assert_eq!(tc.status, ToolCallStatus::Running);
        assert!(tc.collapsed);
        assert!(tc.output.is_none());
    }

    #[test]
    fn test_tool_use_result_updates_entry() {
        let mut state = AppState::new();
        // First, start a tool call
        handle_agent_event(
            AgentEvent::ToolUseStart {
                id: "tc1".to_string(),
                name: "bash".to_string(),
                input: json!({"command": "ls"}),
            },
            &mut state,
        );
        // Then complete it
        handle_agent_event(
            AgentEvent::ToolUseResult {
                id: "tc1".to_string(),
                name: "bash".to_string(),
                output: json!("file1\nfile2"),
                duration_ms: 150,
                is_error: false,
            },
            &mut state,
        );
        let tc = &state.messages[0].tool_calls[0];
        assert_eq!(tc.status, ToolCallStatus::Success);
        assert_eq!(tc.output, Some(json!("file1\nfile2")));
        assert_eq!(tc.duration_ms, Some(150));
        assert!(!tc.is_error());
    }

    #[test]
    fn test_tool_use_result_error() {
        let mut state = AppState::new();
        handle_agent_event(
            AgentEvent::ToolUseStart {
                id: "tc1".to_string(),
                name: "bash".to_string(),
                input: json!({}),
            },
            &mut state,
        );
        handle_agent_event(
            AgentEvent::ToolUseResult {
                id: "tc1".to_string(),
                name: "bash".to_string(),
                output: json!("command not found"),
                duration_ms: 50,
                is_error: true,
            },
            &mut state,
        );
        let tc = &state.messages[0].tool_calls[0];
        assert_eq!(tc.status, ToolCallStatus::Error);
        assert!(tc.is_error());
    }

    #[test]
    fn test_tool_use_result_missing_id_no_panic() {
        let mut state = AppState::new();
        // Result for a tool call that doesn't exist — should not panic
        handle_agent_event(
            AgentEvent::ToolUseResult {
                id: "nonexistent".to_string(),
                name: "bash".to_string(),
                output: json!("ok"),
                duration_ms: 10,
                is_error: false,
            },
            &mut state,
        );
        // No crash, no tool calls added
        assert!(state.messages.is_empty());
    }

    #[test]
    fn test_token_usage_updates_status() {
        let mut state = AppState::new();
        handle_agent_event(
            AgentEvent::TokenUsage {
                input_tokens: 5000,
                output_tokens: 1000,
                cache_read_tokens: Some(200),
                cache_write_tokens: Some(100),
                total_cost_usd: Some(0.05),
            },
            &mut state,
        );
        assert_eq!(state.status.input_tokens, 5000);
        assert_eq!(state.status.output_tokens, 1000);
        assert!((state.status.cost_usd - 0.05).abs() < f64::EPSILON);
    }

    #[test]
    fn test_token_usage_no_cost() {
        let mut state = AppState::new();
        state.status.cost_usd = 0.03;
        handle_agent_event(
            AgentEvent::TokenUsage {
                input_tokens: 100,
                output_tokens: 50,
                cache_read_tokens: None,
                cache_write_tokens: None,
                total_cost_usd: None,
            },
            &mut state,
        );
        // Cost should remain unchanged when None
        assert!((state.status.cost_usd - 0.03).abs() < f64::EPSILON);
    }

    #[test]
    fn test_awaiting_input_switches_to_idle() {
        let mut state = AppState::new();
        state.state = TuiState::Running;
        state.thinking_text = "some thinking".to_string();
        handle_agent_event(AgentEvent::AwaitingInput, &mut state);
        assert_eq!(state.state, TuiState::Idle);
        assert!(state.thinking_text.is_empty());
    }

    #[test]
    fn test_agent_completed_switches_to_idle() {
        let mut state = AppState::new();
        state.state = TuiState::Running;
        state.thinking_text = "thinking".to_string();
        handle_agent_event(
            AgentEvent::AgentCompleted {
                status: AgentStatus::Success,
                result: Some("Done!".to_string()),
                total_duration_ms: 5000,
            },
            &mut state,
        );
        assert_eq!(state.state, TuiState::Idle);
        assert!(state.thinking_text.is_empty());
        // Should have created an assistant message with the result
        assert!(!state.messages.is_empty());
        assert!(state.messages.last().unwrap().text.contains("Done!"));
    }

    #[test]
    fn test_agent_completed_no_result() {
        let mut state = AppState::new();
        state.state = TuiState::Running;
        handle_agent_event(
            AgentEvent::AgentCompleted {
                status: AgentStatus::Success,
                result: None,
                total_duration_ms: 1000,
            },
            &mut state,
        );
        assert_eq!(state.state, TuiState::Idle);
    }

    #[test]
    fn test_agent_completed_empty_result() {
        let mut state = AppState::new();
        state.state = TuiState::Running;
        handle_agent_event(
            AgentEvent::AgentCompleted {
                status: AgentStatus::Success,
                result: Some(String::new()),
                total_duration_ms: 1000,
            },
            &mut state,
        );
        assert_eq!(state.state, TuiState::Idle);
        // Empty result should not create a message
        assert!(state.messages.is_empty());
    }

    #[test]
    fn test_agent_completed_appends_to_existing_message() {
        let mut state = AppState::new();
        state.state = TuiState::Running;
        // Add some text first
        handle_agent_event(
            AgentEvent::TextDelta {
                text: "Working on it".to_string(),
            },
            &mut state,
        );
        handle_agent_event(
            AgentEvent::AgentCompleted {
                status: AgentStatus::Success,
                result: Some("Final result".to_string()),
                total_duration_ms: 1000,
            },
            &mut state,
        );
        assert_eq!(state.messages.len(), 1);
        assert!(state.messages[0].text.contains("Working on it"));
        assert!(state.messages[0].text.contains("Final result"));
    }

    #[test]
    fn test_agent_error_adds_system_message() {
        let mut state = AppState::new();
        state.state = TuiState::Running;
        state.scroll_offset = 5;
        handle_agent_event(
            AgentEvent::AgentError {
                error: "connection lost".to_string(),
            },
            &mut state,
        );
        assert_eq!(state.state, TuiState::Idle);
        assert_eq!(state.messages.len(), 1);
        assert_eq!(state.messages[0].role, MessageRole::System);
        assert_eq!(state.messages[0].text, "Error: connection lost");
        assert_eq!(state.scroll_offset, 0);
    }

    #[test]
    fn test_multiple_tool_calls_in_sequence() {
        let mut state = AppState::new();
        handle_agent_event(
            AgentEvent::ToolUseStart {
                id: "tc1".to_string(),
                name: "read_file".to_string(),
                input: json!({"path": "a.rs"}),
            },
            &mut state,
        );
        handle_agent_event(
            AgentEvent::ToolUseResult {
                id: "tc1".to_string(),
                name: "read_file".to_string(),
                output: json!("contents"),
                duration_ms: 100,
                is_error: false,
            },
            &mut state,
        );
        handle_agent_event(
            AgentEvent::ToolUseStart {
                id: "tc2".to_string(),
                name: "write_file".to_string(),
                input: json!({"path": "b.rs"}),
            },
            &mut state,
        );
        assert_eq!(state.messages.len(), 1);
        assert_eq!(state.messages[0].tool_calls.len(), 2);
        assert_eq!(
            state.messages[0].tool_calls[0].status,
            ToolCallStatus::Success
        );
        assert_eq!(
            state.messages[0].tool_calls[1].status,
            ToolCallStatus::Running
        );
    }

    #[test]
    fn test_text_then_tool_split_messages() {
        let mut state = AppState::new();
        handle_agent_event(
            AgentEvent::TextDelta {
                text: "Let me read that file.".to_string(),
            },
            &mut state,
        );
        handle_agent_event(
            AgentEvent::ToolUseStart {
                id: "tc1".to_string(),
                name: "read_file".to_string(),
                input: json!({}),
            },
            &mut state,
        );
        // Tool call starts a new message after text, interleaving them
        assert_eq!(state.messages.len(), 2);
        assert_eq!(state.messages[0].text, "Let me read that file.");
        assert!(state.messages[0].tool_calls.is_empty());
        assert_eq!(state.messages[1].tool_calls.len(), 1);
    }
}
