use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::tui::app::{AppState, TuiState};

use super::command_palette::render_command_palette;

/// Render the input bar at the bottom of the screen.
pub fn render_input_bar(frame: &mut Frame, area: Rect, state: &AppState) {
    let (border_style, title) = match state.state {
        TuiState::Idle => (Style::default().fg(Color::Green), " Input "),
        TuiState::Running => (Style::default().fg(Color::Yellow), " Running... "),
        TuiState::AwaitingPermission => (Style::default().fg(Color::Red), " Permission Required "),
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);

    let inner = block.inner(area);

    if state.state == TuiState::Idle && state.input_buffer.is_empty() {
        // Show placeholder with 1-char left padding
        let placeholder = Paragraph::new(Line::from(Span::styled(
            " Type a message... (Enter: send, Ctrl+D: quit)",
            Style::default().fg(Color::DarkGray),
        )))
        .block(block);
        frame.render_widget(placeholder, area);
    } else {
        let display_text = if state.state == TuiState::Idle {
            format!(" > {}", state.input_buffer)
        } else {
            " > ...".to_string()
        };

        let paragraph = Paragraph::new(Line::from(display_text)).block(block);
        frame.render_widget(paragraph, area);

        // Set cursor position when idle (3 for " > ")
        if state.state == TuiState::Idle {
            let cursor_x = inner.x + 3 + state.input_cursor as u16;
            let cursor_y = inner.y;
            if cursor_x < inner.x + inner.width {
                frame.set_cursor_position((cursor_x, cursor_y));
            }
        }
    }

    // Render command palette above the input bar if open
    if state.command_palette_open {
        render_command_palette(frame, area, state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    fn create_terminal() -> Terminal<TestBackend> {
        let backend = TestBackend::new(80, 10);
        Terminal::new(backend).unwrap()
    }

    #[test]
    fn test_render_idle_empty() {
        let mut terminal = create_terminal();
        let state = AppState::new();
        terminal
            .draw(|frame| {
                let area = Rect::new(0, 7, 80, 3);
                render_input_bar(frame, area, &state);
            })
            .unwrap();
        let buf = terminal.backend().buffer();
        let text: String = (0..80)
            .map(|x| buf.cell((x, 8)).unwrap().symbol().to_string())
            .collect();
        assert!(text.contains("Type a message"));
    }

    #[test]
    fn test_render_with_input() {
        let mut terminal = create_terminal();
        let mut state = AppState::new();
        state.input_buffer = "hello".to_string();
        state.input_cursor = 5;
        terminal
            .draw(|frame| {
                let area = Rect::new(0, 7, 80, 3);
                render_input_bar(frame, area, &state);
            })
            .unwrap();
        let buf = terminal.backend().buffer();
        let text: String = (0..80)
            .map(|x| buf.cell((x, 8)).unwrap().symbol().to_string())
            .collect();
        assert!(text.contains(" > hello"));
    }

    #[test]
    fn test_render_running() {
        let mut terminal = create_terminal();
        let mut state = AppState::new();
        state.state = TuiState::Running;
        terminal
            .draw(|frame| {
                let area = Rect::new(0, 7, 80, 3);
                render_input_bar(frame, area, &state);
            })
            .unwrap();
    }

    #[test]
    fn test_render_awaiting_permission() {
        let mut terminal = create_terminal();
        let mut state = AppState::new();
        state.state = TuiState::AwaitingPermission;
        terminal
            .draw(|frame| {
                let area = Rect::new(0, 7, 80, 3);
                render_input_bar(frame, area, &state);
            })
            .unwrap();
    }

    #[test]
    fn test_render_with_command_palette() {
        let mut terminal = create_terminal();
        let mut state = AppState::new();
        state.input_buffer = "/hel".to_string();
        state.input_cursor = 4;
        state.command_palette_open = true;
        // command_filter is now computed from input_buffer
        terminal
            .draw(|frame| {
                let area = Rect::new(0, 7, 80, 3);
                render_input_bar(frame, area, &state);
            })
            .unwrap();
    }
}
