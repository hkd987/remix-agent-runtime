use serde_json::Value;
use std::time::Instant;
use tracing::{debug, info, warn};

use crate::browser::mcp::ToolExecutor;
use crate::config::credentials::{inject_credentials_into_system_prompt, CredentialSet};
use crate::config::schema::{AgentConfig, CompactionConfig};
use crate::error::AgentError;
use crate::llm::client::LlmProvider;
use crate::llm::types::{ContentBlock, StopReason};
use crate::output::result::{AgentResult, AgentStatus, StepRecord};
use crate::session::types::{SessionId, SessionStatus};
use crate::session::SessionStore;
use crate::skills::{inject_skills_into_system_prompt, SkillSet};

use super::compaction;
use super::compaction_prompt::COMPACTION_SYSTEM_PROMPT;
use super::state::AgentState;

pub struct AgentRunner<L: LlmProvider, T: ToolExecutor> {
    llm: L,
    tools: T,
    config: AgentConfig,
}

impl<L: LlmProvider, T: ToolExecutor> AgentRunner<L, T> {
    pub fn new(llm: L, tools: T, config: AgentConfig) -> Self {
        Self { llm, tools, config }
    }

    /// Consume the runner and return the inner tool executor for cleanup.
    pub fn into_tools(self) -> T {
        self.tools
    }

    pub async fn run(
        &self,
        task: &str,
        credentials: &CredentialSet,
        skill_set: &SkillSet,
        agents_md: &Option<crate::agents_md::AgentsMdContent>,
    ) -> Result<AgentResult, AgentError> {
        self.run_with_options(task, credentials, skill_set, agents_md, None, None)
            .await
    }

    /// Run the agent loop with optional session persistence and context compaction.
    pub async fn run_with_options(
        &self,
        task: &str,
        credentials: &CredentialSet,
        skill_set: &SkillSet,
        agents_md: &Option<crate::agents_md::AgentsMdContent>,
        session_store: Option<&SessionStore>,
        compaction_config: Option<&CompactionConfig>,
    ) -> Result<AgentResult, AgentError> {
        let mut state = AgentState::new(task);

        // Create session if store is provided
        let session_metadata = if let Some(store) = session_store {
            match store.create(task) {
                Ok(metadata) => {
                    info!(session_id = %metadata.id, "Created new session");
                    Some(metadata)
                }
                Err(e) => {
                    warn!(error = %e, "Failed to create session, continuing without persistence");
                    None
                }
            }
        } else {
            None
        };

        let mut system_parts = Vec::new();
        if let Some(ref prompt) = self.config.system_prompt {
            system_parts.push(prompt.clone());
        }
        if let Some(agents_md_prompt) =
            crate::agents_md::inject_agents_md_into_system_prompt(agents_md)
        {
            system_parts.push(agents_md_prompt);
        }
        if let Some(cred_prompt) = inject_credentials_into_system_prompt(credentials) {
            system_parts.push(cred_prompt);
        }
        if let Some(skill_prompt) = inject_skills_into_system_prompt(skill_set) {
            system_parts.push(skill_prompt);
        }
        let system_prompt = if system_parts.is_empty() {
            None
        } else {
            Some(system_parts.join("\n\n"))
        };

        let tool_defs = self.tools.tool_definitions();
        let timeout_ms = self.config.timeout_secs * 1000;

        loop {
            if state.current_iteration() >= self.config.max_iterations {
                info!(
                    iterations = state.current_iteration(),
                    "Max iterations reached"
                );
                self.finalize_session(
                    session_store,
                    &session_metadata,
                    &state,
                    SessionStatus::Completed,
                );
                return Ok(state.into_result(AgentStatus::MaxIterations, None));
            }

            if state.elapsed_ms() >= timeout_ms {
                info!(elapsed_ms = state.elapsed_ms(), "Timeout reached");
                self.finalize_session(
                    session_store,
                    &session_metadata,
                    &state,
                    SessionStatus::Completed,
                );
                return Ok(state.into_result(AgentStatus::Timeout, None));
            }

            // Check if compaction is needed before sending to LLM
            if let Some(compact_config) = compaction_config {
                if compaction::should_compact(compact_config, state.total_input_tokens()) {
                    info!(
                        input_tokens = state.total_input_tokens(),
                        "Triggering context compaction"
                    );
                    let (compact_end, _) = compaction::compute_compaction_split(
                        state.messages().len(),
                        compact_config.preserve_recent_n,
                    );
                    if compact_end > 0 {
                        let messages_to_compact = &state.messages()[..compact_end];
                        let compaction_messages =
                            compaction::build_compaction_request(messages_to_compact);
                        match self
                            .llm
                            .send_messages(
                                Some(COMPACTION_SYSTEM_PROMPT),
                                &compaction_messages,
                                None,
                            )
                            .await
                        {
                            Ok(summary_response) => {
                                let summary_text = summary_response
                                    .content
                                    .iter()
                                    .filter_map(|b| match b {
                                        ContentBlock::Text { text } => Some(text.clone()),
                                        _ => None,
                                    })
                                    .collect::<Vec<_>>()
                                    .join("\n");
                                state.compact(&summary_text, compact_config.preserve_recent_n);
                                info!("Context compacted successfully");
                            }
                            Err(e) => {
                                warn!(error = %e, "Context compaction failed, continuing without");
                            }
                        }
                    }
                }
            }

            state.increment_iteration();
            debug!(iteration = state.current_iteration(), "Starting iteration");

            let response = self
                .llm
                .send_messages(system_prompt.as_deref(), state.messages(), Some(tool_defs))
                .await?;

            state.accumulate_usage(response.usage.as_ref());

            match response.stop_reason {
                StopReason::ToolUse => {
                    let assistant_content = response.content.clone();

                    // Check if there are actually tool_use blocks; if not, treat as end_turn.
                    let has_tool_use = assistant_content
                        .iter()
                        .any(|b| matches!(b, ContentBlock::ToolUse { .. }));
                    if !has_tool_use {
                        warn!("LLM returned stop_reason=tool_use but no tool_use blocks; treating as end_turn");
                        let final_text = assistant_content
                            .iter()
                            .filter_map(|block| match block {
                                ContentBlock::Text { text } => Some(text.clone()),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        state.add_assistant_message(assistant_content);
                        self.finalize_session(
                            session_store,
                            &session_metadata,
                            &state,
                            SessionStatus::Completed,
                        );
                        return Ok(state.into_result(AgentStatus::Success, Some(final_text)));
                    }

                    state.add_assistant_message(assistant_content.clone());

                    let mut tool_results = Vec::new();
                    for block in &assistant_content {
                        if let ContentBlock::ToolUse { id, name, input } = block {
                            debug!(tool = %name, "Executing tool");
                            let step_start = Instant::now();

                            let exec_result = self.tools.execute_tool(name, input.clone()).await;

                            let step_duration = step_start.elapsed().as_millis() as u64;

                            match exec_result {
                                Ok(result) => {
                                    state.record_step(StepRecord {
                                        iteration: state.current_iteration(),
                                        tool: name.clone(),
                                        input: input.clone(),
                                        output: serde_json::from_str(&result.content)
                                            .unwrap_or_else(|_| {
                                                Value::String(result.content.clone())
                                            }),
                                        duration_ms: step_duration,
                                        is_error: if result.is_error { Some(true) } else { None },
                                    });

                                    tool_results.push(ContentBlock::ToolResult {
                                        tool_use_id: id.clone(),
                                        content: result.content,
                                        is_error: if result.is_error { Some(true) } else { None },
                                    });
                                }
                                Err(e) => {
                                    warn!(tool = %name, error = %e, "Tool execution failed");
                                    let error_msg = e.to_string();
                                    state.record_step(StepRecord {
                                        iteration: state.current_iteration(),
                                        tool: name.clone(),
                                        input: input.clone(),
                                        output: Value::String(error_msg.clone()),
                                        duration_ms: step_duration,
                                        is_error: Some(true),
                                    });

                                    tool_results.push(ContentBlock::ToolResult {
                                        tool_use_id: id.clone(),
                                        content: error_msg,
                                        is_error: Some(true),
                                    });
                                }
                            }
                        }
                    }

                    state.add_tool_results(tool_results);

                    // Persist to session after each tool use iteration
                    self.persist_session_iteration(session_store, &session_metadata, &state);
                }
                StopReason::EndTurn | StopReason::MaxTokens | StopReason::StopSequence => {
                    let final_text = response
                        .content
                        .iter()
                        .filter_map(|block| match block {
                            ContentBlock::Text { text } => Some(text.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");

                    state.add_assistant_message(response.content);

                    self.finalize_session(
                        session_store,
                        &session_metadata,
                        &state,
                        SessionStatus::Completed,
                    );
                    return Ok(state.into_result(AgentStatus::Success, Some(final_text)));
                }
            }
        }
    }

    /// Resume an existing session and continue the agent loop.
    pub async fn resume(
        &self,
        session_store: &SessionStore,
        session_id: &SessionId,
        credentials: &CredentialSet,
        skill_set: &SkillSet,
        agents_md: &Option<crate::agents_md::AgentsMdContent>,
        compaction_config: Option<&CompactionConfig>,
    ) -> Result<AgentResult, AgentError> {
        let snapshot = session_store.load(session_id)?;
        info!(
            session_id = %session_id,
            iteration = snapshot.iteration,
            messages = snapshot.messages.len(),
            "Resuming session"
        );

        // Restore state from snapshot
        let mut state = AgentState::from_snapshot(&snapshot);

        let mut system_parts = Vec::new();
        if let Some(ref prompt) = snapshot.system_prompt {
            system_parts.push(prompt.clone());
        } else if let Some(ref prompt) = self.config.system_prompt {
            system_parts.push(prompt.clone());
        }
        if let Some(agents_md_prompt) =
            crate::agents_md::inject_agents_md_into_system_prompt(agents_md)
        {
            system_parts.push(agents_md_prompt);
        }
        if let Some(cred_prompt) = inject_credentials_into_system_prompt(credentials) {
            system_parts.push(cred_prompt);
        }
        if let Some(skill_prompt) = inject_skills_into_system_prompt(skill_set) {
            system_parts.push(skill_prompt);
        }
        let system_prompt = if system_parts.is_empty() {
            None
        } else {
            Some(system_parts.join("\n\n"))
        };

        let tool_defs = self.tools.tool_definitions();
        let timeout_ms = self.config.timeout_secs * 1000;
        let session_metadata = session_store.load_metadata(session_id).ok();

        loop {
            if state.current_iteration() >= self.config.max_iterations {
                self.finalize_session(
                    Some(session_store),
                    &session_metadata,
                    &state,
                    SessionStatus::Completed,
                );
                return Ok(state.into_result(AgentStatus::MaxIterations, None));
            }

            if state.elapsed_ms() >= timeout_ms {
                self.finalize_session(
                    Some(session_store),
                    &session_metadata,
                    &state,
                    SessionStatus::Completed,
                );
                return Ok(state.into_result(AgentStatus::Timeout, None));
            }

            // Check compaction
            if let Some(compact_config) = compaction_config {
                if compaction::should_compact(compact_config, state.total_input_tokens()) {
                    let (compact_end, _) = compaction::compute_compaction_split(
                        state.messages().len(),
                        compact_config.preserve_recent_n,
                    );
                    if compact_end > 0 {
                        let messages_to_compact = &state.messages()[..compact_end];
                        let compaction_messages =
                            compaction::build_compaction_request(messages_to_compact);
                        if let Ok(summary_response) = self
                            .llm
                            .send_messages(
                                Some(COMPACTION_SYSTEM_PROMPT),
                                &compaction_messages,
                                None,
                            )
                            .await
                        {
                            let summary_text = summary_response
                                .content
                                .iter()
                                .filter_map(|b| match b {
                                    ContentBlock::Text { text } => Some(text.clone()),
                                    _ => None,
                                })
                                .collect::<Vec<_>>()
                                .join("\n");
                            state.compact(&summary_text, compact_config.preserve_recent_n);
                        }
                    }
                }
            }

            state.increment_iteration();
            let response = self
                .llm
                .send_messages(system_prompt.as_deref(), state.messages(), Some(tool_defs))
                .await?;

            state.accumulate_usage(response.usage.as_ref());

            match response.stop_reason {
                StopReason::ToolUse => {
                    let assistant_content = response.content.clone();
                    let has_tool_use = assistant_content
                        .iter()
                        .any(|b| matches!(b, ContentBlock::ToolUse { .. }));
                    if !has_tool_use {
                        let final_text = assistant_content
                            .iter()
                            .filter_map(|block| match block {
                                ContentBlock::Text { text } => Some(text.clone()),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        state.add_assistant_message(assistant_content);
                        self.finalize_session(
                            Some(session_store),
                            &session_metadata,
                            &state,
                            SessionStatus::Completed,
                        );
                        return Ok(state.into_result(AgentStatus::Success, Some(final_text)));
                    }

                    state.add_assistant_message(assistant_content.clone());
                    let mut tool_results = Vec::new();
                    for block in &assistant_content {
                        if let ContentBlock::ToolUse { id, name, input } = block {
                            let step_start = Instant::now();
                            let exec_result = self.tools.execute_tool(name, input.clone()).await;
                            let step_duration = step_start.elapsed().as_millis() as u64;
                            match exec_result {
                                Ok(result) => {
                                    state.record_step(StepRecord {
                                        iteration: state.current_iteration(),
                                        tool: name.clone(),
                                        input: input.clone(),
                                        output: serde_json::from_str(&result.content)
                                            .unwrap_or_else(|_| {
                                                Value::String(result.content.clone())
                                            }),
                                        duration_ms: step_duration,
                                        is_error: if result.is_error { Some(true) } else { None },
                                    });
                                    tool_results.push(ContentBlock::ToolResult {
                                        tool_use_id: id.clone(),
                                        content: result.content,
                                        is_error: if result.is_error { Some(true) } else { None },
                                    });
                                }
                                Err(e) => {
                                    let error_msg = e.to_string();
                                    state.record_step(StepRecord {
                                        iteration: state.current_iteration(),
                                        tool: name.clone(),
                                        input: input.clone(),
                                        output: Value::String(error_msg.clone()),
                                        duration_ms: step_duration,
                                        is_error: Some(true),
                                    });
                                    tool_results.push(ContentBlock::ToolResult {
                                        tool_use_id: id.clone(),
                                        content: error_msg,
                                        is_error: Some(true),
                                    });
                                }
                            }
                        }
                    }
                    state.add_tool_results(tool_results);
                    self.persist_session_iteration(Some(session_store), &session_metadata, &state);
                }
                StopReason::EndTurn | StopReason::MaxTokens | StopReason::StopSequence => {
                    let final_text = response
                        .content
                        .iter()
                        .filter_map(|block| match block {
                            ContentBlock::Text { text } => Some(text.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    state.add_assistant_message(response.content);
                    self.finalize_session(
                        Some(session_store),
                        &session_metadata,
                        &state,
                        SessionStatus::Completed,
                    );
                    return Ok(state.into_result(AgentStatus::Success, Some(final_text)));
                }
            }
        }
    }

    /// Persist session state after each iteration (best-effort, failures logged and ignored).
    fn persist_session_iteration(
        &self,
        session_store: Option<&SessionStore>,
        session_metadata: &Option<crate::session::types::SessionMetadata>,
        state: &AgentState,
    ) {
        if let (Some(store), Some(metadata)) = (session_store, session_metadata) {
            if let Err(e) = store.append_messages(&metadata.id, state.messages()) {
                warn!(error = %e, "Failed to persist session messages");
            }
            if let Err(e) = store.save_steps(&metadata.id, state.steps()) {
                warn!(error = %e, "Failed to persist session steps");
            }
        }
    }

    /// Finalize session with final status (best-effort).
    fn finalize_session(
        &self,
        session_store: Option<&SessionStore>,
        session_metadata: &Option<crate::session::types::SessionMetadata>,
        state: &AgentState,
        status: SessionStatus,
    ) {
        if let (Some(store), Some(metadata)) = (session_store, session_metadata) {
            let mut updated = metadata.clone();
            updated.status = status;
            updated.updated_at = chrono::Utc::now();
            updated.total_input_tokens = if state.total_input_tokens() > 0 {
                Some(state.total_input_tokens())
            } else {
                None
            };
            updated.total_output_tokens = if state.total_output_tokens() > 0 {
                Some(state.total_output_tokens())
            } else {
                None
            };
            if let Err(e) = store.save_metadata(&updated) {
                warn!(error = %e, "Failed to finalize session");
            }
            if let Err(e) = store.save_steps(&metadata.id, state.steps()) {
                warn!(error = %e, "Failed to save final steps");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::mcp::ToolExecutionResult;
    use crate::llm::types::{Message, MessagesResponse, ToolDefinition};
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    struct MockLlm {
        responses: Arc<Mutex<Vec<MessagesResponse>>>,
    }

    #[async_trait]
    impl LlmProvider for MockLlm {
        async fn send_messages(
            &self,
            _system: Option<&str>,
            _messages: &[Message],
            _tools: Option<&[ToolDefinition]>,
        ) -> Result<MessagesResponse, AgentError> {
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                Err(AgentError::Llm("No more mock responses".into()))
            } else {
                Ok(responses.remove(0))
            }
        }
    }

    struct MockTools {
        tools: Vec<ToolDefinition>,
        results: Arc<Mutex<Vec<Result<ToolExecutionResult, AgentError>>>>,
    }

    #[async_trait]
    impl ToolExecutor for MockTools {
        fn tool_definitions(&self) -> &[ToolDefinition] {
            &self.tools
        }

        async fn execute_tool(
            &self,
            _name: &str,
            _args: Value,
        ) -> Result<ToolExecutionResult, AgentError> {
            let mut results = self.results.lock().unwrap();
            if results.is_empty() {
                Ok(ToolExecutionResult {
                    content: "ok".to_string(),
                    is_error: false,
                })
            } else {
                results.remove(0)
            }
        }
    }

    fn make_end_turn_response(text: &str) -> MessagesResponse {
        MessagesResponse {
            id: "msg_test".to_string(),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            model: "test-model".to_string(),
            stop_reason: StopReason::EndTurn,
            usage: None,
        }
    }

    fn make_tool_use_response(tool_id: &str, tool_name: &str, input: Value) -> MessagesResponse {
        MessagesResponse {
            id: "msg_test".to_string(),
            content: vec![ContentBlock::ToolUse {
                id: tool_id.to_string(),
                name: tool_name.to_string(),
                input,
            }],
            model: "test-model".to_string(),
            stop_reason: StopReason::ToolUse,
            usage: None,
        }
    }

    fn default_config() -> AgentConfig {
        AgentConfig {
            max_iterations: 10,
            system_prompt: None,
            timeout_secs: 300,
        }
    }

    fn default_tools() -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "navigate".to_string(),
            description: "Navigate to URL".to_string(),
            input_schema: json!({"type": "object", "properties": {"url": {"type": "string"}}}),
        }]
    }

    #[tokio::test]
    async fn test_success_path_with_tool_use_then_end_turn() {
        let llm = MockLlm {
            responses: Arc::new(Mutex::new(vec![
                make_tool_use_response(
                    "toolu_01",
                    "navigate",
                    json!({"url": "https://example.com"}),
                ),
                make_end_turn_response("Task completed successfully"),
            ])),
        };
        let tools = MockTools {
            tools: default_tools(),
            results: Arc::new(Mutex::new(vec![Ok(ToolExecutionResult {
                content: "Page loaded".to_string(),
                is_error: false,
            })])),
        };

        let runner = AgentRunner::new(llm, tools, default_config());
        let result = runner
            .run(
                "Navigate to example.com",
                &CredentialSet::new(),
                &SkillSet::new(),
                &None,
            )
            .await
            .unwrap();

        assert_eq!(result.status, AgentStatus::Success);
        assert_eq!(
            result.result,
            Some("Task completed successfully".to_string())
        );
        assert_eq!(result.steps.len(), 1);
        assert_eq!(result.steps[0].tool, "navigate");
        assert_eq!(result.total_iterations, 2);
    }

    #[tokio::test]
    async fn test_immediate_completion() {
        let llm = MockLlm {
            responses: Arc::new(Mutex::new(vec![make_end_turn_response("Nothing to do")])),
        };
        let tools = MockTools {
            tools: default_tools(),
            results: Arc::new(Mutex::new(vec![])),
        };

        let runner = AgentRunner::new(llm, tools, default_config());
        let result = runner
            .run(
                "Just say hello",
                &CredentialSet::new(),
                &SkillSet::new(),
                &None,
            )
            .await
            .unwrap();

        assert_eq!(result.status, AgentStatus::Success);
        assert_eq!(result.result, Some("Nothing to do".to_string()));
        assert!(result.steps.is_empty());
        assert_eq!(result.total_iterations, 1);
    }

    #[tokio::test]
    async fn test_max_iterations_reached() {
        let config = AgentConfig {
            max_iterations: 2,
            system_prompt: None,
            timeout_secs: 300,
        };

        // LLM always returns tool_use, so we hit max_iterations
        let llm = MockLlm {
            responses: Arc::new(Mutex::new(vec![
                make_tool_use_response("toolu_01", "navigate", json!({"url": "a.com"})),
                make_tool_use_response("toolu_02", "navigate", json!({"url": "b.com"})),
            ])),
        };
        let tools = MockTools {
            tools: default_tools(),
            results: Arc::new(Mutex::new(vec![])),
        };

        let runner = AgentRunner::new(llm, tools, config);
        let result = runner
            .run(
                "Loop forever",
                &CredentialSet::new(),
                &SkillSet::new(),
                &None,
            )
            .await
            .unwrap();

        assert_eq!(result.status, AgentStatus::MaxIterations);
        assert_eq!(result.total_iterations, 2);
        assert_eq!(result.error, Some("Max iterations (2) reached".to_string()));
    }

    #[tokio::test]
    async fn test_tool_error_recovery() {
        let llm = MockLlm {
            responses: Arc::new(Mutex::new(vec![
                make_tool_use_response("toolu_01", "navigate", json!({"url": "bad.com"})),
                make_end_turn_response("Recovered from error"),
            ])),
        };
        let tools = MockTools {
            tools: default_tools(),
            results: Arc::new(Mutex::new(vec![Err(AgentError::ToolExecution(
                "Connection refused".to_string(),
            ))])),
        };

        let runner = AgentRunner::new(llm, tools, default_config());
        let result = runner
            .run(
                "Navigate to bad.com",
                &CredentialSet::new(),
                &SkillSet::new(),
                &None,
            )
            .await
            .unwrap();

        assert_eq!(result.status, AgentStatus::Success);
        assert_eq!(result.steps.len(), 1);
        assert_eq!(result.steps[0].is_error, Some(true));
        assert_eq!(
            result.steps[0].output,
            Value::String("Tool execution error: Connection refused".to_string())
        );
        assert_eq!(result.total_iterations, 2);
    }

    #[tokio::test]
    async fn test_multiple_tool_calls_in_one_response() {
        let response = MessagesResponse {
            id: "msg_test".to_string(),
            content: vec![
                ContentBlock::Text {
                    text: "I'll use two tools.".to_string(),
                },
                ContentBlock::ToolUse {
                    id: "toolu_01".to_string(),
                    name: "navigate".to_string(),
                    input: json!({"url": "https://a.com"}),
                },
                ContentBlock::ToolUse {
                    id: "toolu_02".to_string(),
                    name: "navigate".to_string(),
                    input: json!({"url": "https://b.com"}),
                },
            ],
            model: "test-model".to_string(),
            stop_reason: StopReason::ToolUse,
            usage: None,
        };

        let llm = MockLlm {
            responses: Arc::new(Mutex::new(vec![
                response,
                make_end_turn_response("Both navigations done"),
            ])),
        };
        let tools = MockTools {
            tools: default_tools(),
            results: Arc::new(Mutex::new(vec![
                Ok(ToolExecutionResult {
                    content: "Page A loaded".to_string(),
                    is_error: false,
                }),
                Ok(ToolExecutionResult {
                    content: "Page B loaded".to_string(),
                    is_error: false,
                }),
            ])),
        };

        let runner = AgentRunner::new(llm, tools, default_config());
        let result = runner
            .run(
                "Open two pages",
                &CredentialSet::new(),
                &SkillSet::new(),
                &None,
            )
            .await
            .unwrap();

        assert_eq!(result.status, AgentStatus::Success);
        assert_eq!(result.steps.len(), 2);
        assert_eq!(result.steps[0].tool, "navigate");
        assert_eq!(result.steps[1].tool, "navigate");
        assert_eq!(result.total_iterations, 2);
    }

    #[tokio::test]
    async fn test_system_prompt_with_credentials() {
        let config = AgentConfig {
            max_iterations: 10,
            system_prompt: Some("You are a browser agent.".to_string()),
            timeout_secs: 300,
        };

        let llm_responses = Arc::new(Mutex::new(vec![make_end_turn_response("Done")]));
        let llm = MockLlm {
            responses: llm_responses,
        };
        let tools = MockTools {
            tools: default_tools(),
            results: Arc::new(Mutex::new(vec![])),
        };

        let mut credentials = CredentialSet::new();
        credentials.add(
            remix_credentials::Credential::new(
                "test_cred".to_string(),
                remix_credentials::CredentialType::UsernamePassword,
                vec![
                    ("username".to_string(), "admin".to_string()),
                    ("password".to_string(), "pass".to_string()),
                ],
                Some("*.example.com".to_string()),
                Default::default(),
            )
            .unwrap(),
        );

        let runner = AgentRunner::new(llm, tools, config);
        let result = runner
            .run(
                "Login to example.com",
                &credentials,
                &SkillSet::new(),
                &None,
            )
            .await
            .unwrap();

        assert_eq!(result.status, AgentStatus::Success);
    }

    #[tokio::test]
    async fn test_llm_error_propagates() {
        let llm = MockLlm {
            responses: Arc::new(Mutex::new(vec![])),
        };
        let tools = MockTools {
            tools: default_tools(),
            results: Arc::new(Mutex::new(vec![])),
        };

        let runner = AgentRunner::new(llm, tools, default_config());
        let result = runner
            .run("Fail", &CredentialSet::new(), &SkillSet::new(), &None)
            .await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), AgentError::Llm(_)));
    }

    #[tokio::test]
    async fn test_tool_result_with_json_output() {
        let llm = MockLlm {
            responses: Arc::new(Mutex::new(vec![
                make_tool_use_response(
                    "toolu_01",
                    "navigate",
                    json!({"url": "https://example.com"}),
                ),
                make_end_turn_response("Done"),
            ])),
        };
        let tools = MockTools {
            tools: default_tools(),
            results: Arc::new(Mutex::new(vec![Ok(ToolExecutionResult {
                content: r#"{"title":"Example","url":"https://example.com"}"#.to_string(),
                is_error: false,
            })])),
        };

        let runner = AgentRunner::new(llm, tools, default_config());
        let result = runner
            .run("Navigate", &CredentialSet::new(), &SkillSet::new(), &None)
            .await
            .unwrap();

        // JSON content should be parsed into a Value::Object, not a string
        assert_eq!(
            result.steps[0].output,
            json!({"title":"Example","url":"https://example.com"})
        );
    }

    #[tokio::test]
    async fn test_token_usage_accumulated_across_iterations() {
        use crate::llm::types::Usage;

        let llm = MockLlm {
            responses: Arc::new(Mutex::new(vec![
                MessagesResponse {
                    id: "msg_1".to_string(),
                    content: vec![ContentBlock::ToolUse {
                        id: "toolu_01".to_string(),
                        name: "navigate".to_string(),
                        input: json!({"url": "https://example.com"}),
                    }],
                    model: "test-model".to_string(),
                    stop_reason: StopReason::ToolUse,
                    usage: Some(Usage {
                        input_tokens: 100,
                        output_tokens: 50,
                    }),
                },
                MessagesResponse {
                    id: "msg_2".to_string(),
                    content: vec![ContentBlock::Text {
                        text: "Done".to_string(),
                    }],
                    model: "test-model".to_string(),
                    stop_reason: StopReason::EndTurn,
                    usage: Some(Usage {
                        input_tokens: 200,
                        output_tokens: 30,
                    }),
                },
            ])),
        };
        let tools = MockTools {
            tools: default_tools(),
            results: Arc::new(Mutex::new(vec![Ok(ToolExecutionResult {
                content: "Page loaded".to_string(),
                is_error: false,
            })])),
        };

        let runner = AgentRunner::new(llm, tools, default_config());
        let result = runner
            .run("Navigate", &CredentialSet::new(), &SkillSet::new(), &None)
            .await
            .unwrap();

        assert_eq!(result.total_input_tokens, Some(300));
        assert_eq!(result.total_output_tokens, Some(80));
    }

    #[tokio::test]
    async fn test_token_usage_none_when_no_usage_reported() {
        let llm = MockLlm {
            responses: Arc::new(Mutex::new(vec![make_end_turn_response("Done")])),
        };
        let tools = MockTools {
            tools: default_tools(),
            results: Arc::new(Mutex::new(vec![])),
        };

        let runner = AgentRunner::new(llm, tools, default_config());
        let result = runner
            .run("Hello", &CredentialSet::new(), &SkillSet::new(), &None)
            .await
            .unwrap();

        assert_eq!(result.total_input_tokens, None);
        assert_eq!(result.total_output_tokens, None);
    }

    #[tokio::test]
    async fn test_tool_use_stop_reason_without_tool_use_blocks() {
        // If the LLM returns stop_reason=tool_use but no actual tool_use blocks,
        // the agent should treat it as end_turn rather than creating empty messages.
        let llm = MockLlm {
            responses: Arc::new(Mutex::new(vec![MessagesResponse {
                id: "msg_test".to_string(),
                content: vec![ContentBlock::Text {
                    text: "I was going to use a tool but decided not to.".to_string(),
                }],
                model: "test-model".to_string(),
                stop_reason: StopReason::ToolUse,
                usage: None,
            }])),
        };
        let tools = MockTools {
            tools: default_tools(),
            results: Arc::new(Mutex::new(vec![])),
        };

        let runner = AgentRunner::new(llm, tools, default_config());
        let result = runner
            .run(
                "Do something",
                &CredentialSet::new(),
                &SkillSet::new(),
                &None,
            )
            .await
            .unwrap();

        assert_eq!(result.status, AgentStatus::Success);
        assert_eq!(
            result.result,
            Some("I was going to use a tool but decided not to.".to_string())
        );
        assert!(result.steps.is_empty());
        assert_eq!(result.total_iterations, 1);
    }

    #[tokio::test]
    async fn test_tool_use_stop_reason_with_empty_content() {
        // If the LLM returns stop_reason=tool_use with completely empty content,
        // the agent should treat it as end_turn.
        let llm = MockLlm {
            responses: Arc::new(Mutex::new(vec![MessagesResponse {
                id: "msg_test".to_string(),
                content: vec![],
                model: "test-model".to_string(),
                stop_reason: StopReason::ToolUse,
                usage: None,
            }])),
        };
        let tools = MockTools {
            tools: default_tools(),
            results: Arc::new(Mutex::new(vec![])),
        };

        let runner = AgentRunner::new(llm, tools, default_config());
        let result = runner
            .run(
                "Empty response",
                &CredentialSet::new(),
                &SkillSet::new(),
                &None,
            )
            .await
            .unwrap();

        assert_eq!(result.status, AgentStatus::Success);
        assert_eq!(result.result, Some(String::new()));
        assert!(result.steps.is_empty());
        assert_eq!(result.total_iterations, 1);
    }

    #[test]
    fn test_into_tools_returns_tool_executor() {
        let llm = MockLlm {
            responses: Arc::new(Mutex::new(vec![])),
        };
        let tools = MockTools {
            tools: default_tools(),
            results: Arc::new(Mutex::new(vec![])),
        };

        let runner = AgentRunner::new(llm, tools, default_config());
        let recovered_tools = runner.into_tools();
        assert_eq!(recovered_tools.tool_definitions().len(), 1);
        assert_eq!(recovered_tools.tool_definitions()[0].name, "navigate");
    }

    #[tokio::test]
    async fn test_skill_prompt_injection() {
        use crate::skills::{SkillEntry, SkillMetadata};
        use std::path::PathBuf;

        // Capture the system prompt passed to the LLM
        struct CapturingLlm {
            captured_system: Arc<Mutex<Option<String>>>,
        }

        #[async_trait]
        impl LlmProvider for CapturingLlm {
            async fn send_messages(
                &self,
                system: Option<&str>,
                _messages: &[Message],
                _tools: Option<&[ToolDefinition]>,
            ) -> Result<MessagesResponse, AgentError> {
                *self.captured_system.lock().unwrap() = system.map(|s| s.to_string());
                Ok(make_end_turn_response("Done"))
            }
        }

        let captured = Arc::new(Mutex::new(None));
        let llm = CapturingLlm {
            captured_system: captured.clone(),
        };
        let tools = MockTools {
            tools: default_tools(),
            results: Arc::new(Mutex::new(vec![])),
        };

        let mut skill_set = SkillSet::new();
        skill_set
            .insert(SkillEntry {
                metadata: SkillMetadata {
                    name: "test-skill".to_string(),
                    description: "A test skill".to_string(),
                    license: None,
                    compatibility: vec![],
                    metadata: Default::default(),
                    allowed_tools: vec![],
                },
                body: "Instructions".to_string(),
                dir_path: PathBuf::from("/tmp/test-skill"),
                skill_md_path: PathBuf::from("/tmp/test-skill/SKILL.md"),
            })
            .unwrap();

        let runner = AgentRunner::new(llm, tools, default_config());
        runner
            .run("Do task", &CredentialSet::new(), &skill_set, &None)
            .await
            .unwrap();

        let system = captured.lock().unwrap().clone().unwrap();
        assert!(system.contains("<available_skills>"));
        assert!(system.contains("test-skill"));
        assert!(system.contains("A test skill"));
        assert!(system.contains("load_skill"));
    }
}
