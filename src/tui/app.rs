use serde_json::Value;

/// The high-level state of the TUI application.
#[derive(Debug, Clone, PartialEq)]
pub enum TuiState {
    /// Awaiting user input; the agent is idle.
    Idle,
    /// The agent is actively processing.
    Running,
    /// A permission prompt is being displayed.
    AwaitingPermission,
}

/// Role of the sender in a chat message.
#[derive(Debug, Clone, PartialEq)]
pub enum MessageRole {
    User,
    Assistant,
    System,
}

/// Display state for a tool call within a message.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCallDisplay {
    pub id: String,
    pub name: String,
    pub input: Value,
    pub output: Option<Value>,
    pub duration_ms: Option<u64>,
    pub collapsed: bool,
    pub status: ToolCallStatus,
}

impl ToolCallDisplay {
    /// Returns true if this tool call resulted in an error.
    pub fn is_error(&self) -> bool {
        self.status == ToolCallStatus::Error
    }
}

/// The completion status of a tool call.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolCallStatus {
    Running,
    Success,
    Error,
}

/// A single chat message displayed in the TUI.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatMessage {
    pub role: MessageRole,
    pub text: String,
    pub tool_calls: Vec<ToolCallDisplay>,
}

/// Data for the status bar.
#[derive(Debug, Clone, PartialEq)]
pub struct StatusInfo {
    pub model: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub context_window: u32,
    pub cost_usd: f64,
    pub iteration: u32,
    pub max_iterations: u32,
    pub session_id: Option<String>,
    pub git_branch: Option<String>,
    pub git_modified: usize,
}

impl Default for StatusInfo {
    fn default() -> Self {
        Self {
            model: String::new(),
            input_tokens: 0,
            output_tokens: 0,
            context_window: 200_000,
            cost_usd: 0.0,
            iteration: 0,
            max_iterations: 0,
            session_id: None,
            git_branch: None,
            git_modified: 0,
        }
    }
}

impl StatusInfo {
    /// Returns the context usage as a percentage (0.0 - 100.0).
    pub fn context_percent(&self) -> f64 {
        if self.context_window == 0 {
            return 0.0;
        }
        (self.input_tokens as f64 / self.context_window as f64) * 100.0
    }
}

/// Data for the permission prompt overlay.
#[derive(Debug, Clone, PartialEq)]
pub struct PermissionPrompt {
    pub tool_name: String,
    pub tool_input: Value,
}

/// The complete application state for the TUI.
pub struct AppState {
    pub state: TuiState,
    pub messages: Vec<ChatMessage>,
    pub input_buffer: String,
    pub input_cursor: usize,
    pub scroll_offset: usize,
    pub status: StatusInfo,
    pub permission_request: Option<PermissionPrompt>,
    pub command_palette_open: bool,
    pub thinking_text: String,
    pub thinking_visible: bool,
    pub history: crate::tui::history::InputHistory,
}

impl AppState {
    /// Create a new AppState with sensible defaults.
    pub fn new() -> Self {
        Self {
            state: TuiState::Idle,
            messages: Vec::new(),
            input_buffer: String::new(),
            input_cursor: 0,
            scroll_offset: 0,
            status: StatusInfo::default(),
            permission_request: None,
            command_palette_open: false,
            thinking_text: String::new(),
            thinking_visible: false,
            history: crate::tui::history::InputHistory::new(1000),
        }
    }

    /// Get the current command filter derived from the input buffer.
    /// Returns the text after the leading '/' if present, or empty string.
    pub fn command_filter(&self) -> &str {
        self.input_buffer.strip_prefix('/').unwrap_or("")
    }

    /// Submit the current input buffer as a chat message, returning the text if non-empty.
    pub fn submit_input(&mut self) -> Option<String> {
        let text = self.input_buffer.trim().to_string();
        if text.is_empty() {
            return None;
        }

        self.history.add(text.clone());
        self.input_buffer.clear();
        self.input_cursor = 0;
        self.command_palette_open = false;

        self.messages.push(ChatMessage {
            role: MessageRole::User,
            text: text.clone(),
            tool_calls: Vec::new(),
        });

        Some(text)
    }

    /// Navigate backward in input history.
    pub fn history_back(&mut self) {
        let current = self.input_buffer.clone();
        if let Some(entry) = self.history.navigate_up(&current) {
            self.input_buffer = entry.to_string();
            self.input_cursor = self.input_buffer.len();
        }
    }

    /// Navigate forward in input history.
    pub fn history_forward(&mut self) {
        match self.history.navigate_down() {
            Some(entry) => {
                self.input_buffer = entry.to_string();
                self.input_cursor = self.input_buffer.len();
            }
            None => {
                self.input_buffer.clear();
                self.input_cursor = 0;
            }
        }
    }

    /// Insert a character at the current cursor position.
    pub fn insert_char(&mut self, c: char) {
        self.input_buffer.insert(self.input_cursor, c);
        self.input_cursor += c.len_utf8();
    }

    /// Delete the character before the cursor (backspace).
    pub fn delete_char_before(&mut self) {
        if self.input_cursor > 0 {
            let prev = self.prev_char_boundary();
            self.input_buffer.drain(prev..self.input_cursor);
            self.input_cursor = prev;
        }
    }

    /// Delete the character at the cursor (delete key).
    pub fn delete_char_at(&mut self) {
        if self.input_cursor < self.input_buffer.len() {
            let next = self.next_char_boundary();
            self.input_buffer.drain(self.input_cursor..next);
        }
    }

    /// Move cursor left by one character.
    pub fn cursor_left(&mut self) {
        if self.input_cursor > 0 {
            self.input_cursor = self.prev_char_boundary();
        }
    }

    /// Move cursor right by one character.
    pub fn cursor_right(&mut self) {
        if self.input_cursor < self.input_buffer.len() {
            self.input_cursor = self.next_char_boundary();
        }
    }

    /// Move cursor to the beginning of the input.
    pub fn cursor_home(&mut self) {
        self.input_cursor = 0;
    }

    /// Move cursor to the end of the input.
    pub fn cursor_end(&mut self) {
        self.input_cursor = self.input_buffer.len();
    }

    /// Scroll up by the given number of lines.
    pub fn scroll_up(&mut self, lines: usize) {
        self.scroll_offset = self.scroll_offset.saturating_add(lines);
    }

    /// Scroll down by the given number of lines.
    pub fn scroll_down(&mut self, lines: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(lines);
    }

    /// Add a system message to the chat display.
    pub fn add_system_message(&mut self, text: &str) {
        self.messages.push(ChatMessage {
            role: MessageRole::System,
            text: text.to_string(),
            tool_calls: Vec::new(),
        });
    }

    /// Get the last assistant message, creating one if needed.
    pub fn ensure_assistant_message(&mut self) -> &mut ChatMessage {
        let needs_new = self
            .messages
            .last()
            .is_none_or(|m| m.role != MessageRole::Assistant);
        if needs_new {
            self.messages.push(ChatMessage {
                role: MessageRole::Assistant,
                text: String::new(),
                tool_calls: Vec::new(),
            });
        }
        self.messages.last_mut().unwrap()
    }

    fn prev_char_boundary(&self) -> usize {
        let mut idx = self.input_cursor - 1;
        while !self.input_buffer.is_char_boundary(idx) {
            idx -= 1;
        }
        idx
    }

    fn next_char_boundary(&self) -> usize {
        let mut idx = self.input_cursor + 1;
        while idx < self.input_buffer.len() && !self.input_buffer.is_char_boundary(idx) {
            idx += 1;
        }
        idx
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_tui_state_equality() {
        assert_eq!(TuiState::Idle, TuiState::Idle);
        assert_eq!(TuiState::Running, TuiState::Running);
        assert_eq!(TuiState::AwaitingPermission, TuiState::AwaitingPermission);
        assert_ne!(TuiState::Idle, TuiState::Running);
    }

    #[test]
    fn test_app_state_new_defaults() {
        let state = AppState::new();
        assert_eq!(state.state, TuiState::Idle);
        assert!(state.messages.is_empty());
        assert!(state.input_buffer.is_empty());
        assert_eq!(state.input_cursor, 0);
        assert_eq!(state.scroll_offset, 0);
        assert!(state.permission_request.is_none());
        assert!(!state.command_palette_open);
        assert!(state.command_filter().is_empty());
        assert!(state.thinking_text.is_empty());
        assert!(!state.thinking_visible);
    }

    #[test]
    fn test_submit_input_empty() {
        let mut state = AppState::new();
        state.input_buffer = "   ".to_string();
        assert!(state.submit_input().is_none());
    }

    #[test]
    fn test_submit_input_non_empty() {
        let mut state = AppState::new();
        state.input_buffer = "hello world".to_string();
        state.input_cursor = 11;

        let result = state.submit_input();
        assert_eq!(result, Some("hello world".to_string()));
        assert!(state.input_buffer.is_empty());
        assert_eq!(state.input_cursor, 0);
        assert_eq!(state.messages.len(), 1);
        assert_eq!(state.messages[0].role, MessageRole::User);
        assert_eq!(state.messages[0].text, "hello world");
    }

    #[test]
    fn test_submit_input_trims_whitespace() {
        let mut state = AppState::new();
        state.input_buffer = "  hello  ".to_string();
        let result = state.submit_input();
        assert_eq!(result, Some("hello".to_string()));
    }

    #[test]
    fn test_insert_char() {
        let mut state = AppState::new();
        state.insert_char('a');
        state.insert_char('b');
        assert_eq!(state.input_buffer, "ab");
        assert_eq!(state.input_cursor, 2);
    }

    #[test]
    fn test_insert_char_at_middle() {
        let mut state = AppState::new();
        state.input_buffer = "ac".to_string();
        state.input_cursor = 1;
        state.insert_char('b');
        assert_eq!(state.input_buffer, "abc");
        assert_eq!(state.input_cursor, 2);
    }

    #[test]
    fn test_delete_char_before() {
        let mut state = AppState::new();
        state.input_buffer = "abc".to_string();
        state.input_cursor = 3;
        state.delete_char_before();
        assert_eq!(state.input_buffer, "ab");
        assert_eq!(state.input_cursor, 2);
    }

    #[test]
    fn test_delete_char_before_at_start() {
        let mut state = AppState::new();
        state.input_buffer = "abc".to_string();
        state.input_cursor = 0;
        state.delete_char_before();
        assert_eq!(state.input_buffer, "abc");
        assert_eq!(state.input_cursor, 0);
    }

    #[test]
    fn test_delete_char_at() {
        let mut state = AppState::new();
        state.input_buffer = "abc".to_string();
        state.input_cursor = 1;
        state.delete_char_at();
        assert_eq!(state.input_buffer, "ac");
        assert_eq!(state.input_cursor, 1);
    }

    #[test]
    fn test_delete_char_at_end() {
        let mut state = AppState::new();
        state.input_buffer = "abc".to_string();
        state.input_cursor = 3;
        state.delete_char_at();
        assert_eq!(state.input_buffer, "abc");
    }

    #[test]
    fn test_cursor_left_right() {
        let mut state = AppState::new();
        state.input_buffer = "abc".to_string();
        state.input_cursor = 1;
        state.cursor_left();
        assert_eq!(state.input_cursor, 0);
        state.cursor_left(); // already at 0
        assert_eq!(state.input_cursor, 0);
        state.cursor_right();
        assert_eq!(state.input_cursor, 1);
    }

    #[test]
    fn test_cursor_right_at_end() {
        let mut state = AppState::new();
        state.input_buffer = "ab".to_string();
        state.input_cursor = 2;
        state.cursor_right();
        assert_eq!(state.input_cursor, 2);
    }

    #[test]
    fn test_cursor_home_end() {
        let mut state = AppState::new();
        state.input_buffer = "hello".to_string();
        state.input_cursor = 3;
        state.cursor_home();
        assert_eq!(state.input_cursor, 0);
        state.cursor_end();
        assert_eq!(state.input_cursor, 5);
    }

    #[test]
    fn test_scroll_up_down() {
        let mut state = AppState::new();
        assert_eq!(state.scroll_offset, 0);
        state.scroll_up(5);
        assert_eq!(state.scroll_offset, 5);
        state.scroll_down(3);
        assert_eq!(state.scroll_offset, 2);
        state.scroll_down(10);
        assert_eq!(state.scroll_offset, 0);
    }

    #[test]
    fn test_history_back_empty() {
        let mut state = AppState::new();
        state.history_back();
        // No history entries, buffer stays empty
        assert!(state.input_buffer.is_empty());
    }

    #[test]
    fn test_history_back_and_forward() {
        let mut state = AppState::new();
        state.input_buffer = "first".to_string();
        state.submit_input();
        state.input_buffer = "second".to_string();
        state.submit_input();

        state.history_back();
        assert_eq!(state.input_buffer, "second");

        state.history_back();
        assert_eq!(state.input_buffer, "first");

        state.history_back(); // can't go further back
        assert_eq!(state.input_buffer, "first");

        state.history_forward();
        assert_eq!(state.input_buffer, "second");

        state.history_forward(); // past end clears
        assert!(state.input_buffer.is_empty());
    }

    #[test]
    fn test_history_forward_no_index() {
        let mut state = AppState::new();
        state.history.add("hello".to_string());
        // No navigation position, forward clears buffer
        state.history_forward();
        assert!(state.input_buffer.is_empty());
    }

    #[test]
    fn test_ensure_assistant_message_creates_new() {
        let mut state = AppState::new();
        let msg = state.ensure_assistant_message();
        assert_eq!(msg.role, MessageRole::Assistant);
        assert!(msg.text.is_empty());
        assert_eq!(state.messages.len(), 1);
    }

    #[test]
    fn test_ensure_assistant_message_reuses_existing() {
        let mut state = AppState::new();
        state.messages.push(ChatMessage {
            role: MessageRole::Assistant,
            text: "hello".to_string(),
            tool_calls: Vec::new(),
        });
        let msg = state.ensure_assistant_message();
        msg.text.push_str(" world");
        assert_eq!(state.messages.len(), 1);
        assert_eq!(state.messages[0].text, "hello world");
    }

    #[test]
    fn test_ensure_assistant_message_after_user() {
        let mut state = AppState::new();
        state.messages.push(ChatMessage {
            role: MessageRole::User,
            text: "hi".to_string(),
            tool_calls: Vec::new(),
        });
        state.ensure_assistant_message();
        assert_eq!(state.messages.len(), 2);
        assert_eq!(state.messages[1].role, MessageRole::Assistant);
    }

    #[test]
    fn test_status_info_context_percent() {
        let status = StatusInfo {
            input_tokens: 50_000,
            context_window: 200_000,
            ..StatusInfo::default()
        };
        assert!((status.context_percent() - 25.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_status_info_context_percent_zero_window() {
        let status = StatusInfo {
            context_window: 0,
            ..StatusInfo::default()
        };
        assert!((status.context_percent() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_permission_prompt_clone() {
        let prompt = PermissionPrompt {
            tool_name: "bash".to_string(),
            tool_input: json!({"command": "rm -rf /"}),
        };
        let cloned = prompt.clone();
        assert_eq!(prompt, cloned);
    }

    #[test]
    fn test_tool_call_display_defaults() {
        let tc = ToolCallDisplay {
            id: "tc_1".to_string(),
            name: "read_file".to_string(),
            input: json!({"path": "/tmp/test"}),
            output: None,
            duration_ms: None,
            collapsed: true,
            status: ToolCallStatus::Running,
        };
        assert_eq!(tc.status, ToolCallStatus::Running);
        assert!(tc.collapsed);
        assert!(tc.output.is_none());
        assert!(!tc.is_error());
    }

    #[test]
    fn test_tool_call_display_is_error() {
        let tc = ToolCallDisplay {
            id: "tc_1".to_string(),
            name: "bash".to_string(),
            input: json!({}),
            output: None,
            duration_ms: None,
            collapsed: true,
            status: ToolCallStatus::Error,
        };
        assert!(tc.is_error());
    }

    #[test]
    fn test_tool_call_status_equality() {
        assert_eq!(ToolCallStatus::Running, ToolCallStatus::Running);
        assert_eq!(ToolCallStatus::Success, ToolCallStatus::Success);
        assert_eq!(ToolCallStatus::Error, ToolCallStatus::Error);
        assert_ne!(ToolCallStatus::Running, ToolCallStatus::Success);
    }

    #[test]
    fn test_message_role_equality() {
        assert_eq!(MessageRole::User, MessageRole::User);
        assert_eq!(MessageRole::Assistant, MessageRole::Assistant);
        assert_eq!(MessageRole::System, MessageRole::System);
        assert_ne!(MessageRole::User, MessageRole::Assistant);
    }

    #[test]
    fn test_user_message_variants() {
        use crate::agent::interactive::UserMessage;
        let chat = UserMessage::Chat("hello".to_string());
        assert_eq!(chat, UserMessage::Chat("hello".to_string()));
        assert_ne!(chat, UserMessage::Interrupt);
        assert_ne!(chat, UserMessage::Quit);
    }

    #[test]
    fn test_insert_multibyte_char() {
        let mut state = AppState::new();
        state.insert_char('a');
        state.insert_char('\u{00e9}'); // e-acute, 2 bytes
        state.insert_char('b');
        assert_eq!(state.input_buffer, "a\u{00e9}b");
        assert_eq!(state.input_cursor, 4); // 1 + 2 + 1
    }

    #[test]
    fn test_delete_multibyte_char() {
        let mut state = AppState::new();
        state.input_buffer = "a\u{00e9}b".to_string();
        state.input_cursor = 3; // after e-acute
        state.delete_char_before();
        assert_eq!(state.input_buffer, "ab");
        assert_eq!(state.input_cursor, 1);
    }

    #[test]
    fn test_default_impl() {
        let state = AppState::default();
        assert_eq!(state.state, TuiState::Idle);
    }

    #[test]
    fn test_submit_clears_command_palette() {
        let mut state = AppState::new();
        state.command_palette_open = true;
        state.input_buffer = "/help".to_string();
        state.input_cursor = 5;
        state.submit_input();
        assert!(!state.command_palette_open);
    }
}
