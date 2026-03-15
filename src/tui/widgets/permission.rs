use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::tui::app::AppState;

/// Render a centered permission prompt overlay.
pub fn render_permission_overlay(frame: &mut Frame, area: Rect, state: &AppState) {
    let prompt = match &state.permission_request {
        Some(p) => p,
        None => return,
    };

    let popup = centered_rect(60, 50, area);

    // Clear the area behind the popup
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Red))
        .title(" Permission Required ");

    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // Tool name
            Constraint::Min(3),    // Input details
            Constraint::Length(2), // Action buttons
        ])
        .split(inner);

    // Tool name
    let tool_line = Paragraph::new(Line::from(vec![
        Span::styled("Tool: ", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(prompt.tool_name.clone(), Style::default().fg(Color::Yellow)),
    ]));
    frame.render_widget(tool_line, chunks[0]);

    // Input details
    let input_text = format_tool_input(&prompt.tool_input, chunks[1].width as usize);
    let input_para = Paragraph::new(input_text)
        .style(Style::default().fg(Color::White))
        .wrap(Wrap { trim: false });
    frame.render_widget(input_para, chunks[1]);

    // Action buttons
    let buttons = Paragraph::new(Line::from(vec![
        Span::styled(
            " [y] ",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("Allow  "),
        Span::styled(
            " [n] ",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
        Span::raw("Deny  "),
        Span::styled(
            " [a] ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("Always Allow"),
    ]));
    frame.render_widget(buttons, chunks[2]);
}

/// Format tool input for display in the permission dialog.
fn format_tool_input(input: &serde_json::Value, max_width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    match input {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                let value_str = match value {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };

                lines.push(Line::from(vec![Span::styled(
                    format!("{key}: "),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )]));

                // Wrap long values
                for chunk in wrap_text(&value_str, max_width.saturating_sub(2)) {
                    lines.push(Line::from(Span::raw(format!("  {chunk}"))));
                }
            }
        }
        other => {
            let text = other.to_string();
            for chunk in wrap_text(&text, max_width) {
                lines.push(Line::from(Span::raw(chunk)));
            }
        }
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "(no input)",
            Style::default().fg(Color::DarkGray),
        )));
    }

    lines
}

/// Simple text wrapping at max_width.
fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![text.to_string()];
    }

    let mut result = Vec::new();
    for line in text.lines() {
        if line.len() <= max_width {
            result.push(line.to_string());
        } else {
            let mut remaining = line;
            while remaining.len() > max_width {
                let (chunk, rest) = remaining.split_at(max_width);
                result.push(chunk.to_string());
                remaining = rest;
            }
            if !remaining.is_empty() {
                result.push(remaining.to_string());
            }
        }
    }
    result
}

/// Create a centered rectangle with percentage-based sizing.
fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{PermissionPrompt, TuiState};
    use ratatui::{backend::TestBackend, Terminal};
    use serde_json::json;

    fn create_terminal() -> Terminal<TestBackend> {
        let backend = TestBackend::new(80, 24);
        Terminal::new(backend).unwrap()
    }

    #[test]
    fn test_centered_rect() {
        let area = Rect::new(0, 0, 100, 50);
        let result = centered_rect(60, 50, area);
        // Should be roughly centered
        assert!(result.x > 0);
        assert!(result.y > 0);
        assert!(result.width < 100);
        assert!(result.height < 50);
    }

    #[test]
    fn test_render_permission_no_request() {
        let mut terminal = create_terminal();
        let state = AppState::new();
        terminal
            .draw(|frame| {
                render_permission_overlay(frame, frame.area(), &state);
            })
            .unwrap();
        // No panic when permission_request is None
    }

    #[test]
    fn test_render_permission_with_request() {
        let mut terminal = create_terminal();
        let mut state = AppState::new();
        state.state = TuiState::AwaitingPermission;
        state.permission_request = Some(PermissionPrompt {
            tool_name: "bash".to_string(),
            tool_input: json!({"command": "rm -rf /tmp/test"}),
        });
        terminal
            .draw(|frame| {
                render_permission_overlay(frame, frame.area(), &state);
            })
            .unwrap();
    }

    #[test]
    fn test_format_tool_input_object() {
        let input = json!({"command": "ls -la", "cwd": "/tmp"});
        let lines = format_tool_input(&input, 40);
        assert!(!lines.is_empty());
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect::<Vec<_>>()
            .join("");
        assert!(all_text.contains("command:"));
    }

    #[test]
    fn test_format_tool_input_string() {
        let input = json!("simple string");
        let lines = format_tool_input(&input, 40);
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_format_tool_input_null() {
        let input = json!(null);
        let lines = format_tool_input(&input, 40);
        // null value formats as "null"
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_format_tool_input_empty_object() {
        let input = json!({});
        let lines = format_tool_input(&input, 40);
        assert!(!lines.is_empty());
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect::<Vec<_>>()
            .join("");
        assert!(all_text.contains("(no input)"));
    }

    #[test]
    fn test_wrap_text_short() {
        let result = wrap_text("hello", 10);
        assert_eq!(result, vec!["hello"]);
    }

    #[test]
    fn test_wrap_text_exact() {
        let result = wrap_text("hello", 5);
        assert_eq!(result, vec!["hello"]);
    }

    #[test]
    fn test_wrap_text_long() {
        let result = wrap_text("abcdefghij", 4);
        assert_eq!(result, vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn test_wrap_text_multiline() {
        let result = wrap_text("abc\ndef", 10);
        assert_eq!(result, vec!["abc", "def"]);
    }

    #[test]
    fn test_wrap_text_zero_width() {
        let result = wrap_text("hello", 0);
        assert_eq!(result, vec!["hello"]);
    }

    #[test]
    fn test_render_permission_with_diff_input() {
        let mut terminal = create_terminal();
        let mut state = AppState::new();
        state.state = TuiState::AwaitingPermission;
        state.permission_request = Some(PermissionPrompt {
            tool_name: "write_file".to_string(),
            tool_input: json!({
                "path": "/src/main.rs",
                "content": "fn main() {\n    println!(\"hello\");\n}"
            }),
        });
        terminal
            .draw(|frame| {
                render_permission_overlay(frame, frame.area(), &state);
            })
            .unwrap();
    }
}
