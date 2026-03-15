use ratatui::{
    layout::{Constraint, Direction, Layout},
    Frame,
};

use super::app::AppState;
use super::widgets::{
    chat::render_chat_area, input_bar::render_input_bar, permission::render_permission_overlay,
    status_bar::render_status_bar,
};

/// Main render function that draws the entire TUI layout.
pub fn render(frame: &mut Frame, state: &AppState) {
    let area = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),    // Chat area (takes remaining space)
            Constraint::Length(1), // Status bar
            Constraint::Length(3), // Input bar
        ])
        .split(area);

    render_chat_area(frame, chunks[0], state);
    render_status_bar(frame, chunks[1], state);
    render_input_bar(frame, chunks[2], state);

    // Permission overlay renders on top of everything
    if state.permission_request.is_some() {
        render_permission_overlay(frame, area, state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    fn create_test_terminal() -> Terminal<TestBackend> {
        let backend = TestBackend::new(80, 24);
        Terminal::new(backend).unwrap()
    }

    #[test]
    fn test_render_empty_state() {
        let mut terminal = create_test_terminal();
        let state = AppState::new();
        terminal
            .draw(|frame| {
                render(frame, &state);
            })
            .unwrap();
        // No panic = success for basic rendering
    }

    #[test]
    fn test_render_with_messages() {
        let mut terminal = create_test_terminal();
        let mut state = AppState::new();
        state.messages.push(super::super::app::ChatMessage {
            role: super::super::app::MessageRole::User,
            text: "Hello there".to_string(),
            tool_calls: Vec::new(),
        });
        state.messages.push(super::super::app::ChatMessage {
            role: super::super::app::MessageRole::Assistant,
            text: "Hi! How can I help?".to_string(),
            tool_calls: Vec::new(),
        });
        terminal
            .draw(|frame| {
                render(frame, &state);
            })
            .unwrap();
    }

    #[test]
    fn test_render_with_permission_overlay() {
        let mut terminal = create_test_terminal();
        let mut state = AppState::new();
        state.state = super::super::app::TuiState::AwaitingPermission;
        state.permission_request = Some(super::super::app::PermissionPrompt {
            tool_name: "bash".to_string(),
            tool_input: serde_json::json!({"command": "ls -la"}),
        });
        terminal
            .draw(|frame| {
                render(frame, &state);
            })
            .unwrap();
    }

    #[test]
    fn test_render_small_terminal() {
        let backend = TestBackend::new(20, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        let state = AppState::new();
        terminal
            .draw(|frame| {
                render(frame, &state);
            })
            .unwrap();
    }

    #[test]
    fn test_render_with_input() {
        let mut terminal = create_test_terminal();
        let mut state = AppState::new();
        state.input_buffer = "some typed text".to_string();
        state.input_cursor = 15;
        terminal
            .draw(|frame| {
                render(frame, &state);
            })
            .unwrap();
    }

    #[test]
    fn test_render_with_status() {
        let mut terminal = create_test_terminal();
        let mut state = AppState::new();
        state.status.input_tokens = 50_000;
        state.status.output_tokens = 10_000;
        state.status.cost_usd = 0.123;
        state.status.iteration = 3;
        state.status.max_iterations = 50;
        terminal
            .draw(|frame| {
                render(frame, &state);
            })
            .unwrap();
    }

    #[test]
    fn test_render_with_tool_calls() {
        let mut terminal = create_test_terminal();
        let mut state = AppState::new();
        state.messages.push(super::super::app::ChatMessage {
            role: super::super::app::MessageRole::Assistant,
            text: "Let me check.".to_string(),
            tool_calls: vec![
                super::super::app::ToolCallDisplay {
                    id: "tc1".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({"path": "/tmp/test.rs"}),
                    output: Some(serde_json::json!("file contents")),
                    duration_ms: Some(150),
                    collapsed: true,
                    status: super::super::app::ToolCallStatus::Success,
                },
                super::super::app::ToolCallDisplay {
                    id: "tc2".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({"command": "cargo test"}),
                    output: None,
                    duration_ms: None,
                    collapsed: true,
                    status: super::super::app::ToolCallStatus::Running,
                },
            ],
        });
        terminal
            .draw(|frame| {
                render(frame, &state);
            })
            .unwrap();
    }
}
