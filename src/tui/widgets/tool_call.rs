use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

use crate::tui::app::{ToolCallDisplay, ToolCallStatus};

/// Render a single tool call as a Line with status icon, name, args, and duration.
pub fn render_tool_call_line(tc: &ToolCallDisplay) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();

    // Indent
    spans.push(Span::raw("  "));

    // Collapse indicator
    let arrow = if tc.collapsed { "\u{25b8}" } else { "\u{25be}" };
    spans.push(Span::raw(format!("{arrow} ")));

    // Status icon
    let (icon, icon_style) = status_icon(&tc.status);
    spans.push(Span::styled(format!("{icon} "), icon_style));

    // Tool name
    spans.push(Span::styled(
        tc.name.clone(),
        Style::default().fg(Color::Blue),
    ));

    // Truncated args
    let args_display = truncate_input(&tc.input, 40);
    if !args_display.is_empty() {
        spans.push(Span::styled(
            format!("({args_display})"),
            Style::default().fg(Color::DarkGray),
        ));
    }

    // Duration
    if let Some(ms) = tc.duration_ms {
        spans.push(Span::styled(
            format!(" [{ms}ms]"),
            Style::default().fg(Color::DarkGray),
        ));
    }

    Line::from(spans)
}

/// Get status icon and style for a tool call status.
fn status_icon(status: &ToolCallStatus) -> (&'static str, Style) {
    match status {
        ToolCallStatus::Running => ("\u{21bb}", Style::default().fg(Color::Yellow)), // ↻
        ToolCallStatus::Success => ("\u{2713}", Style::default().fg(Color::Green)),  // ✓
        ToolCallStatus::Error => ("\u{2717}", Style::default().fg(Color::Red)),      // ✗
    }
}

/// Truncate the tool input JSON to a readable summary.
fn truncate_input(input: &serde_json::Value, max_len: usize) -> String {
    match input {
        serde_json::Value::Object(map) if map.is_empty() => String::new(),
        serde_json::Value::Null => String::new(),
        _ => {
            let s = input.to_string();
            if s.len() <= max_len {
                s
            } else if max_len <= 3 {
                "...".to_string()
            } else {
                format!("{}...", &s[..max_len - 3])
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_running_tool_call() {
        let tc = ToolCallDisplay {
            id: "tc1".to_string(),
            name: "bash".to_string(),
            input: json!({"command": "ls"}),
            output: None,
            duration_ms: None,
            collapsed: true,
            status: ToolCallStatus::Running,
        };
        let line = render_tool_call_line(&tc);
        let text: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert!(text.contains("\u{25b8}")); // collapsed arrow
        assert!(text.contains("\u{21bb}")); // running icon
        assert!(text.contains("bash"));
    }

    #[test]
    fn test_success_tool_call() {
        let tc = ToolCallDisplay {
            id: "tc1".to_string(),
            name: "read_file".to_string(),
            input: json!({"path": "/tmp/test"}),
            output: Some(json!("contents")),
            duration_ms: Some(150),
            collapsed: true,
            status: ToolCallStatus::Success,
        };
        let line = render_tool_call_line(&tc);
        let text: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert!(text.contains("\u{2713}")); // check mark
        assert!(text.contains("read_file"));
        assert!(text.contains("[150ms]"));
    }

    #[test]
    fn test_error_tool_call() {
        let tc = ToolCallDisplay {
            id: "tc1".to_string(),
            name: "bash".to_string(),
            input: json!({"command": "fail"}),
            output: Some(json!("error")),
            duration_ms: Some(50),
            collapsed: false,
            status: ToolCallStatus::Error,
        };
        let line = render_tool_call_line(&tc);
        let text: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert!(text.contains("\u{25be}")); // expanded arrow
        assert!(text.contains("\u{2717}")); // X mark
        assert!(text.contains("bash"));
    }

    #[test]
    fn test_empty_input() {
        let tc = ToolCallDisplay {
            id: "tc1".to_string(),
            name: "tool".to_string(),
            input: json!({}),
            output: None,
            duration_ms: None,
            collapsed: true,
            status: ToolCallStatus::Running,
        };
        let line = render_tool_call_line(&tc);
        let text: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        // Should not have parentheses for empty args
        assert!(!text.contains("()"));
    }

    #[test]
    fn test_null_input() {
        let tc = ToolCallDisplay {
            id: "tc1".to_string(),
            name: "tool".to_string(),
            input: json!(null),
            output: None,
            duration_ms: None,
            collapsed: true,
            status: ToolCallStatus::Running,
        };
        let line = render_tool_call_line(&tc);
        let text: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert!(!text.contains("null"));
    }

    #[test]
    fn test_long_input_truncated() {
        let tc = ToolCallDisplay {
            id: "tc1".to_string(),
            name: "tool".to_string(),
            input: json!({"very_long_key": "a very long value that should be truncated in the display"}),
            output: None,
            duration_ms: None,
            collapsed: true,
            status: ToolCallStatus::Running,
        };
        let line = render_tool_call_line(&tc);
        let text: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert!(text.contains("..."));
    }

    #[test]
    fn test_truncate_input_short() {
        let result = truncate_input(&json!({"a": 1}), 40);
        assert_eq!(result, r#"{"a":1}"#);
    }

    #[test]
    fn test_truncate_input_exact() {
        let input = json!("abc");
        let result = truncate_input(&input, 5);
        assert_eq!(result, r#""abc""#);
    }

    #[test]
    fn test_truncate_input_very_short_max() {
        let input = json!("abcdef");
        let result = truncate_input(&input, 3);
        assert_eq!(result, "...");
    }

    #[test]
    fn test_status_icon_styles() {
        let (_, style) = status_icon(&ToolCallStatus::Running);
        assert_eq!(style.fg, Some(Color::Yellow));

        let (_, style) = status_icon(&ToolCallStatus::Success);
        assert_eq!(style.fg, Some(Color::Green));

        let (_, style) = status_icon(&ToolCallStatus::Error);
        assert_eq!(style.fg, Some(Color::Red));
    }

    #[test]
    fn test_no_duration_no_bracket() {
        let tc = ToolCallDisplay {
            id: "tc1".to_string(),
            name: "tool".to_string(),
            input: json!({}),
            output: None,
            duration_ms: None,
            collapsed: true,
            status: ToolCallStatus::Running,
        };
        let line = render_tool_call_line(&tc);
        let text: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert!(!text.contains("ms]"));
    }
}
