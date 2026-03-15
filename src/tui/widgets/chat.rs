use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use crate::tui::app::{AppState, ChatMessage, MessageRole};

use super::tool_call::render_tool_call_line;

/// Render the scrollable chat message area.
pub fn render_chat_area(frame: &mut Frame, area: Rect, state: &AppState) {
    let lines = build_chat_lines(state);

    let total_lines = lines.len();
    let visible_height = area.height.saturating_sub(2) as usize; // minus borders

    // Clamp scroll offset
    let max_scroll = total_lines.saturating_sub(visible_height);
    let scroll_offset = state.scroll_offset.min(max_scroll);

    // Scroll from bottom: offset 0 = bottom, higher = more history
    let scroll_from_top = max_scroll.saturating_sub(scroll_offset);

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" remix-agent "),
        )
        .wrap(Wrap { trim: false })
        .scroll((scroll_from_top as u16, 0));

    frame.render_widget(paragraph, area);
}

/// Build all lines for the chat area from messages.
fn build_chat_lines(state: &AppState) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();

    for (i, msg) in state.messages.iter().enumerate() {
        if i > 0 {
            lines.push(Line::from(""));
        }
        build_message_lines(msg, &mut lines);
    }

    // If we have thinking text and it's visible, add it
    if state.thinking_visible && !state.thinking_text.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            " --- Thinking ---",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        )]));
        for line_text in state.thinking_text.lines() {
            lines.push(Line::from(Span::styled(
                format!(" {line_text}"),
                Style::default().fg(Color::DarkGray),
            )));
        }
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            " Welcome to remix-agent. Type a message to begin.",
            Style::default().fg(Color::DarkGray),
        )));
    }

    lines
}

/// Build display lines for a single chat message.
fn build_message_lines(msg: &ChatMessage, lines: &mut Vec<Line<'static>>) {
    let (prefix, style) = role_display(&msg.role);

    // 1-space left padding prefix
    let padded_prefix = format!(" {prefix}");

    // First line: role prefix + first line of text
    let text_lines: Vec<&str> = if msg.text.is_empty() {
        vec![]
    } else {
        msg.text.lines().collect()
    };

    if text_lines.is_empty() && msg.tool_calls.is_empty() {
        lines.push(Line::from(vec![Span::styled(padded_prefix.clone(), style)]));
    } else if let Some(first) = text_lines.first() {
        lines.push(Line::from(vec![
            Span::styled(padded_prefix.clone(), style),
            Span::raw(first.to_string()),
        ]));
        for text_line in text_lines.iter().skip(1) {
            lines.push(Line::from(Span::raw(format!(
                "{}{}",
                " ".repeat(padded_prefix.len()),
                text_line
            ))));
        }
    }

    // Tool calls (with 1-space left padding)
    for tc in &msg.tool_calls {
        let mut line = render_tool_call_line(tc);
        line.spans.insert(0, Span::raw(" ".to_string()));
        lines.push(line);
    }
}

/// Get the display prefix and style for a message role.
fn role_display(role: &MessageRole) -> (&'static str, Style) {
    match role {
        MessageRole::User => (
            "You: ",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        MessageRole::Assistant => (
            "Agent: ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        MessageRole::System => (
            "System: ",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{AppState, ChatMessage, MessageRole, ToolCallDisplay, ToolCallStatus};
    use ratatui::{backend::TestBackend, Terminal};
    use serde_json::json;

    fn create_terminal() -> Terminal<TestBackend> {
        let backend = TestBackend::new(80, 20);
        Terminal::new(backend).unwrap()
    }

    #[test]
    fn test_empty_state_shows_welcome() {
        let state = AppState::new();
        let lines = build_chat_lines(&state);
        assert_eq!(lines.len(), 1);
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("Welcome"));
    }

    #[test]
    fn test_user_message_lines() {
        let mut state = AppState::new();
        state.messages.push(ChatMessage {
            role: MessageRole::User,
            text: "Hello".to_string(),
            tool_calls: Vec::new(),
        });
        let lines = build_chat_lines(&state);
        assert!(!lines.is_empty());
        let first_text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(first_text.contains("You:"));
        assert!(first_text.contains("Hello"));
    }

    #[test]
    fn test_assistant_message_lines() {
        let mut state = AppState::new();
        state.messages.push(ChatMessage {
            role: MessageRole::Assistant,
            text: "Hi there!".to_string(),
            tool_calls: Vec::new(),
        });
        let lines = build_chat_lines(&state);
        let first_text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(first_text.contains("Agent:"));
        assert!(first_text.contains("Hi there!"));
    }

    #[test]
    fn test_system_message_lines() {
        let mut state = AppState::new();
        state.messages.push(ChatMessage {
            role: MessageRole::System,
            text: "Error occurred".to_string(),
            tool_calls: Vec::new(),
        });
        let lines = build_chat_lines(&state);
        let first_text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(first_text.contains("System:"));
    }

    #[test]
    fn test_multiline_message() {
        let mut state = AppState::new();
        state.messages.push(ChatMessage {
            role: MessageRole::User,
            text: "line1\nline2\nline3".to_string(),
            tool_calls: Vec::new(),
        });
        let lines = build_chat_lines(&state);
        assert!(lines.len() >= 3);
    }

    #[test]
    fn test_message_with_tool_calls() {
        let mut state = AppState::new();
        state.messages.push(ChatMessage {
            role: MessageRole::Assistant,
            text: "Let me check.".to_string(),
            tool_calls: vec![ToolCallDisplay {
                id: "tc1".to_string(),
                name: "read_file".to_string(),
                input: json!({"path": "/tmp/test"}),
                output: Some(json!("contents")),
                duration_ms: Some(100),
                collapsed: true,
                status: ToolCallStatus::Success,
            }],
        });
        let lines = build_chat_lines(&state);
        // Should have message line + tool call line
        assert!(lines.len() >= 2);
    }

    #[test]
    fn test_thinking_visible() {
        let mut state = AppState::new();
        state.thinking_visible = true;
        state.thinking_text = "I need to think about this.\nLet me consider.".to_string();
        state.messages.push(ChatMessage {
            role: MessageRole::Assistant,
            text: "Processing...".to_string(),
            tool_calls: Vec::new(),
        });
        let lines = build_chat_lines(&state);
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(all_text.contains("Thinking"));
        assert!(all_text.contains("I need to think"));
    }

    #[test]
    fn test_thinking_hidden() {
        let mut state = AppState::new();
        state.thinking_visible = false;
        state.thinking_text = "secret thoughts".to_string();
        state.messages.push(ChatMessage {
            role: MessageRole::Assistant,
            text: "Hello".to_string(),
            tool_calls: Vec::new(),
        });
        let lines = build_chat_lines(&state);
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(!all_text.contains("Thinking"));
    }

    #[test]
    fn test_multiple_messages_separator() {
        let mut state = AppState::new();
        state.messages.push(ChatMessage {
            role: MessageRole::User,
            text: "First".to_string(),
            tool_calls: Vec::new(),
        });
        state.messages.push(ChatMessage {
            role: MessageRole::Assistant,
            text: "Second".to_string(),
            tool_calls: Vec::new(),
        });
        let lines = build_chat_lines(&state);
        // Should have separator between messages
        let has_empty = lines.iter().any(|l| l.spans.is_empty());
        assert!(has_empty);
    }

    #[test]
    fn test_render_chat_area_no_panic() {
        let mut terminal = create_terminal();
        let state = AppState::new();
        terminal
            .draw(|frame| {
                render_chat_area(frame, frame.area(), &state);
            })
            .unwrap();
    }

    #[test]
    fn test_empty_text_with_tool_calls() {
        let mut state = AppState::new();
        state.messages.push(ChatMessage {
            role: MessageRole::Assistant,
            text: String::new(),
            tool_calls: vec![ToolCallDisplay {
                id: "tc1".to_string(),
                name: "bash".to_string(),
                input: json!({"command": "ls"}),
                output: None,
                duration_ms: None,
                collapsed: true,
                status: ToolCallStatus::Running,
            }],
        });
        let lines = build_chat_lines(&state);
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_role_display_styles() {
        let (prefix, style) = role_display(&MessageRole::User);
        assert_eq!(prefix, "You: ");
        assert_eq!(style.fg, Some(Color::Green));

        let (prefix, style) = role_display(&MessageRole::Assistant);
        assert_eq!(prefix, "Agent: ");
        assert_eq!(style.fg, Some(Color::Cyan));

        let (prefix, style) = role_display(&MessageRole::System);
        assert_eq!(prefix, "System: ");
        assert_eq!(style.fg, Some(Color::Yellow));
    }

    #[test]
    fn test_message_content_has_left_padding() {
        let mut state = AppState::new();
        state.messages.push(ChatMessage {
            role: MessageRole::User,
            text: "Hello".to_string(),
            tool_calls: Vec::new(),
        });
        let lines = build_chat_lines(&state);
        let first_text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        // Should have leading space for padding
        assert!(first_text.starts_with(' '));
    }
}
