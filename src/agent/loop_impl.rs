use serde_json::Value;
use std::borrow::Cow;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, info, warn};

use crate::browser::mcp::ToolExecutor;
use crate::config::credentials::{inject_credentials_into_system_prompt, CredentialSet};
use crate::config::schema::{AgentConfig, CompactionConfig};
use crate::error::AgentError;
use crate::llm::client::LlmProvider;
use crate::llm::types::{
    CacheControl, ContentBlock, StopReason, SystemContent, ToolDefinition, ToolResultContent,
};
use crate::output::events::{self, AgentEvent, EventBus};
use crate::output::result::{AgentResult, AgentStatus, StepRecord};
use crate::session::types::{SessionId, SessionStatus};
use crate::session::SessionStorage;
use crate::skills::{inject_skills_into_system_prompt, SkillSet};

use super::compaction;
use super::compaction_prompt::COMPACTION_SYSTEM_PROMPT;
use super::compaction_stages;
use super::reminders::ReminderScheduler;
use super::self_critique;
use super::state::AgentState;
use super::tool_registry::ToolRegistry;

/// Tools that are always visible in lazy discovery mode (core local tools).
const ALWAYS_AVAILABLE_TOOLS: &[&str] = &[
    "read_file",
    "write_file",
    "edit_file",
    "bash",
    "grep",
    "glob",
];

pub struct AgentRunner<L: LlmProvider, T: ToolExecutor> {
    llm: L,
    tools: T,
    config: AgentConfig,
    event_bus: Option<Arc<EventBus>>,
}

impl<L: LlmProvider, T: ToolExecutor> AgentRunner<L, T> {
    pub fn new(llm: L, tools: T, config: AgentConfig) -> Self {
        Self {
            llm,
            tools,
            config,
            event_bus: None,
        }
    }

    /// Attach an event bus for real-time event streaming.
    pub fn with_event_bus(mut self, event_bus: Arc<EventBus>) -> Self {
        self.event_bus = Some(event_bus);
        self
    }

    /// Consume the runner and return the inner tool executor for cleanup.
    pub fn into_tools(self) -> T {
        self.tools
    }

    /// Get a reference to the inner tool executor (non-consuming).
    pub fn tools_ref(&self) -> &T {
        &self.tools
    }

    /// Returns `true` for tools that only read state.
    /// Used for plan mode filtering and potential future parallel execution.
    pub fn is_read_only_tool(name: &str) -> bool {
        matches!(
            name,
            "read_file"
                | "grep"
                | "glob"
                | "list_files"
                | "screenshot"
                | "get_page_info"
                | "get_console_logs"
                | "get_page_content"
                | "search_tools"
        )
    }

    /// Filter tool definitions to only include read-only tools (for plan mode).
    fn filter_read_only_tools(tools: &[ToolDefinition]) -> Vec<ToolDefinition> {
        tools
            .iter()
            .filter(|t| t.read_only || Self::is_read_only_tool(&t.name))
            .cloned()
            .collect()
    }

    /// Execute tool_use blocks from an assistant response sequentially.
    ///
    /// Returns `(tool_results, step_records)` preserving the original block order.
    #[cfg(test)]
    async fn execute_tools(
        &self,
        assistant_content: &[ContentBlock],
        iteration: u32,
    ) -> (Vec<ContentBlock>, Vec<StepRecord>) {
        let mut tool_results = Vec::new();
        let mut step_records = Vec::new();

        for block in assistant_content {
            match block {
                ContentBlock::ToolUse { id, name, input } => {
                    debug!(tool = %name, "Executing tool");
                    events::emit(
                        &self.event_bus,
                        AgentEvent::ToolUseStart {
                            id: id.clone(),
                            name: name.clone(),
                            input: input.clone(),
                        },
                    );
                    let step_start = Instant::now();
                    let exec_result = self.tools.execute_tool(name, input.clone()).await;
                    let step_duration = step_start.elapsed().as_millis() as u64;
                    let (content_block, step) = Self::process_tool_result(
                        id,
                        name,
                        input,
                        exec_result,
                        step_duration,
                        iteration,
                    );
                    events::emit(
                        &self.event_bus,
                        AgentEvent::ToolUseResult {
                            id: id.clone(),
                            name: name.clone(),
                            output: step.output.clone(),
                            duration_ms: step.duration_ms,
                            is_error: step.is_error.unwrap_or(false),
                        },
                    );
                    tool_results.push(content_block);
                    step_records.push(step);
                }
                ContentBlock::Thinking { thinking } => {
                    debug!(
                        thinking_length = thinking.len(),
                        "Model produced thinking block"
                    );
                    events::emit(
                        &self.event_bus,
                        AgentEvent::ThinkingComplete {
                            thinking: thinking.clone(),
                        },
                    );
                }
                _ => {}
            }
        }

        (tool_results, step_records)
    }

    /// Process a single tool execution result into a ContentBlock and StepRecord.
    fn process_tool_result(
        id: &str,
        name: &str,
        input: &Value,
        exec_result: Result<crate::browser::mcp::ToolExecutionResult, AgentError>,
        step_duration: u64,
        iteration: u32,
    ) -> (ContentBlock, StepRecord) {
        match exec_result {
            Ok(result) => {
                let step = StepRecord {
                    iteration,
                    tool: name.to_string(),
                    input: input.clone(),
                    output: serde_json::from_str(&result.content)
                        .unwrap_or_else(|_| Value::String(result.content.clone())),
                    duration_ms: step_duration,
                    is_error: if result.is_error { Some(true) } else { None },
                };
                let block = ContentBlock::ToolResult {
                    tool_use_id: id.to_string(),
                    content: ToolResultContent::Text(result.content),
                    is_error: if result.is_error { Some(true) } else { None },
                };
                (block, step)
            }
            Err(e) => {
                warn!(tool = %name, error = %e, "Tool execution failed");
                let error_msg = e.to_string();
                let step = StepRecord {
                    iteration,
                    tool: name.to_string(),
                    input: input.clone(),
                    output: Value::String(error_msg.clone()),
                    duration_ms: step_duration,
                    is_error: Some(true),
                };
                let block = ContentBlock::ToolResult {
                    tool_use_id: id.to_string(),
                    content: ToolResultContent::Text(error_msg),
                    is_error: Some(true),
                };
                (block, step)
            }
        }
    }

    pub async fn run(
        &self,
        task: &str,
        credentials: &CredentialSet,
        skill_set: &SkillSet,
        agents_md: &Option<crate::agents_md::AgentsMdContent>,
    ) -> Result<AgentResult, AgentError> {
        self.run_with_options(task, credentials, skill_set, agents_md, None, None, None)
            .await
    }

    /// Run the agent loop with optional session persistence, context compaction,
    /// and a dedicated compaction LLM.
    #[allow(clippy::too_many_arguments)]
    pub async fn run_with_options(
        &self,
        task: &str,
        credentials: &CredentialSet,
        skill_set: &SkillSet,
        agents_md: &Option<crate::agents_md::AgentsMdContent>,
        session_store: Option<&dyn SessionStorage>,
        compaction_config: Option<&CompactionConfig>,
        compaction_llm: Option<&dyn LlmProvider>,
    ) -> Result<AgentResult, AgentError> {
        let mut state = AgentState::new(task);

        events::emit(
            &self.event_bus,
            AgentEvent::AgentStarted {
                task: task.to_string(),
                timestamp_ms: chrono::Utc::now().timestamp_millis() as u64,
            },
        );

        // Create session if store is provided
        let session_metadata = if let Some(store) = session_store {
            match store.create(task).await {
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

        let mut system_blocks: Vec<SystemContent> = Vec::new();
        if let Some(ref prompt) = self.config.system_prompt {
            system_blocks.push(SystemContent::text(prompt.clone()));
        }
        if let Some(agents_md_prompt) =
            crate::agents_md::inject_agents_md_into_system_prompt(agents_md)
        {
            system_blocks.push(SystemContent::text(agents_md_prompt));
        }
        if let Some(cred_prompt) = inject_credentials_into_system_prompt(credentials) {
            system_blocks.push(SystemContent::text(cred_prompt));
        }
        if let Some(ref coord_config) = self.config.coordination_config {
            if let Some(coord_prompt) =
                crate::coordination::inject_coordination_into_system_prompt(coord_config)
            {
                system_blocks.push(SystemContent::text(coord_prompt));
            }
        }
        if let Some(skill_prompt) = inject_skills_into_system_prompt(skill_set) {
            system_blocks.push(SystemContent::text(skill_prompt));
        }
        // Mark the last block as cached
        if let Some(last) = system_blocks.last_mut() {
            match last {
                SystemContent::Text { cache_control, .. } => {
                    *cache_control = Some(CacheControl::ephemeral());
                }
            }
        }
        let system_prompt = if system_blocks.is_empty() {
            None
        } else {
            Some(system_blocks)
        };

        // Feature 1: Lazy tool discovery — build registry only when needed
        let all_tool_defs = self.tools.tool_definitions().to_vec();
        let mut tool_registry = if self.config.lazy_tool_discovery {
            let always_avail = ALWAYS_AVAILABLE_TOOLS
                .iter()
                .map(|s| s.to_string())
                .collect();
            Some(ToolRegistry::new(all_tool_defs.clone(), always_avail))
        } else {
            None
        };

        // Feature 5: System reminders — build scheduler
        let reminder_scheduler = ReminderScheduler::from_configs(&self.config.reminders);

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
                )
                .await;
                self.emit_completed(AgentStatus::MaxIterations, None, &state);
                return Ok(state.into_result(AgentStatus::MaxIterations, None));
            }

            if state.elapsed_ms() >= timeout_ms {
                info!(elapsed_ms = state.elapsed_ms(), "Timeout reached");
                self.finalize_session(
                    session_store,
                    &session_metadata,
                    &state,
                    SessionStatus::Completed,
                )
                .await;
                self.emit_completed(AgentStatus::Timeout, None, &state);
                return Ok(state.into_result(AgentStatus::Timeout, None));
            }

            // Feature 3: Progressive context compaction
            if let Some(compact_config) = compaction_config {
                self.run_compaction(compact_config, &mut state, compaction_llm)
                    .await;
            }

            state.increment_iteration();
            debug!(iteration = state.current_iteration(), "Starting iteration");

            events::emit(
                &self.event_bus,
                AgentEvent::IterationStarted {
                    iteration: state.current_iteration(),
                },
            );

            // Feature 5: Check system reminders before LLM call
            if !reminder_scheduler.is_empty() {
                let pending_tools: Vec<String> = Vec::new(); // pre-execution, no pending tools yet
                let reminders = reminder_scheduler.check(state.current_iteration(), &pending_tools);
                for reminder in reminders {
                    state.inject_system_notification(&reminder);
                }
            }

            // Feature 1 + 4: Resolve tool definitions based on mode
            let effective_tool_defs: Cow<'_, [ToolDefinition]> =
                if let Some(ref registry) = tool_registry {
                    let discovered = registry.discovered_definitions();
                    if self.config.plan_mode {
                        Cow::Owned(Self::filter_read_only_tools(&discovered))
                    } else {
                        Cow::Owned(discovered)
                    }
                } else if self.config.plan_mode {
                    Cow::Owned(Self::filter_read_only_tools(&all_tool_defs))
                } else {
                    Cow::Borrowed(&all_tool_defs)
                };

            let response = self
                .llm
                .send_messages(
                    system_prompt.as_deref(),
                    state.messages(),
                    Some(&effective_tool_defs),
                )
                .await?;

            state.accumulate_usage(response.usage.as_ref(), &response.model);

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
                        )
                        .await;
                        self.emit_completed(AgentStatus::Success, Some(final_text.clone()), &state);
                        return Ok(state.into_result(AgentStatus::Success, Some(final_text)));
                    }

                    // Feature 5: Check reminders before tool execution
                    if !reminder_scheduler.is_empty() {
                        let pending_tool_names: Vec<String> = assistant_content
                            .iter()
                            .filter_map(|b| match b {
                                ContentBlock::ToolUse { name, .. } => Some(name.clone()),
                                _ => None,
                            })
                            .collect();
                        let tool_reminders = reminder_scheduler
                            .check(state.current_iteration(), &pending_tool_names);
                        for reminder in tool_reminders {
                            state.inject_system_notification(&reminder);
                        }
                    }

                    // Feature 6: Self-critique phase
                    if let Some(ref critique_config) = self.config.self_critique {
                        if self_critique::should_critique(critique_config, &assistant_content) {
                            debug!("Running self-critique on planned actions");
                            let critique_prompt =
                                self_critique::build_critique_prompt(&assistant_content, task);
                            let critique_system =
                                [SystemContent::text(self_critique::CRITIQUE_SYSTEM_PROMPT)];
                            let critique_messages = vec![crate::llm::types::Message {
                                role: crate::llm::types::Role::User,
                                content: vec![ContentBlock::Text {
                                    text: critique_prompt,
                                }],
                            }];
                            if let Ok(critique_response) = self
                                .llm
                                .send_messages(Some(&critique_system), &critique_messages, None)
                                .await
                            {
                                let critique_text = critique_response
                                    .content
                                    .iter()
                                    .filter_map(|b| match b {
                                        ContentBlock::Text { text } => Some(text.clone()),
                                        _ => None,
                                    })
                                    .collect::<Vec<_>>()
                                    .join("\n");
                                let result = self_critique::parse_critique_response(&critique_text);
                                if !result.approved {
                                    info!("Self-critique rejected actions: {}", result.reasoning);
                                    state.add_assistant_message(assistant_content);
                                    let feedback = format!(
                                        "[SELF_CRITIQUE] Your planned actions were rejected.\nReason: {}\n{}Please revise your approach.",
                                        result.reasoning,
                                        result.suggestions.map(|s| format!("Suggestions: {s}\n")).unwrap_or_default(),
                                    );
                                    state.inject_system_notification(&feedback);
                                    continue;
                                }
                            }
                        }
                    }

                    state.add_assistant_message(assistant_content.clone());

                    // Feature 1: Intercept search_tools meta-tool
                    let (tool_results, step_records) = self
                        .execute_tools_with_registry(
                            &assistant_content,
                            state.current_iteration(),
                            tool_registry.as_mut(),
                        )
                        .await;
                    for step in step_records {
                        state.record_step(step);
                    }
                    state.add_tool_results(tool_results);

                    // Persist to session after each tool use iteration
                    self.persist_session_iteration(session_store, &session_metadata, &state)
                        .await;

                    // Check for inter-agent messages
                    if let Some(messages) = self.tools.check_inbox().await {
                        if !messages.is_empty() {
                            let inbox_text = format!(
                                "<inbox_messages>\n{}\n</inbox_messages>",
                                messages.join("\n---\n")
                            );
                            state.inject_system_notification(&inbox_text);
                        }
                    }
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

                    self.emit_response_content_events(&response.content);

                    state.add_assistant_message(response.content);

                    self.finalize_session(
                        session_store,
                        &session_metadata,
                        &state,
                        SessionStatus::Completed,
                    )
                    .await;
                    self.emit_completed(AgentStatus::Success, Some(final_text.clone()), &state);
                    return Ok(state.into_result(AgentStatus::Success, Some(final_text)));
                }
            }
        }
    }

    /// Execute tools with optional registry support for search_tools interception and plan mode blocking.
    async fn execute_tools_with_registry(
        &self,
        assistant_content: &[ContentBlock],
        iteration: u32,
        mut registry: Option<&mut ToolRegistry>,
    ) -> (Vec<ContentBlock>, Vec<StepRecord>) {
        let mut tool_results = Vec::new();
        let mut step_records = Vec::new();

        for block in assistant_content {
            match block {
                ContentBlock::ToolUse { id, name, input } => {
                    // Feature 1: Intercept search_tools meta-tool
                    if ToolRegistry::is_search_tool(name) {
                        if let Some(ref mut reg) = registry {
                            debug!(tool = %name, "Intercepting search_tools meta-tool");
                            let step_start = Instant::now();
                            let query = input.get("query").and_then(|v| v.as_str()).unwrap_or("");
                            let max_results = input
                                .get("max_results")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(5)
                                as usize;
                            let results = reg.search(query, max_results);
                            let result_text = ToolRegistry::format_search_results(&results);
                            let step_duration = step_start.elapsed().as_millis() as u64;

                            let step = StepRecord {
                                iteration,
                                tool: name.to_string(),
                                input: input.clone(),
                                output: Value::String(result_text.clone()),
                                duration_ms: step_duration,
                                is_error: None,
                            };
                            let result_block = ContentBlock::ToolResult {
                                tool_use_id: id.to_string(),
                                content: ToolResultContent::Text(result_text),
                                is_error: None,
                            };
                            tool_results.push(result_block);
                            step_records.push(step);
                            continue;
                        } // if let Some(ref mut reg)
                    }

                    // Feature 4: Block non-read-only tools in plan mode
                    if self.config.plan_mode && !Self::is_read_only_tool(name) {
                        let error_msg = format!(
                            "Tool '{}' is blocked in plan mode. Only read-only tools are allowed.",
                            name
                        );
                        let step = StepRecord {
                            iteration,
                            tool: name.to_string(),
                            input: input.clone(),
                            output: Value::String(error_msg.clone()),
                            duration_ms: 0,
                            is_error: Some(true),
                        };
                        let result_block = ContentBlock::ToolResult {
                            tool_use_id: id.to_string(),
                            content: ToolResultContent::Text(error_msg),
                            is_error: Some(true),
                        };
                        tool_results.push(result_block);
                        step_records.push(step);
                        continue;
                    }

                    // Normal tool execution
                    debug!(tool = %name, "Executing tool");
                    events::emit(
                        &self.event_bus,
                        AgentEvent::ToolUseStart {
                            id: id.clone(),
                            name: name.clone(),
                            input: input.clone(),
                        },
                    );
                    let step_start = Instant::now();
                    let exec_result = self.tools.execute_tool(name, input.clone()).await;
                    let step_duration = step_start.elapsed().as_millis() as u64;
                    let (content_block, step) = Self::process_tool_result(
                        id,
                        name,
                        input,
                        exec_result,
                        step_duration,
                        iteration,
                    );
                    events::emit(
                        &self.event_bus,
                        AgentEvent::ToolUseResult {
                            id: id.clone(),
                            name: name.clone(),
                            output: step.output.clone(),
                            duration_ms: step.duration_ms,
                            is_error: step.is_error.unwrap_or(false),
                        },
                    );
                    tool_results.push(content_block);
                    step_records.push(step);
                }
                ContentBlock::Thinking { thinking } => {
                    debug!(
                        thinking_length = thinking.len(),
                        "Model produced thinking block"
                    );
                    events::emit(
                        &self.event_bus,
                        AgentEvent::ThinkingComplete {
                            thinking: thinking.clone(),
                        },
                    );
                }
                _ => {}
            }
        }

        (tool_results, step_records)
    }

    /// Emit an AgentCompleted event via the event bus.
    fn emit_completed(&self, status: AgentStatus, result: Option<String>, state: &AgentState) {
        events::emit(
            &self.event_bus,
            AgentEvent::AgentCompleted {
                status,
                result,
                total_duration_ms: state.elapsed_ms(),
            },
        );
    }

    /// Emit TextDelta and ThinkingComplete events for content blocks in a response.
    fn emit_response_content_events(&self, content: &[ContentBlock]) {
        for block in content {
            match block {
                ContentBlock::Text { text } => {
                    events::emit(
                        &self.event_bus,
                        AgentEvent::TextDelta { text: text.clone() },
                    );
                }
                ContentBlock::Thinking { thinking } => {
                    events::emit(
                        &self.event_bus,
                        AgentEvent::ThinkingComplete {
                            thinking: thinking.clone(),
                        },
                    );
                }
                _ => {}
            }
        }
    }

    /// Run compaction with progressive staging support and dedicated compaction LLM.
    async fn run_compaction(
        &self,
        compact_config: &CompactionConfig,
        state: &mut AgentState,
        compaction_llm: Option<&dyn LlmProvider>,
    ) {
        // Feature 3: Progressive compaction stages
        if let Some(ref thresholds) = compact_config.stage_thresholds {
            if let Some(stage) = compaction_stages::determine_stage(
                thresholds,
                compact_config.context_window_tokens,
                state.effective_input_tokens(),
            ) {
                info!(
                    stage = ?stage,
                    input_tokens = state.effective_input_tokens(),
                    "Triggering progressive compaction"
                );
                match stage {
                    compaction_stages::CompactionStage::ToolOutputCompression => {
                        let messages = state.messages_mut();
                        let count = compaction_stages::compress_tool_outputs(messages, 500);
                        if count > 0 {
                            info!(compressed = count, "Compressed tool outputs");
                        }
                    }
                    compaction_stages::CompactionStage::ObservationMerging => {
                        let merged = compaction_stages::merge_observations(
                            state.messages(),
                            compact_config.preserve_recent_n,
                        );
                        state.replace_messages(merged);
                        info!("Merged observations");
                    }
                    compaction_stages::CompactionStage::ConversationSummary => {
                        // Delegate to LLM-based compaction
                        self.run_llm_compaction(compact_config, state, compaction_llm)
                            .await;
                    }
                    compaction_stages::CompactionStage::LowValuePruning => {
                        let pruned = compaction_stages::prune_low_value(
                            state.messages(),
                            compact_config.preserve_recent_n,
                            50,
                        );
                        state.replace_messages(pruned);
                        info!("Pruned low-value exchanges");
                    }
                }
            }
        } else {
            // Legacy single-pass compaction
            if compaction::should_compact(compact_config, state.effective_input_tokens()) {
                self.run_llm_compaction(compact_config, state, compaction_llm)
                    .await;
            }
        }
    }

    /// Run LLM-based conversation summarization compaction.
    async fn run_llm_compaction(
        &self,
        compact_config: &CompactionConfig,
        state: &mut AgentState,
        compaction_llm: Option<&dyn LlmProvider>,
    ) {
        info!(
            input_tokens = state.effective_input_tokens(),
            "Triggering context compaction"
        );
        let (compact_end, _) = compaction::compute_compaction_split(
            state.messages().len(),
            compact_config.preserve_recent_n,
        );
        if compact_end > 0 {
            let messages_to_compact = &state.messages()[..compact_end];
            let compaction_messages = compaction::build_compaction_request(messages_to_compact);
            let compaction_system = [SystemContent::text(COMPACTION_SYSTEM_PROMPT)];
            // Feature 2: Use dedicated compaction LLM if available
            let llm_to_use: &dyn LlmProvider = compaction_llm.unwrap_or(&self.llm);
            match llm_to_use
                .send_messages(Some(&compaction_system), &compaction_messages, None)
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
                    // Estimate tokens for compacted context (summary + preserved messages)
                    // A rough estimate: count chars / 4 for the summary, plus a baseline
                    let summary_chars = summary_text.len() as u32;
                    let estimated_tokens = summary_chars / 4 + 1000;
                    state.reset_effective_tokens(estimated_tokens);
                    info!(
                        estimated_tokens = estimated_tokens,
                        "Context compacted successfully"
                    );
                }
                Err(e) => {
                    warn!(error = %e, "Context compaction failed, continuing without");
                }
            }
        }
    }

    /// Resume an existing session and continue the agent loop.
    #[allow(clippy::too_many_arguments)]
    pub async fn resume(
        &self,
        session_store: &dyn SessionStorage,
        session_id: &SessionId,
        credentials: &CredentialSet,
        skill_set: &SkillSet,
        agents_md: &Option<crate::agents_md::AgentsMdContent>,
        compaction_config: Option<&CompactionConfig>,
        compaction_llm: Option<&dyn LlmProvider>,
    ) -> Result<AgentResult, AgentError> {
        let snapshot = session_store.load(session_id).await?;
        info!(
            session_id = %session_id,
            iteration = snapshot.iteration,
            messages = snapshot.messages.len(),
            "Resuming session"
        );

        // Restore state from snapshot
        let mut state = AgentState::from_snapshot(&snapshot);

        let mut system_blocks: Vec<SystemContent> = Vec::new();
        if let Some(ref prompt) = snapshot.system_prompt {
            system_blocks.push(SystemContent::text(prompt.clone()));
        } else if let Some(ref prompt) = self.config.system_prompt {
            system_blocks.push(SystemContent::text(prompt.clone()));
        }
        if let Some(agents_md_prompt) =
            crate::agents_md::inject_agents_md_into_system_prompt(agents_md)
        {
            system_blocks.push(SystemContent::text(agents_md_prompt));
        }
        if let Some(cred_prompt) = inject_credentials_into_system_prompt(credentials) {
            system_blocks.push(SystemContent::text(cred_prompt));
        }
        if let Some(ref coord_config) = self.config.coordination_config {
            if let Some(coord_prompt) =
                crate::coordination::inject_coordination_into_system_prompt(coord_config)
            {
                system_blocks.push(SystemContent::text(coord_prompt));
            }
        }
        if let Some(skill_prompt) = inject_skills_into_system_prompt(skill_set) {
            system_blocks.push(SystemContent::text(skill_prompt));
        }
        // Mark the last block as cached
        if let Some(last) = system_blocks.last_mut() {
            match last {
                SystemContent::Text { cache_control, .. } => {
                    *cache_control = Some(CacheControl::ephemeral());
                }
            }
        }
        let system_prompt = if system_blocks.is_empty() {
            None
        } else {
            Some(system_blocks)
        };

        let all_tool_defs = self.tools.tool_definitions().to_vec();
        let mut tool_registry = if self.config.lazy_tool_discovery {
            let always_avail = ALWAYS_AVAILABLE_TOOLS
                .iter()
                .map(|s| s.to_string())
                .collect();
            Some(ToolRegistry::new(all_tool_defs.clone(), always_avail))
        } else {
            None
        };
        let reminder_scheduler = ReminderScheduler::from_configs(&self.config.reminders);
        let timeout_ms = self.config.timeout_secs * 1000;
        let session_metadata = session_store.load_metadata(session_id).await.ok();

        loop {
            if state.current_iteration() >= self.config.max_iterations {
                self.finalize_session(
                    Some(session_store),
                    &session_metadata,
                    &state,
                    SessionStatus::Completed,
                )
                .await;
                self.emit_completed(AgentStatus::MaxIterations, None, &state);
                return Ok(state.into_result(AgentStatus::MaxIterations, None));
            }

            if state.elapsed_ms() >= timeout_ms {
                self.finalize_session(
                    Some(session_store),
                    &session_metadata,
                    &state,
                    SessionStatus::Completed,
                )
                .await;
                self.emit_completed(AgentStatus::Timeout, None, &state);
                return Ok(state.into_result(AgentStatus::Timeout, None));
            }

            // Compaction with progressive staging and dedicated LLM support
            if let Some(compact_config) = compaction_config {
                self.run_compaction(compact_config, &mut state, compaction_llm)
                    .await;
            }

            state.increment_iteration();
            events::emit(
                &self.event_bus,
                AgentEvent::IterationStarted {
                    iteration: state.current_iteration(),
                },
            );

            // System reminders
            if !reminder_scheduler.is_empty() {
                let reminders = reminder_scheduler.check(state.current_iteration(), &[]);
                for reminder in reminders {
                    state.inject_system_notification(&reminder);
                }
            }

            // Resolve tool definitions
            let effective_tool_defs: Cow<'_, [ToolDefinition]> =
                if let Some(ref registry) = tool_registry {
                    let discovered = registry.discovered_definitions();
                    if self.config.plan_mode {
                        Cow::Owned(Self::filter_read_only_tools(&discovered))
                    } else {
                        Cow::Owned(discovered)
                    }
                } else if self.config.plan_mode {
                    Cow::Owned(Self::filter_read_only_tools(&all_tool_defs))
                } else {
                    Cow::Borrowed(&all_tool_defs)
                };

            let response = self
                .llm
                .send_messages(
                    system_prompt.as_deref(),
                    state.messages(),
                    Some(&effective_tool_defs),
                )
                .await?;

            state.accumulate_usage(response.usage.as_ref(), &response.model);

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
                        )
                        .await;
                        self.emit_completed(AgentStatus::Success, Some(final_text.clone()), &state);
                        return Ok(state.into_result(AgentStatus::Success, Some(final_text)));
                    }

                    state.add_assistant_message(assistant_content.clone());
                    let (tool_results, step_records) = self
                        .execute_tools_with_registry(
                            &assistant_content,
                            state.current_iteration(),
                            tool_registry.as_mut(),
                        )
                        .await;
                    for step in step_records {
                        state.record_step(step);
                    }
                    state.add_tool_results(tool_results);
                    self.persist_session_iteration(Some(session_store), &session_metadata, &state)
                        .await;

                    // Check for inter-agent messages
                    if let Some(messages) = self.tools.check_inbox().await {
                        if !messages.is_empty() {
                            let inbox_text = format!(
                                "<inbox_messages>\n{}\n</inbox_messages>",
                                messages.join("\n---\n")
                            );
                            state.inject_system_notification(&inbox_text);
                        }
                    }
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

                    self.emit_response_content_events(&response.content);

                    state.add_assistant_message(response.content);
                    self.finalize_session(
                        Some(session_store),
                        &session_metadata,
                        &state,
                        SessionStatus::Completed,
                    )
                    .await;
                    self.emit_completed(AgentStatus::Success, Some(final_text.clone()), &state);
                    return Ok(state.into_result(AgentStatus::Success, Some(final_text)));
                }
            }
        }
    }

    /// Persist session state after each iteration (best-effort, failures logged and ignored).
    async fn persist_session_iteration(
        &self,
        session_store: Option<&dyn SessionStorage>,
        session_metadata: &Option<crate::session::types::SessionMetadata>,
        state: &AgentState,
    ) {
        if let (Some(store), Some(metadata)) = (session_store, session_metadata) {
            let (r1, r2) = tokio::join!(
                store.append_messages(&metadata.id, state.messages()),
                store.save_steps(&metadata.id, state.steps())
            );
            if let Err(e) = r1 {
                warn!(error = %e, "Failed to persist session messages");
            }
            if let Err(e) = r2 {
                warn!(error = %e, "Failed to persist session steps");
            }
        }
    }

    /// Finalize session with final status (best-effort).
    async fn finalize_session(
        &self,
        session_store: Option<&dyn SessionStorage>,
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
            let (r1, r2) = tokio::join!(
                store.save_metadata(&updated),
                store.save_steps(&metadata.id, state.steps())
            );
            if let Err(e) = r1 {
                warn!(error = %e, "Failed to finalize session");
            }
            if let Err(e) = r2 {
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
            _system: Option<&[SystemContent]>,
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
            coordination_config: None,
            tool_result_max_bytes: 32768,
            max_budget_usd: None,
            lazy_tool_discovery: false,
            plan_mode: false,
            reminders: Vec::new(),
            self_critique: None,
        }
    }

    fn default_tools() -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "navigate".to_string(),
            description: "Navigate to URL".to_string(),
            input_schema: json!({"type": "object", "properties": {"url": {"type": "string"}}}),
            cache_control: None,
            read_only: false,
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
            coordination_config: None,
            tool_result_max_bytes: 32768,
            max_budget_usd: None,
            lazy_tool_discovery: false,
            plan_mode: false,
            reminders: Vec::new(),
            self_critique: None,
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
            coordination_config: None,
            tool_result_max_bytes: 32768,
            max_budget_usd: None,
            lazy_tool_discovery: false,
            plan_mode: false,
            reminders: Vec::new(),
            self_critique: None,
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
                        cache_creation_input_tokens: None,
                        cache_read_input_tokens: None,
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
                        cache_creation_input_tokens: None,
                        cache_read_input_tokens: None,
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
                system: Option<&[SystemContent]>,
                _messages: &[Message],
                _tools: Option<&[ToolDefinition]>,
            ) -> Result<MessagesResponse, AgentError> {
                *self.captured_system.lock().unwrap() = system.map(|s| {
                    s.iter()
                        .map(|c| match c {
                            SystemContent::Text { text, .. } => text.clone(),
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                });
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

    #[test]
    fn test_is_read_only_tool() {
        // Read-only tools
        assert!(AgentRunner::<MockLlm, MockTools>::is_read_only_tool(
            "read_file"
        ));
        assert!(AgentRunner::<MockLlm, MockTools>::is_read_only_tool("grep"));
        assert!(AgentRunner::<MockLlm, MockTools>::is_read_only_tool("glob"));
        assert!(AgentRunner::<MockLlm, MockTools>::is_read_only_tool(
            "list_files"
        ));
        assert!(AgentRunner::<MockLlm, MockTools>::is_read_only_tool(
            "screenshot"
        ));

        // Write / side-effecting tools
        assert!(!AgentRunner::<MockLlm, MockTools>::is_read_only_tool(
            "write_file"
        ));
        assert!(!AgentRunner::<MockLlm, MockTools>::is_read_only_tool(
            "edit_file"
        ));
        assert!(!AgentRunner::<MockLlm, MockTools>::is_read_only_tool(
            "bash"
        ));
        assert!(!AgentRunner::<MockLlm, MockTools>::is_read_only_tool(
            "navigate"
        ));
        assert!(!AgentRunner::<MockLlm, MockTools>::is_read_only_tool(
            "click"
        ));
        assert!(!AgentRunner::<MockLlm, MockTools>::is_read_only_tool(
            "unknown_tool"
        ));
    }

    #[tokio::test]
    async fn test_execute_tools_empty_content() {
        let tools = MockTools {
            tools: default_tools(),
            results: Arc::new(Mutex::new(vec![])),
        };
        let llm = MockLlm {
            responses: Arc::new(Mutex::new(vec![])),
        };
        let runner = AgentRunner::new(llm, tools, default_config());

        let content = vec![ContentBlock::Text {
            text: "No tools here".to_string(),
        }];
        let (tool_results, step_records) = runner.execute_tools(&content, 1).await;
        assert!(tool_results.is_empty());
        assert!(step_records.is_empty());
    }

    #[tokio::test]
    async fn test_execute_tools_single_tool() {
        let tools = MockTools {
            tools: default_tools(),
            results: Arc::new(Mutex::new(vec![Ok(ToolExecutionResult {
                content: "navigated".to_string(),
                is_error: false,
            })])),
        };
        let llm = MockLlm {
            responses: Arc::new(Mutex::new(vec![])),
        };
        let runner = AgentRunner::new(llm, tools, default_config());

        let content = vec![ContentBlock::ToolUse {
            id: "toolu_01".to_string(),
            name: "navigate".to_string(),
            input: json!({"url": "https://example.com"}),
        }];
        let (tool_results, step_records) = runner.execute_tools(&content, 1).await;
        assert_eq!(tool_results.len(), 1);
        assert_eq!(step_records.len(), 1);
        assert!(matches!(
            &tool_results[0],
            ContentBlock::ToolResult { content, is_error, .. }
            if *content == ToolResultContent::Text("navigated".to_string()) && is_error.is_none()
        ));
        assert_eq!(step_records[0].tool, "navigate");
    }

    /// A mock executor that returns a result based on the tool name.
    struct NameAwareMockTools;

    #[async_trait]
    impl ToolExecutor for NameAwareMockTools {
        fn tool_definitions(&self) -> &[ToolDefinition] {
            &[]
        }

        async fn execute_tool(
            &self,
            name: &str,
            _args: Value,
        ) -> Result<ToolExecutionResult, AgentError> {
            Ok(ToolExecutionResult {
                content: format!("{name}_result"),
                is_error: false,
            })
        }
    }

    #[tokio::test]
    async fn test_execute_tools_preserves_order_mixed() {
        // read_file (read-only), write_file (write), grep (read-only)
        let llm = MockLlm {
            responses: Arc::new(Mutex::new(vec![])),
        };
        let runner = AgentRunner::new(llm, NameAwareMockTools, default_config());

        let content = vec![
            ContentBlock::ToolUse {
                id: "t1".to_string(),
                name: "read_file".to_string(),
                input: json!({"path": "a.txt"}),
            },
            ContentBlock::ToolUse {
                id: "t2".to_string(),
                name: "write_file".to_string(),
                input: json!({"path": "b.txt", "content": "hello"}),
            },
            ContentBlock::ToolUse {
                id: "t3".to_string(),
                name: "grep".to_string(),
                input: json!({"pattern": "foo"}),
            },
        ];
        let (tool_results, step_records) = runner.execute_tools(&content, 1).await;
        assert_eq!(tool_results.len(), 3);

        // Results should be in original order regardless of execution order
        assert!(matches!(
            &tool_results[0],
            ContentBlock::ToolResult { tool_use_id, content, .. }
            if tool_use_id == "t1" && *content == ToolResultContent::Text("read_file_result".to_string())
        ));
        assert!(matches!(
            &tool_results[1],
            ContentBlock::ToolResult { tool_use_id, content, .. }
            if tool_use_id == "t2" && *content == ToolResultContent::Text("write_file_result".to_string())
        ));
        assert!(matches!(
            &tool_results[2],
            ContentBlock::ToolResult { tool_use_id, content, .. }
            if tool_use_id == "t3" && *content == ToolResultContent::Text("grep_result".to_string())
        ));

        // Step records also in order
        assert_eq!(step_records[0].tool, "read_file");
        assert_eq!(step_records[1].tool, "write_file");
        assert_eq!(step_records[2].tool, "grep");
    }

    #[tokio::test]
    async fn test_execute_tools_handles_errors() {
        let tools = MockTools {
            tools: default_tools(),
            results: Arc::new(Mutex::new(vec![Err(AgentError::ToolExecution(
                "fail".to_string(),
            ))])),
        };
        let llm = MockLlm {
            responses: Arc::new(Mutex::new(vec![])),
        };
        let runner = AgentRunner::new(llm, tools, default_config());

        let content = vec![ContentBlock::ToolUse {
            id: "t1".to_string(),
            name: "bash".to_string(),
            input: json!({"command": "false"}),
        }];
        let (tool_results, step_records) = runner.execute_tools(&content, 1).await;
        assert_eq!(tool_results.len(), 1);
        assert!(matches!(
            &tool_results[0],
            ContentBlock::ToolResult {
                is_error: Some(true),
                ..
            }
        ));
        assert_eq!(step_records[0].is_error, Some(true));
    }

    #[tokio::test]
    async fn test_event_bus_emits_tool_events() {
        let bus = Arc::new(EventBus::new(64));
        let mut rx = bus.subscribe();

        let tools = MockTools {
            tools: default_tools(),
            results: Arc::new(Mutex::new(vec![])),
        };

        let content = vec![ContentBlock::ToolUse {
            id: "t1".to_string(),
            name: "navigate".to_string(),
            input: json!({"url": "https://example.com"}),
        }];

        let runner = AgentRunner::new(
            MockLlm {
                responses: Arc::new(Mutex::new(vec![])),
            },
            tools,
            default_config(),
        )
        .with_event_bus(bus);

        runner.execute_tools(&content, 1).await;

        // Should receive ToolUseStart then ToolUseResult
        let ev1 = rx.recv().await.unwrap();
        assert_eq!(ev1.event_type(), "tool_use_start");

        let ev2 = rx.recv().await.unwrap();
        assert_eq!(ev2.event_type(), "tool_use_result");
    }

    #[tokio::test]
    async fn test_event_bus_emits_thinking_events() {
        let bus = Arc::new(EventBus::new(64));
        let mut rx = bus.subscribe();

        let tools = MockTools {
            tools: default_tools(),
            results: Arc::new(Mutex::new(vec![])),
        };

        let content = vec![ContentBlock::Thinking {
            thinking: "Let me think about this...".to_string(),
        }];

        let runner = AgentRunner::new(
            MockLlm {
                responses: Arc::new(Mutex::new(vec![])),
            },
            tools,
            default_config(),
        )
        .with_event_bus(bus);

        runner.execute_tools(&content, 1).await;

        let ev = rx.recv().await.unwrap();
        assert_eq!(ev.event_type(), "thinking_complete");
    }

    #[tokio::test]
    async fn test_event_bus_emits_completion_on_end_turn() {
        let bus = Arc::new(EventBus::new(64));
        let mut rx = bus.subscribe();

        let llm = MockLlm {
            responses: Arc::new(Mutex::new(vec![make_end_turn_response("Done!")])),
        };
        let tools = MockTools {
            tools: default_tools(),
            results: Arc::new(Mutex::new(vec![])),
        };
        let runner = AgentRunner::new(llm, tools, default_config()).with_event_bus(bus);

        let result = runner
            .run("test task", &Default::default(), &Default::default(), &None)
            .await
            .unwrap();
        assert_eq!(result.status, AgentStatus::Success);

        // Collect all events
        let mut event_types = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            event_types.push(ev.event_type().to_string());
        }

        assert!(
            event_types.contains(&"agent_started".to_string()),
            "Missing agent_started event"
        );
        assert!(
            event_types.contains(&"iteration_started".to_string()),
            "Missing iteration_started event"
        );
        assert!(
            event_types.contains(&"text_delta".to_string()),
            "Missing text_delta event"
        );
        assert!(
            event_types.contains(&"agent_completed".to_string()),
            "Missing agent_completed event"
        );
    }
}
