use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::tui::app::AppState;

/// Render the status bar showing model, context usage, cost, iteration, etc.
pub fn render_status_bar(frame: &mut Frame, area: Rect, state: &AppState) {
    let spans = build_status_spans(state, area.width as usize);
    let paragraph = Paragraph::new(Line::from(spans))
        .style(Style::default().bg(Color::DarkGray).fg(Color::White));
    frame.render_widget(paragraph, area);
}

/// Build the span list for the status bar.
fn build_status_spans(state: &AppState, max_width: usize) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();

    // Model name (truncated if needed)
    let model_display = truncate_string(&state.status.model, 25);
    spans.push(Span::styled(
        format!(" {model_display}"),
        Style::default().add_modifier(Modifier::BOLD),
    ));

    spans.push(separator());

    // Context usage
    let ctx_pct = state.status.context_percent();
    let ctx_color = if ctx_pct > 90.0 {
        Color::Red
    } else if ctx_pct > 70.0 {
        Color::Yellow
    } else {
        Color::Green
    };
    spans.push(Span::styled(
        format!("ctx:{ctx_pct:.0}%"),
        Style::default().fg(ctx_color),
    ));

    spans.push(separator());

    // Cost
    spans.push(Span::raw(format!("${:.4}", state.status.cost_usd)));

    spans.push(separator());

    // Iteration
    if state.status.max_iterations > 0 {
        spans.push(Span::raw(format!(
            "iter:{}/{}",
            state.status.iteration, state.status.max_iterations
        )));
    } else {
        spans.push(Span::raw(format!("iter:{}", state.status.iteration)));
    }

    // Session ID (truncated)
    if let Some(ref sid) = state.status.session_id {
        spans.push(separator());
        let short = truncate_string(sid, 8);
        spans.push(Span::styled(short, Style::default().fg(Color::DarkGray)));
    }

    // Git branch
    if let Some(ref branch) = state.status.git_branch {
        spans.push(separator());
        let branch_text = if state.status.git_modified > 0 {
            format!("{branch} +{}", state.status.git_modified)
        } else {
            branch.clone()
        };
        spans.push(Span::styled(
            branch_text,
            Style::default().fg(Color::Magenta),
        ));
    }

    // Pad to fill width
    let current_len: usize = spans.iter().map(|s| s.content.len()).sum();
    if current_len < max_width {
        spans.push(Span::raw(" ".repeat(max_width - current_len)));
    }

    spans
}

fn separator() -> Span<'static> {
    Span::styled(" | ", Style::default().fg(Color::Gray))
}

/// Truncate a string to max_len, adding ".." if needed.
fn truncate_string(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else if max_len <= 2 {
        "..".to_string()
    } else {
        format!("{}..", &s[..max_len - 2])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::StatusInfo;
    use ratatui::{backend::TestBackend, Terminal};

    fn create_terminal() -> Terminal<TestBackend> {
        let backend = TestBackend::new(100, 5);
        Terminal::new(backend).unwrap()
    }

    #[test]
    fn test_render_default_status() {
        let mut terminal = create_terminal();
        let state = AppState::new();
        terminal
            .draw(|frame| {
                let area = Rect::new(0, 2, 100, 1);
                render_status_bar(frame, area, &state);
            })
            .unwrap();
        let buf = terminal.backend().buffer();
        let text: String = (0..100)
            .map(|x| buf.cell((x, 2)).unwrap().symbol().to_string())
            .collect();
        assert!(text.contains("ctx:"));
        assert!(text.contains("$0.0000"));
        assert!(text.contains("iter:0"));
    }

    #[test]
    fn test_status_with_session_id() {
        let mut state = AppState::new();
        state.status.session_id = Some("abcdefghijklmnop".to_string());
        let spans = build_status_spans(&state, 100);
        let text: String = spans.iter().map(|s| s.content.to_string()).collect();
        assert!(text.contains("abcdef..")); // truncated to 8
    }

    #[test]
    fn test_status_with_git_branch() {
        let mut state = AppState::new();
        state.status.git_branch = Some("main".to_string());
        state.status.git_modified = 3;
        let spans = build_status_spans(&state, 100);
        let text: String = spans.iter().map(|s| s.content.to_string()).collect();
        assert!(text.contains("main +3"));
    }

    #[test]
    fn test_status_with_git_no_modifications() {
        let mut state = AppState::new();
        state.status.git_branch = Some("feature".to_string());
        state.status.git_modified = 0;
        let spans = build_status_spans(&state, 100);
        let text: String = spans.iter().map(|s| s.content.to_string()).collect();
        assert!(text.contains("feature"));
        assert!(!text.contains("+0"));
    }

    #[test]
    fn test_context_percent_colors() {
        // Low usage = green
        let mut state = AppState::new();
        state.status.input_tokens = 10_000;
        state.status.context_window = 200_000;
        let spans = build_status_spans(&state, 100);
        let ctx_span = spans.iter().find(|s| s.content.contains("ctx:")).unwrap();
        assert_eq!(ctx_span.style.fg, Some(Color::Green));

        // High usage = yellow
        state.status.input_tokens = 150_000;
        let spans = build_status_spans(&state, 100);
        let ctx_span = spans.iter().find(|s| s.content.contains("ctx:")).unwrap();
        assert_eq!(ctx_span.style.fg, Some(Color::Yellow));

        // Critical usage = red
        state.status.input_tokens = 190_000;
        let spans = build_status_spans(&state, 100);
        let ctx_span = spans.iter().find(|s| s.content.contains("ctx:")).unwrap();
        assert_eq!(ctx_span.style.fg, Some(Color::Red));
    }

    #[test]
    fn test_max_iterations_display() {
        let mut state = AppState::new();
        state.status.iteration = 5;
        state.status.max_iterations = 50;
        let spans = build_status_spans(&state, 100);
        let text: String = spans.iter().map(|s| s.content.to_string()).collect();
        assert!(text.contains("iter:5/50"));
    }

    #[test]
    fn test_no_max_iterations() {
        let mut state = AppState::new();
        state.status.iteration = 3;
        state.status.max_iterations = 0;
        let spans = build_status_spans(&state, 100);
        let text: String = spans.iter().map(|s| s.content.to_string()).collect();
        assert!(text.contains("iter:3"));
        assert!(!text.contains("iter:3/"));
    }

    #[test]
    fn test_truncate_string_short() {
        assert_eq!(truncate_string("abc", 10), "abc");
    }

    #[test]
    fn test_truncate_string_exact() {
        assert_eq!(truncate_string("abc", 3), "abc");
    }

    #[test]
    fn test_truncate_string_long() {
        assert_eq!(truncate_string("abcdefghij", 5), "abc..");
    }

    #[test]
    fn test_truncate_string_very_short_max() {
        assert_eq!(truncate_string("abcdef", 2), "..");
    }

    #[test]
    fn test_cost_formatting() {
        let mut state = AppState::new();
        state.status.cost_usd = 1.2345;
        let spans = build_status_spans(&state, 100);
        let text: String = spans.iter().map(|s| s.content.to_string()).collect();
        assert!(text.contains("$1.2345"));
    }

    #[test]
    fn test_model_truncation() {
        let mut state = AppState::new();
        state.status.model = "claude-opus-4-20250514-with-extra-long-suffix".to_string();
        let spans = build_status_spans(&state, 100);
        let text: String = spans.iter().map(|s| s.content.to_string()).collect();
        // Model should be truncated to 25 chars
        assert!(text.contains(".."));
    }

    #[test]
    fn test_status_info_default() {
        let info = StatusInfo::default();
        assert_eq!(info.input_tokens, 0);
        assert_eq!(info.output_tokens, 0);
        assert_eq!(info.context_window, 200_000);
        assert!((info.cost_usd - 0.0).abs() < f64::EPSILON);
    }
}
