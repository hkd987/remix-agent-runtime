use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem},
    Frame,
};

use crate::tui::app::AppState;
use crate::tui::commands::CommandRegistry;

/// Render the command palette dropdown above the input bar.
pub fn render_command_palette(frame: &mut Frame, input_area: Rect, state: &AppState) {
    let registry = CommandRegistry::new();
    let filtered = filter_commands(&registry, state.command_filter());

    if filtered.is_empty() {
        return;
    }

    let visible_count = filtered.len().min(6) as u16;
    let palette_height = visible_count + 2; // +2 for borders

    // Position above the input bar
    let palette_y = input_area.y.saturating_sub(palette_height);
    let palette_area = Rect::new(
        input_area.x,
        palette_y,
        input_area.width.min(50),
        palette_height,
    );

    frame.render_widget(Clear, palette_area);

    let items: Vec<ListItem> = filtered
        .into_iter()
        .map(|cmd| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    cmd.name.to_string(),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" - "),
                Span::styled(
                    cmd.description.to_string(),
                    Style::default().fg(Color::DarkGray),
                ),
            ]))
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(" Commands "),
    );

    frame.render_widget(list, palette_area);
}

/// Filter commands by the current filter string using the registry.
fn filter_commands<'a>(
    registry: &'a CommandRegistry,
    filter: &str,
) -> Vec<&'a crate::tui::commands::SlashCommand> {
    let prefixed = if filter.is_empty() {
        String::new()
    } else {
        format!("/{filter}")
    };
    registry.filter(&prefixed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    fn create_terminal() -> Terminal<TestBackend> {
        let backend = TestBackend::new(80, 24);
        Terminal::new(backend).unwrap()
    }

    #[test]
    fn test_filter_commands_empty() {
        let registry = CommandRegistry::new();
        let result = filter_commands(&registry, "");
        assert_eq!(result.len(), registry.all().len());
    }

    #[test]
    fn test_filter_commands_h() {
        let registry = CommandRegistry::new();
        let result = filter_commands(&registry, "h");
        // /help
        assert!(result.iter().any(|cmd| cmd.name == "/help"));
    }

    #[test]
    fn test_filter_commands_he() {
        let registry = CommandRegistry::new();
        let result = filter_commands(&registry, "he");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "/help");
    }

    #[test]
    fn test_filter_commands_no_match() {
        let registry = CommandRegistry::new();
        let result = filter_commands(&registry, "xyz");
        assert!(result.is_empty());
    }

    #[test]
    fn test_filter_commands_case_insensitive() {
        let registry = CommandRegistry::new();
        let result = filter_commands(&registry, "H");
        assert!(result.iter().any(|cmd| cmd.name == "/help"));
    }

    #[test]
    fn test_filter_commands_q() {
        let registry = CommandRegistry::new();
        let result = filter_commands(&registry, "q");
        assert!(result.iter().any(|cmd| cmd.name == "/quit"));
    }

    #[test]
    fn test_filter_commands_c() {
        let registry = CommandRegistry::new();
        let result = filter_commands(&registry, "c");
        // /clear, /compact, /context
        assert!(result.len() >= 2);
    }

    #[test]
    fn test_render_palette_no_panic() {
        let mut terminal = create_terminal();
        let mut state = AppState::new();
        state.command_palette_open = true;
        state.input_buffer = "/h".to_string();
        terminal
            .draw(|frame| {
                let area = Rect::new(0, 21, 80, 3);
                render_command_palette(frame, area, &state);
            })
            .unwrap();
    }

    #[test]
    fn test_render_palette_no_match_no_render() {
        let mut terminal = create_terminal();
        let mut state = AppState::new();
        state.command_palette_open = true;
        state.input_buffer = "/zzz".to_string();
        terminal
            .draw(|frame| {
                let area = Rect::new(0, 21, 80, 3);
                render_command_palette(frame, area, &state);
            })
            .unwrap();
        // No panic = success (empty palette not rendered)
    }

    #[test]
    fn test_render_palette_all_commands() {
        let mut terminal = create_terminal();
        let mut state = AppState::new();
        state.command_palette_open = true;
        terminal
            .draw(|frame| {
                let area = Rect::new(0, 21, 80, 3);
                render_command_palette(frame, area, &state);
            })
            .unwrap();
    }

    #[test]
    fn test_commands_list_complete() {
        let registry = CommandRegistry::new();
        let names: Vec<&str> = registry.all().iter().map(|c| c.name).collect();
        assert!(names.contains(&"/help"));
        assert!(names.contains(&"/clear"));
        assert!(names.contains(&"/compact"));
        assert!(names.contains(&"/quit"));
        assert!(names.contains(&"/context"));
        assert!(names.contains(&"/diff"));
        assert!(names.contains(&"/undo"));
        assert!(names.contains(&"/rewind"));
    }

    #[test]
    fn test_commands_all_have_descriptions() {
        let registry = CommandRegistry::new();
        for cmd in registry.all() {
            assert!(!cmd.name.is_empty(), "command name should not be empty");
            assert!(
                !cmd.description.is_empty(),
                "command '{}' should have a description",
                cmd.name
            );
        }
    }
}
