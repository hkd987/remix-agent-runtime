pub mod app;
pub mod checkpoint;
pub mod commands;
pub mod event_handler;
pub mod history;
pub mod input;
pub mod prompt;
pub mod render;
pub mod widgets;

use std::io;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use crossterm::{
    event::{self, Event},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use tokio::sync::mpsc;

use crate::agent::interactive::UserMessage;
use crate::agent::AgentRunner;
use crate::agents_md::AgentsMdContent;
use crate::browser::mcp::ToolExecutor;
use crate::config::credentials::CredentialSet;
use crate::config::schema::CompactionConfig;
use crate::error::ExitStatus;
use crate::llm::client::StreamingLlmProvider;
use crate::output::events::EventBus;
use crate::session::{SessionStorage, SessionStore};
use crate::skills::SkillSet;

use self::app::{AppState, TuiState};
use self::event_handler::handle_agent_event;
use self::input::{handle_key_event, InputAction};
use self::render::render;

/// Tick rate for the TUI render loop (~60fps).
const TICK_RATE: Duration = Duration::from_millis(16);

/// High-level entry point called from `main.rs` for the `chat` command.
///
/// Sets up the terminal, spawns the agent loop on a tokio task, and runs
/// the TUI render/input loop on the main thread.
#[allow(clippy::too_many_arguments)]
pub async fn run_tui<L, T>(
    runner: AgentRunner<L, T>,
    event_bus: Arc<EventBus>,
    credential_set: CredentialSet,
    skill_set: SkillSet,
    agents_md: Option<AgentsMdContent>,
    session_store: Option<SessionStore>,
    compaction_config: Option<CompactionConfig>,
    model_name: String,
) -> ExitCode
where
    L: StreamingLlmProvider + Send + Sync + 'static,
    T: ToolExecutor + Send + Sync + 'static,
{
    // Set up terminal with panic hook for cleanup
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        original_hook(panic_info);
    }));

    if let Err(e) = enable_raw_mode() {
        eprintln!("Error: Failed to enable raw mode: {e}");
        return ExitStatus::AgentError.into();
    }

    let mut stdout = io::stdout();
    if let Err(e) = execute!(
        stdout,
        EnterAlternateScreen,
        crossterm::event::EnableMouseCapture
    ) {
        let _ = disable_raw_mode();
        eprintln!("Error: Failed to enter alternate screen: {e}");
        return ExitStatus::AgentError.into();
    }

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = match Terminal::new(backend) {
        Ok(t) => t,
        Err(e) => {
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
            eprintln!("Error: Failed to create terminal: {e}");
            return ExitStatus::AgentError.into();
        }
    };
    let _ = terminal.clear();

    // Create channels for TUI <-> Agent communication
    let (input_tx, mut input_rx) = mpsc::channel::<UserMessage>(32);
    let (signal_tx, mut signal_rx) =
        mpsc::channel::<crate::agent::interactive::InteractiveSignal>(8);

    // Subscribe to agent events for the TUI
    let mut event_rx = event_bus.subscribe();

    // Initialize app state
    let mut state = AppState::new();
    state.status.model = model_name;

    // Store the permission response sender alongside the prompt
    let mut permission_respond: Option<tokio::sync::oneshot::Sender<bool>> = None;

    // Spawn the agent streaming loop on a background tokio task
    let agent_handle = tokio::spawn(async move {
        let mut runner = runner;
        let session_store_ref = session_store.as_ref().map(|s| s as &dyn SessionStorage);
        let compaction_ref = compaction_config.as_ref();

        runner
            .run_interactive_stream(
                &mut input_rx,
                &signal_tx,
                &credential_set,
                &skill_set,
                &agents_md,
                session_store_ref,
                compaction_ref,
            )
            .await
    });

    // Main TUI event loop
    let exit_code = loop {
        // Render
        if let Err(e) = terminal.draw(|frame| {
            render(frame, &state);
        }) {
            tracing::error!(error = %e, "TUI render error");
        }

        // Poll for terminal input events
        let has_event = event::poll(TICK_RATE).unwrap_or(false);
        if has_event {
            match event::read() {
                Ok(Event::Mouse(mouse_event)) => match mouse_event.kind {
                    crossterm::event::MouseEventKind::ScrollUp => {
                        state.scroll_up(3);
                    }
                    crossterm::event::MouseEventKind::ScrollDown => {
                        state.scroll_down(3);
                    }
                    _ => {}
                },
                Ok(Event::Key(key_event)) => {
                    match handle_key_event(key_event, &mut state) {
                        InputAction::None | InputAction::Redraw => {}
                        InputAction::SendMessage(msg) => {
                            match msg {
                                UserMessage::Quit => {
                                    // Send quit to agent and exit
                                    let _ = input_tx.send(UserMessage::Quit).await;
                                    break ExitStatus::Success;
                                }
                                UserMessage::Interrupt => {
                                    let _ = input_tx.send(UserMessage::Interrupt).await;
                                }
                                UserMessage::Chat(ref text) => {
                                    if text.starts_with('/') {
                                        // Handle slash commands locally
                                        if let Some(action) = commands::CommandRegistry::parse(text)
                                        {
                                            handle_slash_command(action, &mut state);
                                            continue;
                                        }
                                    }
                                    // Send to agent
                                    state.state = TuiState::Running;
                                    state.thinking_text.clear();
                                    let _ = input_tx.send(UserMessage::Chat(text.clone())).await;
                                }
                            }
                        }
                        InputAction::PermissionResponse(allowed) => {
                            state.permission_request = None;
                            state.state = TuiState::Running;
                            if let Some(respond) = permission_respond.take() {
                                let _ = respond.send(allowed);
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        // Process agent events (non-blocking drain)
        loop {
            match event_rx.try_recv() {
                Ok(agent_event) => {
                    handle_agent_event(agent_event, &mut state);
                }
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(n)) => {
                    tracing::warn!(skipped = n, "Event bus lagged");
                    break;
                }
                Err(tokio::sync::broadcast::error::TryRecvError::Closed) => break,
            }
        }

        // Process interactive signals (permission requests, awaiting input)
        loop {
            match signal_rx.try_recv() {
                Ok(crate::agent::interactive::InteractiveSignal::AwaitingInput) => {
                    state.state = TuiState::Idle;
                }
                Ok(crate::agent::interactive::InteractiveSignal::PermissionRequest {
                    tool_name,
                    tool_input,
                    respond,
                }) => {
                    state.state = TuiState::AwaitingPermission;
                    state.permission_request = Some(app::PermissionPrompt {
                        tool_name,
                        tool_input,
                    });
                    permission_respond = Some(respond);
                }
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    // Agent task ended
                    state.state = TuiState::Idle;
                    break;
                }
            }
        }

        // Check if agent task has finished
        if agent_handle.is_finished() {
            state.state = TuiState::Idle;
        }
    };

    // Restore terminal
    let _ = disable_raw_mode();
    let _ = execute!(
        terminal.backend_mut(),
        crossterm::event::DisableMouseCapture,
        LeaveAlternateScreen
    );
    let _ = terminal.show_cursor();

    exit_code.into()
}

/// Handle a parsed slash command locally (not sent to agent).
fn handle_slash_command(action: commands::CommandAction, state: &mut AppState) {
    match action {
        commands::CommandAction::Help => {
            let registry = commands::CommandRegistry::new();
            let mut help_text = String::from("Available commands:\n");
            for cmd in registry.all() {
                help_text.push_str(&format!("  {} — {}\n", cmd.name, cmd.description));
            }
            help_text.push_str(
                "\nShortcuts: Enter=send  Ctrl+C=interrupt  Ctrl+D=quit  Up/Down=history",
            );
            state.add_system_message(&help_text);
        }
        commands::CommandAction::Clear => {
            state.messages.clear();
            state.scroll_offset = 0;
        }
        commands::CommandAction::Compact => {
            state.add_system_message("Context compaction triggered.");
        }
        commands::CommandAction::Context => {
            let info = format!(
                "Tokens: {} in / {} out | Context: {:.1}% | Cost: ${:.4}",
                state.status.input_tokens,
                state.status.output_tokens,
                state.status.context_percent(),
                state.status.cost_usd,
            );
            state.add_system_message(&info);
        }
        commands::CommandAction::Diff => {
            state.add_system_message("Running git diff...");
        }
        commands::CommandAction::Undo => {
            state.add_system_message("Undo: restoring to last checkpoint...");
        }
        commands::CommandAction::Rewind => {
            state.add_system_message("Rewind: showing checkpoint list...");
        }
        commands::CommandAction::Quit => {
            // Handled by the main loop via InputAction::SendMessage(Quit)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::events::AgentEvent;
    use ratatui::{backend::TestBackend, Terminal};

    #[test]
    fn test_tick_rate_is_reasonable() {
        assert!(TICK_RATE.as_millis() > 0);
        assert!(TICK_RATE.as_millis() <= 100);
    }

    #[test]
    fn test_full_render_cycle_with_test_backend() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new();

        state.messages.push(app::ChatMessage {
            role: app::MessageRole::User,
            text: "Hello agent".to_string(),
            tool_calls: Vec::new(),
        });
        state.messages.push(app::ChatMessage {
            role: app::MessageRole::Assistant,
            text: "Hi! How can I help?".to_string(),
            tool_calls: Vec::new(),
        });
        state.status.input_tokens = 1000;
        state.status.cost_usd = 0.001;
        state.status.iteration = 1;

        terminal
            .draw(|frame| {
                render(frame, &state);
            })
            .unwrap();

        let buf = terminal.backend().buffer();
        let all_text: String = (0..24)
            .flat_map(|y| (0..80).map(move |x| (x, y)))
            .map(|(x, y)| buf.cell((x, y)).unwrap().symbol().to_string())
            .collect();
        assert!(all_text.contains("remix-agent"));
        assert!(all_text.contains("Hello agent"));
        assert!(all_text.contains("How can I help"));
    }

    #[tokio::test]
    async fn test_event_bus_integration() {
        let bus = EventBus::new(64);
        let mut rx = bus.subscribe();

        bus.send(AgentEvent::TextDelta {
            text: "hello".to_string(),
        });

        let event = rx.recv().await.unwrap();
        let mut state = AppState::new();
        handle_agent_event(event, &mut state);

        assert_eq!(state.messages.len(), 1);
        assert_eq!(state.messages[0].text, "hello");
    }

    #[test]
    fn test_render_all_states() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();

        for tui_state in [
            TuiState::Idle,
            TuiState::Running,
            TuiState::AwaitingPermission,
        ] {
            let mut state = AppState::new();
            state.state = tui_state;
            if state.state == TuiState::AwaitingPermission {
                state.permission_request = Some(app::PermissionPrompt {
                    tool_name: "bash".to_string(),
                    tool_input: serde_json::json!({"command": "ls"}),
                });
            }
            terminal
                .draw(|frame| {
                    render(frame, &state);
                })
                .unwrap();
        }
    }

    #[test]
    fn test_input_then_render() {
        use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new();

        let keys = vec!['h', 'e', 'l', 'l', 'o'];
        for c in keys {
            let key_event = KeyEvent {
                code: KeyCode::Char(c),
                modifiers: KeyModifiers::NONE,
                kind: KeyEventKind::Press,
                state: KeyEventState::NONE,
            };
            handle_key_event(key_event, &mut state);
        }

        assert_eq!(state.input_buffer, "hello");

        terminal
            .draw(|frame| {
                render(frame, &state);
            })
            .unwrap();
    }

    #[test]
    fn test_event_processing_then_render() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new();

        handle_agent_event(
            AgentEvent::AgentStarted {
                task: "test".to_string(),
                timestamp_ms: 0,
            },
            &mut state,
        );
        handle_agent_event(AgentEvent::IterationStarted { iteration: 1 }, &mut state);
        handle_agent_event(
            AgentEvent::TextDelta {
                text: "Working on it...".to_string(),
            },
            &mut state,
        );
        handle_agent_event(
            AgentEvent::ToolUseStart {
                id: "tc1".to_string(),
                name: "read_file".to_string(),
                input: serde_json::json!({"path": "/tmp/test.rs"}),
            },
            &mut state,
        );
        handle_agent_event(
            AgentEvent::ToolUseResult {
                id: "tc1".to_string(),
                name: "read_file".to_string(),
                output: serde_json::json!("fn main() {}"),
                duration_ms: 50,
                is_error: false,
            },
            &mut state,
        );
        handle_agent_event(
            AgentEvent::TokenUsage {
                input_tokens: 5000,
                output_tokens: 200,
                cache_read_tokens: None,
                cache_write_tokens: None,
                total_cost_usd: Some(0.01),
            },
            &mut state,
        );

        assert_eq!(state.state, TuiState::Running);
        assert_eq!(state.status.iteration, 1);
        // Text + tool call are split into separate messages (interleaved)
        assert_eq!(state.messages.len(), 2);
        assert_eq!(state.messages[0].text, "Working on it...");
        assert_eq!(state.messages[1].tool_calls.len(), 1);
        assert_eq!(
            state.messages[1].tool_calls[0].status,
            app::ToolCallStatus::Success
        );

        terminal
            .draw(|frame| {
                render(frame, &state);
            })
            .unwrap();
    }

    #[test]
    fn test_handle_slash_help() {
        let mut state = AppState::new();
        handle_slash_command(commands::CommandAction::Help, &mut state);
        assert!(!state.messages.is_empty());
        assert!(state
            .messages
            .last()
            .unwrap()
            .text
            .contains("Available commands"));
    }

    #[test]
    fn test_handle_slash_clear() {
        let mut state = AppState::new();
        state.messages.push(app::ChatMessage {
            role: app::MessageRole::User,
            text: "hello".to_string(),
            tool_calls: Vec::new(),
        });
        handle_slash_command(commands::CommandAction::Clear, &mut state);
        assert!(state.messages.is_empty());
    }

    #[test]
    fn test_handle_slash_context() {
        let mut state = AppState::new();
        state.status.input_tokens = 5000;
        state.status.output_tokens = 1000;
        handle_slash_command(commands::CommandAction::Context, &mut state);
        let msg = state.messages.last().unwrap();
        assert!(msg.text.contains("5000"));
    }
}
