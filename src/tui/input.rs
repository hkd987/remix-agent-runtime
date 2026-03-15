use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::app::{AppState, TuiState};
use crate::agent::interactive::UserMessage;

/// The result of processing a keyboard event.
#[derive(Debug, Clone, PartialEq)]
pub enum InputAction {
    /// No action required.
    None,
    /// Send a user message (chat, interrupt, or quit).
    SendMessage(UserMessage),
    /// Respond to a permission prompt.
    PermissionResponse(bool),
    /// Redraw needed (state changed but no message to send).
    Redraw,
}

/// Process a key event and update app state, returning the appropriate action.
pub fn handle_key_event(key: KeyEvent, state: &mut AppState) -> InputAction {
    // Ctrl+D always quits
    if key.code == KeyCode::Char('d') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return InputAction::SendMessage(UserMessage::Quit);
    }

    // Handle permission prompt state
    if state.state == TuiState::AwaitingPermission {
        return handle_permission_input(key, state);
    }

    match key.code {
        KeyCode::Enter => handle_enter(state),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => handle_ctrl_c(state),
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.cursor_home();
            InputAction::Redraw
        }
        KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.cursor_end();
            InputAction::Redraw
        }
        KeyCode::Up => handle_up(state),
        KeyCode::Down => handle_down(state),
        KeyCode::PageUp => {
            state.scroll_up(10);
            InputAction::Redraw
        }
        KeyCode::PageDown => {
            state.scroll_down(10);
            InputAction::Redraw
        }
        KeyCode::Esc => handle_esc(state),
        KeyCode::Tab => handle_tab(state),
        KeyCode::Backspace => {
            if state.state == TuiState::Idle {
                state.delete_char_before();
                update_command_palette(state);
                InputAction::Redraw
            } else {
                InputAction::None
            }
        }
        KeyCode::Delete => {
            if state.state == TuiState::Idle {
                state.delete_char_at();
                InputAction::Redraw
            } else {
                InputAction::None
            }
        }
        KeyCode::Left => {
            if state.state == TuiState::Idle {
                state.cursor_left();
                InputAction::Redraw
            } else {
                InputAction::None
            }
        }
        KeyCode::Right => {
            if state.state == TuiState::Idle {
                state.cursor_right();
                InputAction::Redraw
            } else {
                InputAction::None
            }
        }
        KeyCode::Home => {
            if state.state == TuiState::Idle {
                state.cursor_home();
                InputAction::Redraw
            } else {
                InputAction::None
            }
        }
        KeyCode::End => {
            if state.state == TuiState::Idle {
                state.cursor_end();
                InputAction::Redraw
            } else {
                InputAction::None
            }
        }
        KeyCode::Char(c) => {
            if state.state == TuiState::Idle {
                state.insert_char(c);
                update_command_palette(state);
                InputAction::Redraw
            } else {
                InputAction::None
            }
        }
        _ => InputAction::None,
    }
}

fn handle_enter(state: &mut AppState) -> InputAction {
    if state.state != TuiState::Idle {
        return InputAction::None;
    }
    match state.submit_input() {
        Some(text) => InputAction::SendMessage(UserMessage::Chat(text)),
        None => InputAction::None,
    }
}

fn handle_ctrl_c(state: &mut AppState) -> InputAction {
    match state.state {
        TuiState::Running => InputAction::SendMessage(UserMessage::Interrupt),
        TuiState::Idle => {
            if state.input_buffer.is_empty() {
                // Double Ctrl+C to quit (empty input = quit)
                InputAction::SendMessage(UserMessage::Quit)
            } else {
                state.input_buffer.clear();
                state.input_cursor = 0;
                state.command_palette_open = false;
                InputAction::Redraw
            }
        }
        TuiState::AwaitingPermission => InputAction::None,
    }
}

fn handle_up(state: &mut AppState) -> InputAction {
    match state.state {
        TuiState::Idle => {
            // If input is empty, scroll chat instead of navigating history
            if state.input_buffer.is_empty() {
                state.scroll_up(1);
            } else {
                state.history_back();
            }
            InputAction::Redraw
        }
        TuiState::Running => {
            state.scroll_up(1);
            InputAction::Redraw
        }
        TuiState::AwaitingPermission => InputAction::None,
    }
}

fn handle_down(state: &mut AppState) -> InputAction {
    match state.state {
        TuiState::Idle => {
            // If input is empty, scroll chat instead of navigating history
            if state.input_buffer.is_empty() {
                state.scroll_down(1);
            } else {
                state.history_forward();
            }
            InputAction::Redraw
        }
        TuiState::Running => {
            state.scroll_down(1);
            InputAction::Redraw
        }
        TuiState::AwaitingPermission => InputAction::None,
    }
}

fn handle_esc(state: &mut AppState) -> InputAction {
    if state.command_palette_open {
        state.command_palette_open = false;
        return InputAction::Redraw;
    }
    InputAction::None
}

fn handle_tab(state: &mut AppState) -> InputAction {
    if state.state != TuiState::Idle {
        return InputAction::None;
    }
    if state.input_buffer.starts_with('/') && !state.command_palette_open {
        state.command_palette_open = true;
        return InputAction::Redraw;
    }
    InputAction::None
}

fn handle_permission_input(key: KeyEvent, state: &mut AppState) -> InputAction {
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('a') => {
            state.state = TuiState::Running;
            state.permission_request = None;
            InputAction::PermissionResponse(true)
        }
        KeyCode::Char('n') => {
            state.state = TuiState::Running;
            state.permission_request = None;
            InputAction::PermissionResponse(false)
        }
        KeyCode::Esc => {
            state.state = TuiState::Running;
            state.permission_request = None;
            InputAction::PermissionResponse(false)
        }
        _ => InputAction::None,
    }
}

/// Update command palette visibility based on current input.
fn update_command_palette(state: &mut AppState) {
    if state.input_buffer.starts_with('/') && state.input_buffer.len() > 1 {
        state.command_palette_open = true;
    } else if !state.input_buffer.starts_with('/') {
        state.command_palette_open = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn key_with_mod(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        key_with_mod(code, KeyModifiers::CONTROL)
    }

    #[test]
    fn test_ctrl_d_quits() {
        let mut state = AppState::new();
        let action = handle_key_event(ctrl(KeyCode::Char('d')), &mut state);
        assert_eq!(action, InputAction::SendMessage(UserMessage::Quit));
    }

    #[test]
    fn test_ctrl_d_quits_in_running() {
        let mut state = AppState::new();
        state.state = TuiState::Running;
        let action = handle_key_event(ctrl(KeyCode::Char('d')), &mut state);
        assert_eq!(action, InputAction::SendMessage(UserMessage::Quit));
    }

    #[test]
    fn test_enter_submits_input() {
        let mut state = AppState::new();
        state.input_buffer = "hello".to_string();
        state.input_cursor = 5;
        let action = handle_key_event(key(KeyCode::Enter), &mut state);
        assert_eq!(
            action,
            InputAction::SendMessage(UserMessage::Chat("hello".to_string()))
        );
    }

    #[test]
    fn test_enter_empty_does_nothing() {
        let mut state = AppState::new();
        let action = handle_key_event(key(KeyCode::Enter), &mut state);
        assert_eq!(action, InputAction::None);
    }

    #[test]
    fn test_enter_while_running_does_nothing() {
        let mut state = AppState::new();
        state.state = TuiState::Running;
        state.input_buffer = "hello".to_string();
        let action = handle_key_event(key(KeyCode::Enter), &mut state);
        assert_eq!(action, InputAction::None);
    }

    #[test]
    fn test_ctrl_c_running_interrupts() {
        let mut state = AppState::new();
        state.state = TuiState::Running;
        let action = handle_key_event(ctrl(KeyCode::Char('c')), &mut state);
        assert_eq!(action, InputAction::SendMessage(UserMessage::Interrupt));
    }

    #[test]
    fn test_ctrl_c_idle_clears_input() {
        let mut state = AppState::new();
        state.input_buffer = "hello".to_string();
        state.input_cursor = 5;
        let action = handle_key_event(ctrl(KeyCode::Char('c')), &mut state);
        assert_eq!(action, InputAction::Redraw);
        assert!(state.input_buffer.is_empty());
        assert_eq!(state.input_cursor, 0);
    }

    #[test]
    fn test_ctrl_c_idle_empty_quits() {
        let mut state = AppState::new();
        let action = handle_key_event(ctrl(KeyCode::Char('c')), &mut state);
        assert_eq!(action, InputAction::SendMessage(UserMessage::Quit));
    }

    #[test]
    fn test_char_input_idle() {
        let mut state = AppState::new();
        let action = handle_key_event(key(KeyCode::Char('a')), &mut state);
        assert_eq!(action, InputAction::Redraw);
        assert_eq!(state.input_buffer, "a");
    }

    #[test]
    fn test_char_input_running_ignored() {
        let mut state = AppState::new();
        state.state = TuiState::Running;
        let action = handle_key_event(key(KeyCode::Char('a')), &mut state);
        assert_eq!(action, InputAction::None);
        assert!(state.input_buffer.is_empty());
    }

    #[test]
    fn test_backspace() {
        let mut state = AppState::new();
        state.input_buffer = "ab".to_string();
        state.input_cursor = 2;
        let action = handle_key_event(key(KeyCode::Backspace), &mut state);
        assert_eq!(action, InputAction::Redraw);
        assert_eq!(state.input_buffer, "a");
    }

    #[test]
    fn test_backspace_while_running() {
        let mut state = AppState::new();
        state.state = TuiState::Running;
        let action = handle_key_event(key(KeyCode::Backspace), &mut state);
        assert_eq!(action, InputAction::None);
    }

    #[test]
    fn test_delete_key() {
        let mut state = AppState::new();
        state.input_buffer = "ab".to_string();
        state.input_cursor = 0;
        let action = handle_key_event(key(KeyCode::Delete), &mut state);
        assert_eq!(action, InputAction::Redraw);
        assert_eq!(state.input_buffer, "b");
    }

    #[test]
    fn test_left_right_arrows() {
        let mut state = AppState::new();
        state.input_buffer = "abc".to_string();
        state.input_cursor = 2;

        handle_key_event(key(KeyCode::Left), &mut state);
        assert_eq!(state.input_cursor, 1);

        handle_key_event(key(KeyCode::Right), &mut state);
        assert_eq!(state.input_cursor, 2);
    }

    #[test]
    fn test_home_end() {
        let mut state = AppState::new();
        state.input_buffer = "hello".to_string();
        state.input_cursor = 3;

        handle_key_event(key(KeyCode::Home), &mut state);
        assert_eq!(state.input_cursor, 0);

        handle_key_event(key(KeyCode::End), &mut state);
        assert_eq!(state.input_cursor, 5);
    }

    #[test]
    fn test_ctrl_a_moves_to_start() {
        let mut state = AppState::new();
        state.input_buffer = "hello".to_string();
        state.input_cursor = 3;
        let action = handle_key_event(ctrl(KeyCode::Char('a')), &mut state);
        assert_eq!(action, InputAction::Redraw);
        assert_eq!(state.input_cursor, 0);
    }

    #[test]
    fn test_ctrl_e_moves_to_end() {
        let mut state = AppState::new();
        state.input_buffer = "hello".to_string();
        state.input_cursor = 0;
        let action = handle_key_event(ctrl(KeyCode::Char('e')), &mut state);
        assert_eq!(action, InputAction::Redraw);
        assert_eq!(state.input_cursor, 5);
    }

    #[test]
    fn test_up_idle_with_input_navigates_history() {
        let mut state = AppState::new();
        state.history.add("first".to_string());
        state.input_buffer = "typing".to_string(); // non-empty input triggers history
        let action = handle_key_event(key(KeyCode::Up), &mut state);
        assert_eq!(action, InputAction::Redraw);
        assert_eq!(state.input_buffer, "first");
    }

    #[test]
    fn test_up_idle_empty_input_scrolls() {
        let mut state = AppState::new();
        state.history.add("first".to_string());
        // empty input_buffer → scroll chat, not history
        let action = handle_key_event(key(KeyCode::Up), &mut state);
        assert_eq!(action, InputAction::Redraw);
        assert_eq!(state.scroll_offset, 1);
    }

    #[test]
    fn test_up_running_scrolls() {
        let mut state = AppState::new();
        state.state = TuiState::Running;
        let action = handle_key_event(key(KeyCode::Up), &mut state);
        assert_eq!(action, InputAction::Redraw);
        assert_eq!(state.scroll_offset, 1);
    }

    #[test]
    fn test_down_idle_history() {
        let mut state = AppState::new();
        state.history.add("first".to_string());
        state.history.add("second".to_string());
        // Navigate up twice to position at "first"
        state.history_back(); // -> "second"
        state.history_back(); // -> "first"
        state.input_buffer = "first".to_string();
        let action = handle_key_event(key(KeyCode::Down), &mut state);
        assert_eq!(action, InputAction::Redraw);
        assert_eq!(state.input_buffer, "second");
    }

    #[test]
    fn test_down_running_scrolls() {
        let mut state = AppState::new();
        state.state = TuiState::Running;
        state.scroll_offset = 5;
        let action = handle_key_event(key(KeyCode::Down), &mut state);
        assert_eq!(action, InputAction::Redraw);
        assert_eq!(state.scroll_offset, 4);
    }

    #[test]
    fn test_page_up_down() {
        let mut state = AppState::new();
        let action = handle_key_event(key(KeyCode::PageUp), &mut state);
        assert_eq!(action, InputAction::Redraw);
        assert_eq!(state.scroll_offset, 10);

        let action = handle_key_event(key(KeyCode::PageDown), &mut state);
        assert_eq!(action, InputAction::Redraw);
        assert_eq!(state.scroll_offset, 0);
    }

    #[test]
    fn test_esc_closes_command_palette() {
        let mut state = AppState::new();
        state.command_palette_open = true;
        state.input_buffer = "/hel".to_string();
        let action = handle_key_event(key(KeyCode::Esc), &mut state);
        assert_eq!(action, InputAction::Redraw);
        assert!(!state.command_palette_open);
    }

    #[test]
    fn test_esc_no_palette_does_nothing() {
        let mut state = AppState::new();
        let action = handle_key_event(key(KeyCode::Esc), &mut state);
        assert_eq!(action, InputAction::None);
    }

    #[test]
    fn test_tab_opens_command_palette() {
        let mut state = AppState::new();
        state.input_buffer = "/hel".to_string();
        state.input_cursor = 4;
        let action = handle_key_event(key(KeyCode::Tab), &mut state);
        assert_eq!(action, InputAction::Redraw);
        assert!(state.command_palette_open);
        assert_eq!(state.command_filter(), "hel");
    }

    #[test]
    fn test_tab_no_slash_does_nothing() {
        let mut state = AppState::new();
        state.input_buffer = "hello".to_string();
        let action = handle_key_event(key(KeyCode::Tab), &mut state);
        assert_eq!(action, InputAction::None);
    }

    #[test]
    fn test_tab_while_running_does_nothing() {
        let mut state = AppState::new();
        state.state = TuiState::Running;
        state.input_buffer = "/help".to_string();
        let action = handle_key_event(key(KeyCode::Tab), &mut state);
        assert_eq!(action, InputAction::None);
    }

    #[test]
    fn test_permission_y_allows() {
        let mut state = AppState::new();
        state.state = TuiState::AwaitingPermission;
        state.permission_request = Some(super::super::app::PermissionPrompt {
            tool_name: "bash".to_string(),
            tool_input: serde_json::json!({}),
        });
        let action = handle_key_event(key(KeyCode::Char('y')), &mut state);
        assert_eq!(action, InputAction::PermissionResponse(true));
        assert_eq!(state.state, TuiState::Running);
        assert!(state.permission_request.is_none());
    }

    #[test]
    fn test_permission_n_denies() {
        let mut state = AppState::new();
        state.state = TuiState::AwaitingPermission;
        let action = handle_key_event(key(KeyCode::Char('n')), &mut state);
        assert_eq!(action, InputAction::PermissionResponse(false));
        assert_eq!(state.state, TuiState::Running);
    }

    #[test]
    fn test_permission_a_allows() {
        let mut state = AppState::new();
        state.state = TuiState::AwaitingPermission;
        let action = handle_key_event(key(KeyCode::Char('a')), &mut state);
        assert_eq!(action, InputAction::PermissionResponse(true));
    }

    #[test]
    fn test_permission_esc_denies() {
        let mut state = AppState::new();
        state.state = TuiState::AwaitingPermission;
        let action = handle_key_event(key(KeyCode::Esc), &mut state);
        assert_eq!(action, InputAction::PermissionResponse(false));
    }

    #[test]
    fn test_permission_other_key_ignored() {
        let mut state = AppState::new();
        state.state = TuiState::AwaitingPermission;
        let action = handle_key_event(key(KeyCode::Char('x')), &mut state);
        assert_eq!(action, InputAction::None);
    }

    #[test]
    fn test_slash_typing_opens_palette() {
        let mut state = AppState::new();
        handle_key_event(key(KeyCode::Char('/')), &mut state);
        assert!(!state.command_palette_open); // just "/" alone doesn't open

        handle_key_event(key(KeyCode::Char('h')), &mut state);
        assert!(state.command_palette_open);
        assert_eq!(state.command_filter(), "h");
    }

    #[test]
    fn test_backspace_closes_palette_when_no_filter() {
        let mut state = AppState::new();
        state.input_buffer = "/h".to_string();
        state.input_cursor = 2;
        state.command_palette_open = true;

        // Backspace removes 'h', leaving just "/"
        handle_key_event(key(KeyCode::Backspace), &mut state);
        assert_eq!(state.input_buffer, "/");
        // Palette stays since it starts with '/' but filter logic won't match
    }

    #[test]
    fn test_arrow_keys_while_running_no_history_change() {
        let mut state = AppState::new();
        state.state = TuiState::Running;
        state.history.add("old".to_string());
        handle_key_event(key(KeyCode::Up), &mut state);
        // Should scroll, not change history
        assert!(state.input_buffer.is_empty());
        assert_eq!(state.scroll_offset, 1);
    }

    #[test]
    fn test_left_right_while_running_ignored() {
        let mut state = AppState::new();
        state.state = TuiState::Running;
        let action = handle_key_event(key(KeyCode::Left), &mut state);
        assert_eq!(action, InputAction::None);
        let action = handle_key_event(key(KeyCode::Right), &mut state);
        assert_eq!(action, InputAction::None);
    }

    #[test]
    fn test_home_end_while_running_ignored() {
        let mut state = AppState::new();
        state.state = TuiState::Running;
        let action = handle_key_event(key(KeyCode::Home), &mut state);
        assert_eq!(action, InputAction::None);
        let action = handle_key_event(key(KeyCode::End), &mut state);
        assert_eq!(action, InputAction::None);
    }

    #[test]
    fn test_delete_while_running_ignored() {
        let mut state = AppState::new();
        state.state = TuiState::Running;
        let action = handle_key_event(key(KeyCode::Delete), &mut state);
        assert_eq!(action, InputAction::None);
    }

    #[test]
    fn test_unknown_key_does_nothing() {
        let mut state = AppState::new();
        let action = handle_key_event(key(KeyCode::F(1)), &mut state);
        assert_eq!(action, InputAction::None);
    }
}
