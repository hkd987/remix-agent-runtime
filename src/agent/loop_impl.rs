use serde_json::Value;
use std::borrow::Cow;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, error, info, warn};

use crate::browser::mcp::ToolExecutor;
use crate::config::credentials::{inject_credentials_into_system_prompt, CredentialSet};
use crate::config::schema::{AgentConfig, CompactionConfig};
use crate::error::AgentError;
use crate::llm::client::LlmProvider;
use crate::llm::types::{
    content_has_tool_use, CacheControl, ContentBlock, Message, MessagesResponse, StopReason,
    SystemContent, ToolDefinition, ToolResultContent,
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

/// Message injected when the LLM returns text-only without using any tools.
const NUDGE_MESSAGE: &str = "You provided analysis but did not take any action. Continue with the implementation. Use tools to make progress on the task.";

/// Message injected when a response is cut off by the output token limit.
const TRUNCATION_MESSAGE: &str = "Your previous response was cut off because it reached the output token limit. Continue from exactly where you stopped. Do not repeat what you already produced.";

/// Message injected for a one-time goal check before the agent terminates.
const GOAL_CHECK_BASE: &str = "STOP. Before you finish, re-read the ORIGINAL TASK below and verify ALL requirements and constraints are met:\n\n\
    <original_task>\n{task}\n</original_task>\n\n\
    Now verify:\n\
    1) Re-read every constraint in the task above. Check each one explicitly.\n\
    2) Check that all required output files exist at the expected paths.\n\
    3) Run any provided test/eval scripts to confirm correctness.\n\
    4) If ANYTHING is missing, failing, or violates a constraint, fix it now.\n\
    If everything checks out, respond with your final summary.";

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
    thinking_control: Option<Arc<dyn crate::llm::client::ThinkingControl>>,
    /// Registry for lifecycle hooks (SessionStart, SessionEnd, Stop, PreCompact).
    ///
    /// These are not tool calls, so they never reach `HookAwareExecutor`, which is a
    /// `ToolExecutor` decorator. The runner is the only layer that knows when a session
    /// starts, stops, or compacts, so it fires them.
    hooks: Option<(Arc<crate::plugins::components::hooks::HookRegistry>, u64)>,
    /// Dedicated client for self-critique, honouring `self_critique.model` and
    /// `self_critique.max_tokens`. Falls back to the primary client when unset.
    critique_llm: Option<Arc<dyn LlmProvider>>,
}

impl<L: LlmProvider, T: ToolExecutor> AgentRunner<L, T> {
    pub fn new(llm: L, tools: T, config: AgentConfig) -> Self {
        Self {
            llm,
            tools,
            config,
            event_bus: None,
            thinking_control: None,
            hooks: None,
            critique_llm: None,
        }
    }

    /// Attach a dedicated client for the self-critique pass.
    pub fn with_critique_llm(mut self, llm: Arc<dyn LlmProvider>) -> Self {
        self.critique_llm = Some(llm);
        self
    }

    /// Attach a hook registry so the runner can fire lifecycle hooks.
    pub fn with_hooks(
        mut self,
        registry: Arc<crate::plugins::components::hooks::HookRegistry>,
        timeout_secs: u64,
    ) -> Self {
        self.hooks = Some((registry, timeout_secs));
        self
    }

    /// Fire lifecycle hooks for `timing`, if a registry is attached.
    async fn fire_lifecycle(
        &self,
        timing: crate::plugins::components::hooks::HookTiming,
        context: Value,
    ) {
        if let Some((registry, timeout_secs)) = &self.hooks {
            crate::plugins::hook_executor::fire_lifecycle_hooks(
                registry,
                *timeout_secs,
                &timing,
                context,
            )
            .await;
        }
    }

    /// Attach an event bus for real-time event streaming.
    pub fn with_event_bus(mut self, event_bus: Arc<EventBus>) -> Self {
        self.event_bus = Some(event_bus);
        self
    }

    /// Attach a thinking control for dynamic reasoning budget adjustment.
    pub fn with_thinking_control(
        mut self,
        control: Arc<dyn crate::llm::client::ThinkingControl>,
    ) -> Self {
        self.thinking_control = Some(control);
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

    /// Attempt to nudge the LLM to continue using tools.
    /// Returns `None` if a nudge was applied (caller should `continue` the loop).
    /// Returns `Some(content)` if no nudge was applied (content returned for further processing).
    fn try_nudge(
        &self,
        state: &mut AgentState,
        content: Vec<ContentBlock>,
        nudge_count: &mut u32,
    ) -> Option<Vec<ContentBlock>> {
        if !self.config.nudge_on_text_only || *nudge_count >= self.config.nudge_max_count {
            return Some(content);
        }
        *nudge_count += 1;
        info!(
            nudge_count = *nudge_count,
            max = self.config.nudge_max_count,
            "Text-only response detected, nudging LLM to continue"
        );
        self.emit_response_content_events(&content);
        state.add_assistant_message(content);
        state.inject_system_notification(NUDGE_MESSAGE);
        None
    }

    /// Ask the model to continue a response that was cut off by the output token cap.
    ///
    /// Returns `None` if a continuation was requested (caller should `continue` the
    /// loop), `Some(content)` if the attempt budget is exhausted and the caller should
    /// terminate with whatever text arrived.
    fn try_continue_truncated(
        &self,
        state: &mut AgentState,
        content: Vec<ContentBlock>,
        continuation_count: &mut u32,
    ) -> Option<Vec<ContentBlock>> {
        if *continuation_count >= self.config.nudge_max_count {
            warn!(
                attempts = *continuation_count,
                "Response repeatedly truncated at max_tokens; returning partial output"
            );
            return Some(content);
        }
        *continuation_count += 1;
        info!(
            attempt = *continuation_count,
            "Response truncated at max_tokens, requesting continuation"
        );
        self.emit_response_content_events(&content);
        state.add_assistant_message_refusing_tools(content, TRUNCATION_MESSAGE);
        None
    }

    /// Attempt a one-time goal check before termination.
    /// Returns `Some(content)` if the check was skipped (caller should proceed to terminate).
    /// Returns `None` if the check was applied (caller should `continue` the loop).
    fn try_goal_check(
        &self,
        state: &mut AgentState,
        content: Vec<ContentBlock>,
        goal_check_fired: &mut bool,
    ) -> Option<Vec<ContentBlock>> {
        // Only fire when enabled, not yet fired, and agent did meaningful work.
        // Require at least 5 iterations to prevent premature termination on complex tasks.
        const MIN_ITERATIONS_FOR_GOAL_CHECK: u32 = 5;
        if !self.config.goal_check_on_complete
            || *goal_check_fired
            || state.current_iteration() <= MIN_ITERATIONS_FOR_GOAL_CHECK
        {
            return Some(content);
        }
        *goal_check_fired = true;
        info!("Goal check: verifying work completion before termination");
        self.emit_response_content_events(&content);
        state.add_assistant_message(content);
        let task_text = state.original_task().unwrap_or("(task not available)");
        let goal_msg = GOAL_CHECK_BASE.replace("{task}", task_text);
        state.inject_system_notification(&goal_msg);
        None
    }

    /// Build an action progress reminder if the interval is configured and the
    /// current iteration is on an interval boundary.
    fn build_action_reminder(&self, state: &AgentState) -> Option<String> {
        let interval = self.config.action_reminder_interval?;
        if interval == 0 {
            return None;
        }
        let iteration = state.current_iteration();

        // Only fire on interval boundaries, starting from the interval (not iteration 0)
        if iteration == 0 || !iteration.is_multiple_of(interval) {
            return None;
        }

        let has_written_files = state.has_written_files();
        let remaining = self.config.max_iterations.saturating_sub(iteration);

        let msg = if has_written_files {
            format!(
                "Progress check: You are on iteration {} of {} ({} remaining). \
                 You have written files - good. Make sure your solution is correct by running tests/verification. \
                 If tests fail, fix and re-test.",
                iteration, self.config.max_iterations, remaining
            )
        } else {
            format!(
                "URGENT: You are on iteration {} of {} ({} remaining) and you have NOT written any output files yet. \
                 Stop analyzing and START WRITING CODE NOW. \
                 Write a working solution immediately, even if imperfect. You can iterate and improve it after. \
                 Check the task description for required output file paths.",
                iteration, self.config.max_iterations, remaining
            )
        };

        Some(msg)
    }

    /// Build a one-time budget warning when the agent reaches a configured
    /// fraction of max_iterations. Returns `None` on all other iterations.
    fn build_budget_warning(&self, state: &AgentState) -> Option<String> {
        let threshold = self.config.iteration_budget_warning_threshold?;
        let threshold_iteration = (threshold * self.config.max_iterations as f32).ceil() as u32;
        if state.current_iteration() != threshold_iteration {
            return None;
        }
        let remaining = self
            .config
            .max_iterations
            .saturating_sub(state.current_iteration());
        Some(format!(
            "[BUDGET WARNING] You have used {}/{} iterations ({:.0}%). \
             Focus on completing and verifying your current approach rather than starting over. \
             You have {} iterations remaining.",
            state.current_iteration(),
            self.config.max_iterations,
            (state.current_iteration() as f32 / self.config.max_iterations as f32) * 100.0,
            remaining
        ))
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
                ContentBlock::Thinking { thinking, .. } => {
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
        // Validate reasoning stages config if present
        if let Some(ref stages_config) = self.config.reasoning_stages {
            if let Err(e) = super::reasoning_stages::validate_config(stages_config) {
                warn!("{}", e);
            }
        }

        let mut state = AgentState::new(task);

        events::emit(
            &self.event_bus,
            AgentEvent::AgentStarted {
                task: task.to_string(),
                timestamp_ms: chrono::Utc::now().timestamp_millis() as u64,
            },
        );

        self.fire_lifecycle(
            crate::plugins::components::hooks::HookTiming::SessionStart,
            serde_json::json!({ "task": task }),
        )
        .await;

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
        let mut nudge_count: u32 = 0;
        let mut goal_check_fired = false;
        // Self-critique rejections were unbounded: `max_rounds` was declared in config
        // and never read, so a critic that kept rejecting could loop until the
        // iteration cap.
        let mut critique_rounds: u32 = 0;

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

            // Budget check. `max_budget_usd` was declared in config and documented but
            // never compared against anything, so a runaway agent could spend without
            // limit while the knob appeared to be doing something.
            if let Some(max_budget) = self.config.max_budget_usd {
                if state.total_cost() >= max_budget {
                    info!(
                        spent_usd = state.total_cost(),
                        budget_usd = max_budget,
                        "Budget exhausted"
                    );
                    self.finalize_session(
                        session_store,
                        &session_metadata,
                        &state,
                        SessionStatus::Completed,
                    )
                    .await;
                    self.emit_completed(AgentStatus::BudgetExceeded, None, &state);
                    return Ok(state.into_result(AgentStatus::BudgetExceeded, None));
                }
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

            // Action progress reminder
            if let Some(reminder) = self.build_action_reminder(&state) {
                state.inject_system_notification(&reminder);
            }

            // Iteration budget warning (one-time)
            if let Some(warning) = self.build_budget_warning(&state) {
                state.inject_system_notification(&warning);
            }

            // Loop detection
            if let Some(ref loop_config) = self.config.loop_detection {
                if let Some(warning) =
                    super::loop_detection::detect_loop(state.steps(), loop_config)
                {
                    state.inject_system_notification(&warning);
                }

                // Semantic loop detection (test-without-modify)
                if let Some(warning) =
                    super::loop_detection::detect_semantic_loop(state.steps(), loop_config)
                {
                    state.inject_system_notification(&warning);
                }
            }

            // Feature 5: Check system reminders before LLM call
            if !reminder_scheduler.is_empty() {
                let pending_tools: Vec<String> = Vec::new(); // pre-execution, no pending tools yet
                let reminders = reminder_scheduler.check(state.current_iteration(), &pending_tools);
                for reminder in reminders {
                    state.inject_system_notification(&reminder);
                }
            }

            // Reasoning budget stages: adjust thinking budget based on phase
            if let (Some(ref stages_config), Some(ref thinking_ctl)) =
                (&self.config.reasoning_stages, &self.thinking_control)
            {
                let budget = super::reasoning_stages::compute_thinking_budget(
                    stages_config,
                    state.current_iteration(),
                    self.config.max_iterations,
                );
                thinking_ctl
                    .set_thinking_config(Some(crate::llm::types::ThinkingConfig::enabled(budget)));
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
                .await;

            // Do not let the `?` shortcut skip teardown: without this the persisted
            // session stays stuck in its pre-existing status and no terminal event ever
            // reaches the event bus, so a subscriber sees the run simply stop.
            let response = match response {
                Ok(r) => r,
                Err(e) => {
                    error!(error = %e, "LLM request failed, ending run");
                    self.finalize_session(
                        session_store,
                        &session_metadata,
                        &state,
                        SessionStatus::Error,
                    )
                    .await;
                    events::emit(
                        &self.event_bus,
                        AgentEvent::AgentError {
                            error: e.to_string(),
                        },
                    );
                    return Err(e);
                }
            };

            state.accumulate_usage(response.usage.as_ref(), &response.model);

            // A response carrying tool_use blocks must have them executed regardless of
            // the reported stop_reason. Terminating on `end_turn` while tool calls are
            // pending both drops the work the model asked for and leaves the tool_use
            // unanswered, which the API rejects on the next request.
            let effective_stop_reason = if content_has_tool_use(&response.content) {
                StopReason::ToolUse
            } else {
                response.stop_reason
            };

            match effective_stop_reason {
                StopReason::ToolUse => {
                    let assistant_content = response.content.clone();

                    // Check if there are actually tool_use blocks; if not, treat as end_turn.
                    let has_tool_use = content_has_tool_use(&assistant_content);
                    if !has_tool_use {
                        let Some(assistant_content) =
                            self.try_nudge(&mut state, assistant_content, &mut nudge_count)
                        else {
                            continue;
                        };
                        // Goal check: one-time verification before termination
                        let Some(assistant_content) = self.try_goal_check(
                            &mut state,
                            assistant_content,
                            &mut goal_check_fired,
                        ) else {
                            continue;
                        };
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
                        if critique_rounds < critique_config.max_rounds
                            && self_critique::should_critique(critique_config, &assistant_content)
                        {
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
                            // Use the configured critique model when one was wired in,
                            // falling back to the primary client. `model` and
                            // `max_tokens` were previously ignored entirely; the client
                            // is injected rather than built here so the runner stays
                            // generic over the provider.
                            let critique_provider: &dyn LlmProvider =
                                match self.critique_llm.as_deref() {
                                    Some(c) => c,
                                    None => &self.llm,
                                };

                            if let Ok(critique_response) = critique_provider
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
                                    critique_rounds += 1;
                                    info!(
                                        round = critique_rounds,
                                        max_rounds = critique_config.max_rounds,
                                        "Self-critique rejected actions: {}",
                                        result.reasoning
                                    );
                                    let feedback = format!(
                                        "[SELF_CRITIQUE] Your planned actions were rejected.\nReason: {}\n{}Please revise your approach.",
                                        result.reasoning,
                                        result.suggestions.map(|s| format!("Suggestions: {s}\n")).unwrap_or_default(),
                                    );
                                    // The rejected content carries tool_use blocks by
                                    // construction, so each one needs a tool_result.
                                    state.add_assistant_message_refusing_tools(
                                        assistant_content,
                                        &feedback,
                                    );
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
                StopReason::EndTurn
                | StopReason::MaxTokens
                | StopReason::StopSequence
                | StopReason::Unknown => {
                    if effective_stop_reason == StopReason::Unknown {
                        warn!(
                            stop_reason = ?response.stop_reason,
                            "Unrecognized stop_reason from the API; treating as end_turn"
                        );
                    }

                    // Normalization above guarantees this arm is only reached when the
                    // response carries no tool_use blocks, so both the nudge and the
                    // goal check can safely append the content and inject a follow-up
                    // user message without orphaning a tool call.

                    // `max_tokens` means the output was cut off mid-sentence. Reporting
                    // that as a successful completion hands back a truncated answer, so
                    // ask for a continuation first.
                    let content = if effective_stop_reason == StopReason::MaxTokens {
                        let Some(c) = self.try_continue_truncated(
                            &mut state,
                            response.content,
                            &mut nudge_count,
                        ) else {
                            continue;
                        };
                        c
                    } else {
                        response.content
                    };

                    let Some(content) = self.try_nudge(&mut state, content, &mut nudge_count)
                    else {
                        continue;
                    };

                    // Goal check: one-time verification before termination
                    let Some(content) =
                        self.try_goal_check(&mut state, content, &mut goal_check_fired)
                    else {
                        continue;
                    };

                    let final_text = content
                        .iter()
                        .filter_map(|block| match block {
                            ContentBlock::Text { text } => Some(text.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");

                    self.emit_response_content_events(&content);

                    state.add_assistant_message(content);

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
                ContentBlock::Thinking { thinking, .. } => {
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
                ContentBlock::Thinking { thinking, .. } => {
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
        // Honour `enabled` on both paths. Only the legacy branch used to check it, via
        // `should_compact`, so a config with `enabled: false` plus stage thresholds
        // still compacted.
        if !compact_config.enabled {
            return;
        }

        self.fire_lifecycle(
            crate::plugins::components::hooks::HookTiming::PreCompact,
            serde_json::json!({
                "iteration": state.current_iteration(),
                "input_tokens": state.effective_input_tokens(),
            }),
        )
        .await;

        // Feature 3: Progressive compaction stages
        if let Some(ref thresholds) = compact_config.stage_thresholds {
            if let Some(stage) = compaction_stages::determine_stage(
                thresholds,
                compact_config.context_window_tokens,
                state.effective_input_tokens(),
            ) {
                // Latch: a stage that already ran and did not bring the context below
                // its threshold has nothing left to give, and re-running it every
                // iteration just rewrites the message list for no benefit.
                if !state.should_run_compaction_stage(stage) {
                    debug!(
                        stage = ?stage,
                        "Skipping compaction stage; already applied at this context size"
                    );
                    return;
                }
                info!(
                    stage = ?stage,
                    input_tokens = state.effective_input_tokens(),
                    "Triggering progressive compaction"
                );
                state.record_compaction_stage(stage);
                match stage {
                    compaction_stages::CompactionStage::ToolOutputCompression => {
                        let messages = state.messages_mut();
                        let preserve = compact_config.preserve_recent_n;
                        let count =
                            compaction_stages::compress_tool_outputs(messages, 500, preserve);
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
        let mut nudge_count: u32 = 0;
        let mut goal_check_fired = false;

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

            // Budget check. `max_budget_usd` was declared in config and documented but
            // never compared against anything, so a runaway agent could spend without
            // limit while the knob appeared to be doing something.
            if let Some(max_budget) = self.config.max_budget_usd {
                if state.total_cost() >= max_budget {
                    info!(
                        spent_usd = state.total_cost(),
                        budget_usd = max_budget,
                        "Budget exhausted"
                    );
                    self.finalize_session(
                        Some(session_store),
                        &session_metadata,
                        &state,
                        SessionStatus::Completed,
                    )
                    .await;
                    self.emit_completed(AgentStatus::BudgetExceeded, None, &state);
                    return Ok(state.into_result(AgentStatus::BudgetExceeded, None));
                }
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

            // Action progress reminder
            if let Some(reminder) = self.build_action_reminder(&state) {
                state.inject_system_notification(&reminder);
            }

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
                .await;

            // Do not let the `?` shortcut skip teardown: without this the persisted
            // session stays stuck in its pre-existing status and no terminal event ever
            // reaches the event bus, so a subscriber sees the run simply stop.
            let response = match response {
                Ok(r) => r,
                Err(e) => {
                    error!(error = %e, "LLM request failed, ending run");
                    self.finalize_session(
                        Some(session_store),
                        &session_metadata,
                        &state,
                        SessionStatus::Error,
                    )
                    .await;
                    events::emit(
                        &self.event_bus,
                        AgentEvent::AgentError {
                            error: e.to_string(),
                        },
                    );
                    return Err(e);
                }
            };

            state.accumulate_usage(response.usage.as_ref(), &response.model);

            // A response carrying tool_use blocks must have them executed regardless of
            // the reported stop_reason. Terminating on `end_turn` while tool calls are
            // pending both drops the work the model asked for and leaves the tool_use
            // unanswered, which the API rejects on the next request.
            let effective_stop_reason = if content_has_tool_use(&response.content) {
                StopReason::ToolUse
            } else {
                response.stop_reason
            };

            match effective_stop_reason {
                StopReason::ToolUse => {
                    let assistant_content = response.content.clone();
                    let has_tool_use = content_has_tool_use(&assistant_content);
                    if !has_tool_use {
                        let Some(assistant_content) =
                            self.try_nudge(&mut state, assistant_content, &mut nudge_count)
                        else {
                            continue;
                        };
                        // Goal check: one-time verification before termination
                        let Some(assistant_content) = self.try_goal_check(
                            &mut state,
                            assistant_content,
                            &mut goal_check_fired,
                        ) else {
                            continue;
                        };
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
                StopReason::EndTurn
                | StopReason::MaxTokens
                | StopReason::StopSequence
                | StopReason::Unknown => {
                    if effective_stop_reason == StopReason::Unknown {
                        warn!(
                            stop_reason = ?response.stop_reason,
                            "Unrecognized stop_reason from the API; treating as end_turn"
                        );
                    }

                    // Normalization above guarantees this arm is only reached when the
                    // response carries no tool_use blocks, so both the nudge and the
                    // goal check can safely append the content and inject a follow-up
                    // user message without orphaning a tool call.

                    // `max_tokens` means the output was cut off mid-sentence. Reporting
                    // that as a successful completion hands back a truncated answer, so
                    // ask for a continuation first.
                    let content = if effective_stop_reason == StopReason::MaxTokens {
                        let Some(c) = self.try_continue_truncated(
                            &mut state,
                            response.content,
                            &mut nudge_count,
                        ) else {
                            continue;
                        };
                        c
                    } else {
                        response.content
                    };

                    let Some(content) = self.try_nudge(&mut state, content, &mut nudge_count)
                    else {
                        continue;
                    };

                    // Goal check: one-time verification before termination
                    let Some(content) =
                        self.try_goal_check(&mut state, content, &mut goal_check_fired)
                    else {
                        continue;
                    };

                    let final_text = content
                        .iter()
                        .filter_map(|block| match block {
                            ContentBlock::Text { text } => Some(text.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");

                    self.emit_response_content_events(&content);

                    state.add_assistant_message(content);
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
                store.save_messages(&metadata.id, state.messages()),
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
        // Every terminal path in every loop routes through here, so this is the one
        // place that reliably corresponds to "the run is ending".
        let end_context = serde_json::json!({
            "status": format!("{status:?}"),
            "iterations": state.current_iteration(),
            "total_input_tokens": state.total_input_tokens(),
            "total_output_tokens": state.total_output_tokens(),
            "total_cost_usd": state.total_cost(),
        });
        self.fire_lifecycle(
            crate::plugins::components::hooks::HookTiming::Stop,
            end_context.clone(),
        )
        .await;
        self.fire_lifecycle(
            crate::plugins::components::hooks::HookTiming::SessionEnd,
            end_context,
        )
        .await;

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

/// Interactive streaming support for TUI chat mode.
///
/// This impl block is gated on `L: StreamingLlmProvider` and provides
/// the `run_interactive_stream` method for real-time streaming conversations.
impl<L: crate::llm::client::StreamingLlmProvider, T: ToolExecutor> AgentRunner<L, T> {
    /// Filter out thinking/redacted_thinking blocks from messages before sending to API.
    /// Some providers (OpenRouter) reject these blocks on subsequent turns.
    fn filter_thinking_blocks(messages: &[Message]) -> Vec<Message> {
        // Fast path: if no messages contain thinking blocks, avoid cloning entirely
        let has_thinking = messages.iter().any(|msg| {
            msg.content.iter().any(|block| {
                matches!(
                    block,
                    ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking { .. }
                )
            })
        });
        if !has_thinking {
            return messages.to_vec();
        }

        messages
            .iter()
            .map(|msg| Message {
                role: msg.role.clone(),
                content: msg
                    .content
                    .iter()
                    .filter(|block| {
                        !matches!(
                            block,
                            ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking { .. }
                        )
                    })
                    .cloned()
                    .collect(),
            })
            .collect()
    }

    /// Run the interactive streaming agent loop.
    ///
    /// This method drives a multi-turn conversation where:
    /// 1. The TUI sends user messages via `input_rx`
    /// 2. The agent streams LLM responses via `StreamAssembler`, emitting `AgentEvent`s
    /// 3. Tool execution happens between LLM calls (same as the batch loop)
    /// 4. On `EndTurn`, the agent signals `AwaitingInput` and waits for the next message
    /// 5. On `Quit`, the agent returns the final result
    ///
    /// Interrupt handling: when the user sends `UserMessage::Interrupt` during
    /// streaming, the assembler produces a partial response and the loop continues.
    #[allow(clippy::too_many_arguments)]
    pub async fn run_interactive_stream(
        &mut self,
        input_rx: &mut tokio::sync::mpsc::Receiver<super::interactive::UserMessage>,
        signal_tx: &tokio::sync::mpsc::Sender<super::interactive::InteractiveSignal>,
        credential_set: &CredentialSet,
        skill_set: &SkillSet,
        agents_md: &Option<crate::agents_md::AgentsMdContent>,
        session_store: Option<&dyn SessionStorage>,
        compaction_config: Option<&CompactionConfig>,
    ) -> Result<AgentResult, AgentError> {
        use super::interactive::{InteractiveSignal, StreamAssembler, UserMessage};
        use crate::llm::types::compute_cost;
        use tokio_stream::StreamExt as _;

        // Validate reasoning stages config if present
        if let Some(ref stages_config) = self.config.reasoning_stages {
            if let Err(e) = super::reasoning_stages::validate_config(stages_config) {
                warn!("{}", e);
            }
        }

        // Wait for the first user message to start the conversation
        let first_task = loop {
            match input_rx.recv().await {
                Some(UserMessage::Chat(msg)) => break msg,
                Some(UserMessage::Quit) => {
                    let state = AgentState::new("");
                    return Ok(state.into_result(AgentStatus::Success, Some(String::new())));
                }
                Some(UserMessage::Interrupt) => continue,
                None => {
                    return Err(AgentError::Llm(
                        "Input channel closed before first message".to_string(),
                    ));
                }
            }
        };

        let mut state = AgentState::new(&first_task);

        events::emit(
            &self.event_bus,
            AgentEvent::AgentStarted {
                task: first_task.clone(),
                timestamp_ms: chrono::Utc::now().timestamp_millis() as u64,
            },
        );

        // Build system prompt (same logic as run_with_options)
        let system_prompt = self.build_system_blocks(credential_set, skill_set, agents_md);

        // Tool definitions
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
        let mut nudge_count: u32 = 0;
        let mut goal_check_fired = false;

        loop {
            // Check iteration limit
            if state.current_iteration() >= self.config.max_iterations {
                info!(
                    iterations = state.current_iteration(),
                    "Max iterations reached"
                );
                self.emit_completed(AgentStatus::MaxIterations, None, &state);
                return Ok(state.into_result(AgentStatus::MaxIterations, None));
            }

            // Budget check. `max_budget_usd` was declared in config and documented but
            // never compared against anything, so a runaway agent could spend without
            // limit while the knob appeared to be doing something.
            if let Some(max_budget) = self.config.max_budget_usd {
                if state.total_cost() >= max_budget {
                    info!(
                        spent_usd = state.total_cost(),
                        budget_usd = max_budget,
                        "Budget exhausted"
                    );
                    self.emit_completed(AgentStatus::BudgetExceeded, None, &state);
                    return Ok(state.into_result(AgentStatus::BudgetExceeded, None));
                }
            }

            // Check timeout

            if state.elapsed_ms() >= timeout_ms {
                info!(elapsed_ms = state.elapsed_ms(), "Timeout reached");
                self.emit_completed(AgentStatus::Timeout, None, &state);
                return Ok(state.into_result(AgentStatus::Timeout, None));
            }

            // Progressive context compaction (no dedicated compaction LLM in interactive mode)
            if let Some(compact_config) = compaction_config {
                self.run_compaction(compact_config, &mut state, None).await;
            }

            state.increment_iteration();
            debug!(iteration = state.current_iteration(), "Starting iteration");

            events::emit(
                &self.event_bus,
                AgentEvent::IterationStarted {
                    iteration: state.current_iteration(),
                },
            );

            // Action progress reminder
            if let Some(reminder) = self.build_action_reminder(&state) {
                state.inject_system_notification(&reminder);
            }

            // Iteration budget warning (one-time)
            if let Some(warning) = self.build_budget_warning(&state) {
                state.inject_system_notification(&warning);
            }

            // Loop detection
            if let Some(ref loop_config) = self.config.loop_detection {
                if let Some(warning) =
                    super::loop_detection::detect_loop(state.steps(), loop_config)
                {
                    state.inject_system_notification(&warning);
                }
                if let Some(warning) =
                    super::loop_detection::detect_semantic_loop(state.steps(), loop_config)
                {
                    state.inject_system_notification(&warning);
                }
            }

            // System reminders
            if !reminder_scheduler.is_empty() {
                let reminders =
                    reminder_scheduler.check(state.current_iteration(), &Vec::<String>::new());
                for reminder in reminders {
                    state.inject_system_notification(&reminder);
                }
            }

            // Reasoning budget stages
            if let (Some(ref stages_config), Some(ref thinking_ctl)) =
                (&self.config.reasoning_stages, &self.thinking_control)
            {
                let budget = super::reasoning_stages::compute_thinking_budget(
                    stages_config,
                    state.current_iteration(),
                    self.config.max_iterations,
                );
                thinking_ctl
                    .set_thinking_config(Some(crate::llm::types::ThinkingConfig::enabled(budget)));
            }

            // Resolve effective tool definitions
            let effective_tool_defs: std::borrow::Cow<'_, [ToolDefinition]> =
                if let Some(ref registry) = tool_registry {
                    let discovered = registry.discovered_definitions();
                    if self.config.plan_mode {
                        std::borrow::Cow::Owned(Self::filter_read_only_tools(&discovered))
                    } else {
                        std::borrow::Cow::Owned(discovered)
                    }
                } else if self.config.plan_mode {
                    std::borrow::Cow::Owned(Self::filter_read_only_tools(&all_tool_defs))
                } else {
                    std::borrow::Cow::Borrowed(&all_tool_defs)
                };

            // Stream the LLM response and assemble it.
            // The stream borrows state.messages(), so we scope it to release the
            // borrow before mutating state.
            enum StreamOutcome {
                Complete(MessagesResponse),
                Interrupted(MessagesResponse),
                Quit(MessagesResponse),
                Error(AgentError),
            }

            let outcome = {
                let filtered_messages = Self::filter_thinking_blocks(state.messages());
                let mut stream = self.llm.send_messages_stream(
                    system_prompt.as_deref(),
                    &filtered_messages,
                    Some(&effective_tool_defs),
                );

                let mut assembler = StreamAssembler::new();

                enum StreamExit {
                    Interrupt,
                    Quit,
                    Closed,
                }
                let mut exit_reason: Option<StreamExit> = None;

                loop {
                    tokio::select! {
                        biased;
                        msg = input_rx.recv() => {
                            match msg {
                                Some(UserMessage::Interrupt) => {
                                    info!("User interrupted streaming");
                                    exit_reason = Some(StreamExit::Interrupt);
                                    break;
                                }
                                Some(UserMessage::Quit) => {
                                    exit_reason = Some(StreamExit::Quit);
                                    break;
                                }
                                Some(UserMessage::Chat(_)) => {
                                    // Ignore during streaming
                                }
                                None => {
                                    exit_reason = Some(StreamExit::Closed);
                                    break;
                                }
                            }
                        }
                        event_result = stream.next() => {
                            match event_result {
                                Some(Ok(event)) => {
                                    let deltas = assembler.process_event(&event);
                                    self.emit_stream_deltas(&deltas);
                                }
                                Some(Err(e)) => {
                                    warn!(error = %e, "Stream error");
                                    drop(stream);
                                    return Err(e);
                                }
                                None => break, // Stream ended
                            }
                        }
                    }
                }

                drop(stream);

                match exit_reason {
                    Some(StreamExit::Quit) => match assembler.partial_finish() {
                        Ok(r) => StreamOutcome::Quit(r),
                        Err(e) => StreamOutcome::Error(e),
                    },
                    Some(StreamExit::Interrupt) => match assembler.partial_finish() {
                        Ok(r) => StreamOutcome::Interrupted(r),
                        Err(e) => StreamOutcome::Error(e),
                    },
                    Some(StreamExit::Closed) => {
                        StreamOutcome::Error(AgentError::Llm("Input channel closed".to_string()))
                    }
                    None => match assembler.finish() {
                        Ok(r) => StreamOutcome::Complete(r),
                        Err(e) => StreamOutcome::Error(e),
                    },
                }
            };

            // Handle outcome - state borrow is now released
            let (response, interrupted) = match outcome {
                StreamOutcome::Error(e) => return Err(e),
                StreamOutcome::Quit(response) => {
                    state.accumulate_usage(response.usage.as_ref(), &response.model);
                    if !response.content.is_empty() {
                        state.add_assistant_message(response.content);
                    }
                    self.emit_completed(AgentStatus::Success, None, &state);
                    return Ok(state.into_result(AgentStatus::Success, None));
                }
                StreamOutcome::Interrupted(response) => (response, true),
                StreamOutcome::Complete(response) => (response, false),
            };

            state.accumulate_usage(response.usage.as_ref(), &response.model);

            // Emit final token usage
            if let Some(ref usage) = response.usage {
                let cost = compute_cost(&response.model, usage.input_tokens, usage.output_tokens);
                events::emit(
                    &self.event_bus,
                    AgentEvent::TokenUsage {
                        input_tokens: usage.input_tokens,
                        output_tokens: usage.output_tokens,
                        cache_read_tokens: usage.cache_read_input_tokens,
                        cache_write_tokens: usage.cache_creation_input_tokens,
                        total_cost_usd: Some(cost),
                    },
                );
            }

            if interrupted {
                // On interrupt: save partial response and go back to awaiting input
                if !response.content.is_empty() {
                    state.add_assistant_message(response.content);
                }
                let _ = signal_tx.send(InteractiveSignal::AwaitingInput).await;
                events::emit(&self.event_bus, AgentEvent::AwaitingInput);

                // Wait for next user message
                match input_rx.recv().await {
                    Some(UserMessage::Chat(msg)) => {
                        state.inject_system_notification(&msg);
                        nudge_count = 0;
                        goal_check_fired = false;
                        continue;
                    }
                    Some(UserMessage::Quit) => {
                        self.emit_completed(AgentStatus::Success, None, &state);
                        return Ok(state.into_result(AgentStatus::Success, None));
                    }
                    Some(UserMessage::Interrupt) => continue,
                    None => {
                        return Err(AgentError::Llm("Input channel closed".to_string()));
                    }
                }
            }

            // Process the assembled response (same logic as batch loop). As there, a
            // response carrying tool_use blocks is executed regardless of the reported
            // stop_reason.
            let effective_stop_reason = if content_has_tool_use(&response.content) {
                StopReason::ToolUse
            } else {
                response.stop_reason
            };

            match effective_stop_reason {
                StopReason::ToolUse => {
                    let assistant_content = response.content;
                    let has_tool_use = content_has_tool_use(&assistant_content);

                    if !has_tool_use {
                        let Some(assistant_content) =
                            self.try_nudge(&mut state, assistant_content, &mut nudge_count)
                        else {
                            continue;
                        };
                        let Some(assistant_content) = self.try_goal_check(
                            &mut state,
                            assistant_content,
                            &mut goal_check_fired,
                        ) else {
                            continue;
                        };
                        // No tool_use despite stop_reason=tool_use: treat as end_turn
                        warn!("LLM returned stop_reason=tool_use but no tool_use blocks; treating as end_turn");
                        let final_text = Self::extract_text(&assistant_content);
                        state.add_assistant_message(assistant_content);

                        let _ = signal_tx.send(InteractiveSignal::AwaitingInput).await;
                        events::emit(&self.event_bus, AgentEvent::AwaitingInput);

                        // Wait for next user message
                        match input_rx.recv().await {
                            Some(UserMessage::Chat(msg)) => {
                                state.inject_system_notification(&msg);
                                nudge_count = 0;
                                goal_check_fired = false;
                                continue;
                            }
                            Some(UserMessage::Quit) => {
                                self.emit_completed(
                                    AgentStatus::Success,
                                    Some(final_text.clone()),
                                    &state,
                                );
                                return Ok(
                                    state.into_result(AgentStatus::Success, Some(final_text))
                                );
                            }
                            Some(UserMessage::Interrupt) => continue,
                            None => {
                                return Err(AgentError::Llm("Input channel closed".to_string()));
                            }
                        }
                    }

                    // Execute tools
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

                    // Persist session
                    self.persist_session_iteration(session_store, &None, &state)
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
                StopReason::EndTurn
                | StopReason::MaxTokens
                | StopReason::StopSequence
                | StopReason::Unknown => {
                    if effective_stop_reason == StopReason::Unknown {
                        warn!(
                            stop_reason = ?response.stop_reason,
                            "Unrecognized stop_reason from the API; treating as end_turn"
                        );
                    }
                    let content = if !content_has_tool_use(&response.content) {
                        let Some(content) =
                            self.try_nudge(&mut state, response.content, &mut nudge_count)
                        else {
                            continue;
                        };
                        content
                    } else {
                        response.content
                    };

                    let Some(content) =
                        self.try_goal_check(&mut state, content, &mut goal_check_fired)
                    else {
                        continue;
                    };

                    state.add_assistant_message(content);

                    // Signal TUI that we're ready for input
                    let _ = signal_tx.send(InteractiveSignal::AwaitingInput).await;
                    events::emit(&self.event_bus, AgentEvent::AwaitingInput);

                    // Wait for next user message
                    match input_rx.recv().await {
                        Some(UserMessage::Chat(msg)) => {
                            state.inject_system_notification(&msg);
                            nudge_count = 0;
                            goal_check_fired = false;
                            continue;
                        }
                        Some(UserMessage::Quit) => {
                            let final_text = Self::extract_final_text(&state);
                            self.emit_completed(
                                AgentStatus::Success,
                                Some(final_text.clone()),
                                &state,
                            );
                            return Ok(state.into_result(AgentStatus::Success, Some(final_text)));
                        }
                        Some(UserMessage::Interrupt) => continue,
                        None => {
                            return Err(AgentError::Llm("Input channel closed".to_string()));
                        }
                    }
                }
            }
        }
    }

    /// Emit AgentEvents corresponding to stream deltas.
    fn emit_stream_deltas(&self, deltas: &[super::interactive::StreamDelta]) {
        for delta in deltas {
            match delta {
                super::interactive::StreamDelta::TextDelta(text) => {
                    events::emit(
                        &self.event_bus,
                        AgentEvent::TextDelta { text: text.clone() },
                    );
                }
                super::interactive::StreamDelta::ThinkingDelta(text) => {
                    events::emit(
                        &self.event_bus,
                        AgentEvent::ThinkingDelta { text: text.clone() },
                    );
                }
                super::interactive::StreamDelta::ToolUseStart { id, name } => {
                    events::emit(
                        &self.event_bus,
                        AgentEvent::ToolUseStart {
                            id: id.clone(),
                            name: name.clone(),
                            input: Value::Null,
                        },
                    );
                }
                super::interactive::StreamDelta::UsageUpdate {
                    input_tokens,
                    output_tokens,
                    cache_read,
                    cache_write,
                    cost,
                } => {
                    events::emit(
                        &self.event_bus,
                        AgentEvent::TokenUsage {
                            input_tokens: *input_tokens,
                            output_tokens: *output_tokens,
                            cache_read_tokens: *cache_read,
                            cache_write_tokens: *cache_write,
                            total_cost_usd: *cost,
                        },
                    );
                }
                super::interactive::StreamDelta::InputJsonDelta(_) => {
                    // Input JSON deltas don't map to an AgentEvent
                }
            }
        }
    }

    /// Extract text content from content blocks.
    fn extract_text(content: &[ContentBlock]) -> String {
        content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Extract the final text from the last assistant message in state.
    fn extract_final_text(state: &AgentState) -> String {
        state
            .messages()
            .iter()
            .rev()
            .find(|m| matches!(m.role, crate::llm::types::Role::Assistant))
            .map(|m| Self::extract_text(&m.content))
            .unwrap_or_default()
    }

    /// Build system prompt blocks (shared logic for interactive and batch modes).
    fn build_system_blocks(
        &self,
        credentials: &CredentialSet,
        skill_set: &SkillSet,
        agents_md: &Option<crate::agents_md::AgentsMdContent>,
    ) -> Option<Vec<SystemContent>> {
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
        if system_blocks.is_empty() {
            None
        } else {
            Some(system_blocks)
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

    /// A capturing LLM mock that records all messages sent to it.
    /// Shared across tests that need to inspect injected messages.
    struct CapturingLlm {
        responses: Arc<Mutex<Vec<MessagesResponse>>>,
        captured_messages: Arc<Mutex<Vec<Vec<Message>>>>,
    }

    #[async_trait]
    impl LlmProvider for CapturingLlm {
        async fn send_messages(
            &self,
            _system: Option<&[SystemContent]>,
            messages: &[Message],
            _tools: Option<&[ToolDefinition]>,
        ) -> Result<MessagesResponse, AgentError> {
            self.captured_messages
                .lock()
                .unwrap()
                .push(messages.to_vec());
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                Err(AgentError::Llm("No more mock responses".into()))
            } else {
                Ok(responses.remove(0))
            }
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
            nudge_on_text_only: false,
            nudge_max_count: 3,
            goal_check_on_complete: false,
            action_reminder_interval: None,
            loop_detection: None,
            reasoning_stages: None,
            iteration_budget_warning_threshold: None,
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
            ..default_config()
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
            system_prompt: Some("You are a browser agent.".to_string()),
            ..default_config()
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
        struct SystemCapturingLlm {
            captured_system: Arc<Mutex<Option<String>>>,
        }

        #[async_trait]
        impl LlmProvider for SystemCapturingLlm {
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
        let llm = SystemCapturingLlm {
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
            signature: "test-sig".to_string(),
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

    #[tokio::test]
    async fn test_nudge_on_text_only_continues_loop() {
        let config = AgentConfig {
            nudge_on_text_only: true,
            nudge_max_count: 2,
            ..default_config()
        };

        // With nudge_max_count=2: nudge on responses 1 & 2, then terminate on response 3
        let llm = MockLlm {
            responses: Arc::new(Mutex::new(vec![
                make_end_turn_response("Let me analyze this..."),
                make_end_turn_response("I think the approach should be..."),
                make_end_turn_response("Done"),
            ])),
        };
        let tools = MockTools {
            tools: default_tools(),
            results: Arc::new(Mutex::new(vec![])),
        };

        let runner = AgentRunner::new(llm, tools, config);
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
        // Nudged twice (iterations 1 and 2), then terminated on 3rd
        assert_eq!(
            result.total_iterations, 3,
            "Expected 3 iterations (2 nudges + termination), got {}",
            result.total_iterations
        );
    }

    #[tokio::test]
    async fn test_nudge_disabled_terminates_immediately() {
        let config = AgentConfig {
            nudge_on_text_only: false,
            ..default_config()
        };

        let llm = MockLlm {
            responses: Arc::new(Mutex::new(vec![make_end_turn_response(
                "Here is my analysis...",
            )])),
        };
        let tools = MockTools {
            tools: default_tools(),
            results: Arc::new(Mutex::new(vec![])),
        };

        let runner = AgentRunner::new(llm, tools, config);
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
            result.total_iterations, 1,
            "Expected immediate termination with nudge disabled"
        );
    }

    #[tokio::test]
    async fn test_goal_check_fires_once_before_termination() {
        let config = AgentConfig {
            goal_check_on_complete: true,
            ..default_config()
        };

        // 6 tool-use iterations (exceeds MIN_ITERATIONS_FOR_GOAL_CHECK=5),
        // then end_turn triggers goal check, then verified end_turn terminates.
        let make_tool_result = || {
            Ok(ToolExecutionResult {
                content: "Page loaded".to_string(),
                is_error: false,
            })
        };
        let llm = MockLlm {
            responses: Arc::new(Mutex::new(vec![
                make_tool_use_response("t1", "navigate", json!({"url": "a.com"})),
                make_tool_use_response("t2", "navigate", json!({"url": "b.com"})),
                make_tool_use_response("t3", "navigate", json!({"url": "c.com"})),
                make_tool_use_response("t4", "navigate", json!({"url": "d.com"})),
                make_tool_use_response("t5", "navigate", json!({"url": "e.com"})),
                make_tool_use_response("t6", "navigate", json!({"url": "f.com"})),
                make_end_turn_response("Done"),
                make_end_turn_response("Verified done"),
            ])),
        };
        let tools = MockTools {
            tools: default_tools(),
            results: Arc::new(Mutex::new(vec![
                make_tool_result(),
                make_tool_result(),
                make_tool_result(),
                make_tool_result(),
                make_tool_result(),
                make_tool_result(),
            ])),
        };

        let runner = AgentRunner::new(llm, tools, config);
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
            result.total_iterations, 8,
            "Expected 8 iterations (6 tool work, goal check fires, terminates)"
        );
        assert_eq!(result.result, Some("Verified done".to_string()));
    }

    #[tokio::test]
    async fn test_goal_check_disabled_terminates_immediately() {
        let config = AgentConfig {
            goal_check_on_complete: false,
            ..default_config()
        };

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
                content: "Page loaded".to_string(),
                is_error: false,
            })])),
        };

        let runner = AgentRunner::new(llm, tools, config);
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
            result.total_iterations, 2,
            "Expected 2 iterations (backward compatible, no goal check)"
        );
    }

    #[tokio::test]
    async fn test_goal_check_skips_when_no_tool_work() {
        let config = AgentConfig {
            goal_check_on_complete: true,
            ..default_config()
        };

        // Only one response, no tool work done (iteration 1)
        let llm = MockLlm {
            responses: Arc::new(Mutex::new(vec![make_end_turn_response("Nothing to do")])),
        };
        let tools = MockTools {
            tools: default_tools(),
            results: Arc::new(Mutex::new(vec![])),
        };

        let runner = AgentRunner::new(llm, tools, config);
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
            result.total_iterations, 1,
            "Expected 1 iteration (no tool work, skip goal check)"
        );
        assert_eq!(result.result, Some("Nothing to do".to_string()));
    }

    #[tokio::test]
    async fn test_nudge_exhausted_terminates() {
        let config = AgentConfig {
            nudge_on_text_only: true,
            nudge_max_count: 2,
            ..default_config()
        };

        let llm = MockLlm {
            responses: Arc::new(Mutex::new(vec![
                make_end_turn_response("Analysis part 1..."),
                make_end_turn_response("Analysis part 2..."),
                make_end_turn_response("Analysis part 3..."),
            ])),
        };
        let tools = MockTools {
            tools: default_tools(),
            results: Arc::new(Mutex::new(vec![])),
        };

        let runner = AgentRunner::new(llm, tools, config);
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
        // 2 nudges exhausted, then 3rd iteration terminates normally
        assert_eq!(
            result.total_iterations, 3,
            "Expected 3 iterations (2 nudges exhausted, 3rd terminates)"
        );
        assert_eq!(result.result, Some("Analysis part 3...".to_string()));
    }

    #[tokio::test]
    async fn test_action_reminder_fires_on_interval() {
        // Config with interval 2 and max_iterations 10.
        // We run 4 iterations of tool use, then end_turn on 5th.
        // Reminder should fire at iterations 2 and 4 (but not 1 or 3).
        let config = AgentConfig {
            action_reminder_interval: Some(2),
            max_iterations: 10,
            ..default_config()
        };

        let captured = Arc::new(Mutex::new(Vec::new()));
        let llm = CapturingLlm {
            responses: Arc::new(Mutex::new(vec![
                make_tool_use_response("t1", "navigate", json!({"url": "a.com"})), // iteration 1
                make_tool_use_response("t2", "navigate", json!({"url": "b.com"})), // iteration 2 (reminder fires before this call)
                make_tool_use_response("t3", "navigate", json!({"url": "c.com"})), // iteration 3
                make_tool_use_response("t4", "navigate", json!({"url": "d.com"})), // iteration 4 (reminder fires before this call)
                make_end_turn_response("Done"),                                    // iteration 5
            ])),
            captured_messages: captured.clone(),
        };
        let tools = MockTools {
            tools: default_tools(),
            results: Arc::new(Mutex::new(vec![])),
        };

        let runner = AgentRunner::new(llm, tools, config);
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
        assert_eq!(result.total_iterations, 5);

        let all_calls = captured.lock().unwrap();
        // 5 LLM calls total (iterations 1-5)
        assert_eq!(all_calls.len(), 5);

        // Helper: count how many messages contain "URGENT" or "Progress check" in this call
        // vs the previous call. A new reminder was injected if this call has more such messages.
        let reminder_count = |call_idx: usize| -> usize {
            all_calls[call_idx]
                .iter()
                .filter(|msg| {
                    msg.content.iter().any(|block| match block {
                        ContentBlock::Text { text } => {
                            text.contains("URGENT") || text.contains("Progress check")
                        }
                        _ => false,
                    })
                })
                .count()
        };

        let count_0 = reminder_count(0); // iteration 1
        let count_1 = reminder_count(1); // iteration 2
        let count_2 = reminder_count(2); // iteration 3
        let count_3 = reminder_count(3); // iteration 4

        // Iteration 1: no reminder
        assert_eq!(count_0, 0, "Should not have reminder on iteration 1");
        // Iteration 2: first reminder fires (count increases by 1)
        assert_eq!(
            count_1,
            count_0 + 1,
            "Should have new reminder on iteration 2"
        );
        // Iteration 3: no new reminder (count stays the same)
        assert_eq!(
            count_2, count_1,
            "Should not have new reminder on iteration 3"
        );
        // Iteration 4: second reminder fires (count increases by 1)
        assert_eq!(
            count_3,
            count_2 + 1,
            "Should have new reminder on iteration 4"
        );
    }

    #[tokio::test]
    async fn test_action_reminder_disabled_by_default() {
        // Default config has action_reminder_interval: None.
        // Verify no reminders are injected.
        let captured = Arc::new(Mutex::new(Vec::new()));
        let llm = CapturingLlm {
            responses: Arc::new(Mutex::new(vec![
                make_tool_use_response("t1", "navigate", json!({"url": "a.com"})),
                make_tool_use_response("t2", "navigate", json!({"url": "b.com"})),
                make_end_turn_response("Done"),
            ])),
            captured_messages: captured.clone(),
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

        let all_calls = captured.lock().unwrap();
        for (i, call) in all_calls.iter().enumerate() {
            let has_reminder = call.iter().any(|msg| {
                msg.content.iter().any(|block| match block {
                    ContentBlock::Text { text } => {
                        text.contains("URGENT") || text.contains("Progress check")
                    }
                    _ => false,
                })
            });
            assert!(
                !has_reminder,
                "Should not have reminder on any iteration (disabled), but found one on call {i}"
            );
        }
    }

    #[test]
    fn test_build_action_reminder_no_files_written() {
        let config = AgentConfig {
            action_reminder_interval: Some(2),
            max_iterations: 10,
            ..default_config()
        };
        let llm = MockLlm {
            responses: Arc::new(Mutex::new(vec![])),
        };
        let tools = MockTools {
            tools: default_tools(),
            results: Arc::new(Mutex::new(vec![])),
        };
        let runner = AgentRunner::new(llm, tools, config);

        let mut state = AgentState::new("test task");

        // iteration 0 -> no reminder
        assert!(runner.build_action_reminder(&state).is_none());

        // iteration 1 -> no reminder (1 % 2 != 0)
        state.increment_iteration();
        assert!(runner.build_action_reminder(&state).is_none());

        // iteration 2 -> URGENT reminder (no files written)
        state.increment_iteration();
        let reminder = runner.build_action_reminder(&state).unwrap();
        assert!(reminder.contains("URGENT"));
        assert!(reminder.contains("NOT written any output files"));
        assert!(reminder.contains("iteration 2 of 10"));
    }

    #[test]
    fn test_build_action_reminder_with_files_written() {
        let config = AgentConfig {
            action_reminder_interval: Some(2),
            max_iterations: 10,
            ..default_config()
        };
        let llm = MockLlm {
            responses: Arc::new(Mutex::new(vec![])),
        };
        let tools = MockTools {
            tools: default_tools(),
            results: Arc::new(Mutex::new(vec![])),
        };
        let runner = AgentRunner::new(llm, tools, config);

        let mut state = AgentState::new("test task");
        state.increment_iteration();
        state.increment_iteration();

        // Record a write_file step
        state.record_step(StepRecord {
            iteration: 1,
            tool: "write_file".to_string(),
            input: json!({"path": "out.txt"}),
            output: json!("ok"),
            duration_ms: 10,
            is_error: None,
        });

        let reminder = runner.build_action_reminder(&state).unwrap();
        assert!(reminder.contains("Progress check"));
        assert!(reminder.contains("written files - good"));
        assert!(!reminder.contains("URGENT"));
    }

    #[test]
    fn test_build_action_reminder_none_when_disabled() {
        let config = AgentConfig {
            action_reminder_interval: None,
            ..default_config()
        };
        let llm = MockLlm {
            responses: Arc::new(Mutex::new(vec![])),
        };
        let tools = MockTools {
            tools: default_tools(),
            results: Arc::new(Mutex::new(vec![])),
        };
        let runner = AgentRunner::new(llm, tools, config);

        let mut state = AgentState::new("test task");
        state.increment_iteration();
        state.increment_iteration();

        assert!(runner.build_action_reminder(&state).is_none());
    }

    #[test]
    fn test_build_action_reminder_interval_zero() {
        let config = AgentConfig {
            action_reminder_interval: Some(0),
            ..default_config()
        };
        let llm = MockLlm {
            responses: Arc::new(Mutex::new(vec![])),
        };
        let tools = MockTools {
            tools: default_tools(),
            results: Arc::new(Mutex::new(vec![])),
        };
        let runner = AgentRunner::new(llm, tools, config);

        let mut state = AgentState::new("test");
        state.increment_iteration();
        state.increment_iteration();

        assert!(
            runner.build_action_reminder(&state).is_none(),
            "interval=0 should disable reminders"
        );
    }

    #[test]
    fn test_build_action_reminder_interval_one() {
        let config = AgentConfig {
            action_reminder_interval: Some(1),
            max_iterations: 5,
            ..default_config()
        };
        let llm = MockLlm {
            responses: Arc::new(Mutex::new(vec![])),
        };
        let tools = MockTools {
            tools: default_tools(),
            results: Arc::new(Mutex::new(vec![])),
        };
        let runner = AgentRunner::new(llm, tools, config);

        let mut state = AgentState::new("test");

        // iteration 0 -> no reminder
        assert!(runner.build_action_reminder(&state).is_none());

        // iteration 1 -> fires (every iteration)
        state.increment_iteration();
        assert!(runner.build_action_reminder(&state).is_some());

        // iteration 2 -> fires
        state.increment_iteration();
        assert!(runner.build_action_reminder(&state).is_some());
    }

    fn make_runner_with_config(config: AgentConfig) -> AgentRunner<MockLlm, MockTools> {
        let llm = MockLlm {
            responses: Arc::new(Mutex::new(vec![])),
        };
        let tools = MockTools {
            tools: vec![],
            results: Arc::new(Mutex::new(vec![])),
        };
        AgentRunner::new(llm, tools, config)
    }

    #[test]
    fn test_budget_warning_not_configured() {
        let config = AgentConfig {
            max_iterations: 50,
            ..default_config()
        };
        let runner = make_runner_with_config(config);
        let mut state = AgentState::new("test");
        for _ in 0..40 {
            state.increment_iteration();
        }
        assert!(runner.build_budget_warning(&state).is_none());
    }

    #[test]
    fn test_budget_warning_fires_at_threshold() {
        let config = AgentConfig {
            max_iterations: 50,
            iteration_budget_warning_threshold: Some(0.7),
            ..default_config()
        };
        let runner = make_runner_with_config(config);
        let mut state = AgentState::new("test");

        // 0.7 * 50 = 35, ceil = 35 → fires at iteration 35
        for _ in 0..34 {
            state.increment_iteration();
            assert!(runner.build_budget_warning(&state).is_none());
        }
        state.increment_iteration(); // iteration 35
        let warning = runner.build_budget_warning(&state);
        assert!(warning.is_some());
        assert!(warning.unwrap().contains("BUDGET WARNING"));
    }

    #[test]
    fn test_budget_warning_fires_only_once() {
        let config = AgentConfig {
            max_iterations: 50,
            iteration_budget_warning_threshold: Some(0.7),
            ..default_config()
        };
        let runner = make_runner_with_config(config);
        let mut state = AgentState::new("test");

        // Advance to iteration 35 (threshold)
        for _ in 0..35 {
            state.increment_iteration();
        }
        assert!(runner.build_budget_warning(&state).is_some());

        // Iteration 36 → should not fire again
        state.increment_iteration();
        assert!(runner.build_budget_warning(&state).is_none());
    }

    #[test]
    fn test_budget_warning_at_100_percent() {
        let config = AgentConfig {
            max_iterations: 10,
            iteration_budget_warning_threshold: Some(1.0),
            ..default_config()
        };
        let runner = make_runner_with_config(config);
        let mut state = AgentState::new("test");
        for _ in 0..10 {
            state.increment_iteration();
        }
        assert!(runner.build_budget_warning(&state).is_some());
    }
}
