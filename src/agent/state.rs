use std::time::Instant;

use crate::llm::types::{compute_cost_with_cache, ContentBlock, Message, Role, ToolResultContent};
use crate::output::result::{AgentResult, AgentStatus, StepRecord};
use crate::session::types::SessionSnapshot;

pub struct AgentState {
    messages: Vec<Message>,
    steps: Vec<StepRecord>,
    iteration: u32,
    start_time: Instant,
    total_input_tokens: u32,
    total_output_tokens: u32,
    total_cost: f64,
    /// Tracks the effective context size for compaction decisions.
    /// Unlike `total_input_tokens` (which is cumulative for reporting),
    /// this resets after compaction to reflect the actual context window usage.
    effective_input_tokens: u32,
    /// Cached flag: true once a `write_file` or `edit_file` step has been recorded.
    has_written_files: bool,
    /// The compaction stage most recently applied, and the context size at the time.
    /// Used to stop a stage from re-running when it cannot reduce the context further.
    last_compaction: Option<(crate::agent::compaction_stages::CompactionStage, u32)>,
    /// Loop-detection warnings already delivered, so a tripped detector nags once
    /// rather than on every iteration until its window rolls over.
    delivered_warnings: std::collections::HashSet<String>,
}

impl AgentState {
    pub fn new(task: &str) -> Self {
        Self {
            messages: vec![Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: task.to_string(),
                }],
            }],
            steps: Vec::new(),
            iteration: 0,
            start_time: Instant::now(),
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cost: 0.0,
            effective_input_tokens: 0,
            has_written_files: false,
            last_compaction: None,
            delivered_warnings: std::collections::HashSet::new(),
        }
    }

    /// Restore state from a session snapshot (used when resuming a session).
    pub fn from_snapshot(snapshot: &SessionSnapshot) -> Self {
        let has_written_files = snapshot
            .steps
            .iter()
            .any(|s| s.tool == "write_file" || s.tool == "edit_file");
        Self {
            messages: snapshot.messages.clone(),
            steps: snapshot.steps.clone(),
            iteration: snapshot.iteration,
            start_time: Instant::now(),
            total_input_tokens: snapshot.metadata.total_input_tokens.unwrap_or(0),
            total_output_tokens: snapshot.metadata.total_output_tokens.unwrap_or(0),
            total_cost: 0.0,
            effective_input_tokens: 0,
            has_written_files,
            last_compaction: None,
            delivered_warnings: std::collections::HashSet::new(),
        }
    }

    pub fn add_assistant_message(&mut self, content: Vec<ContentBlock>) {
        self.messages.push(Message {
            role: Role::Assistant,
            content,
        });
    }

    pub fn add_tool_results(&mut self, results: Vec<ContentBlock>) {
        self.messages.push(Message {
            role: Role::User,
            content: results,
        });
    }

    /// Append an assistant message whose tool calls are being refused, answering every
    /// `tool_use` block it contains with a synthetic error `tool_result` carrying
    /// `feedback`.
    ///
    /// Any path that appends an assistant message without executing its tools must go
    /// through this. Appending the message and then pushing a plain user text message
    /// leaves the `tool_use` unanswered, and the API rejects the next request with a
    /// 400. If the content carries no tool calls this is equivalent to
    /// [`Self::add_assistant_message`].
    pub fn add_assistant_message_refusing_tools(
        &mut self,
        content: Vec<ContentBlock>,
        feedback: &str,
    ) {
        let synthetic: Vec<ContentBlock> = content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolUse { id, .. } => Some(ContentBlock::ToolResult {
                    tool_use_id: id.clone(),
                    content: ToolResultContent::Text(feedback.to_string()),
                    is_error: Some(true),
                }),
                _ => None,
            })
            .collect();

        self.add_assistant_message(content);

        if synthetic.is_empty() {
            self.inject_system_notification(feedback);
        } else {
            self.add_tool_results(synthetic);
        }
    }

    pub fn record_step(&mut self, step: StepRecord) {
        if step.tool == "write_file" || step.tool == "edit_file" {
            self.has_written_files = true;
        }
        self.steps.push(step);
    }

    /// Returns `true` if any recorded step used `write_file` or `edit_file`.
    pub fn has_written_files(&self) -> bool {
        self.has_written_files
    }

    pub fn increment_iteration(&mut self) {
        self.iteration += 1;
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Extract the original task text from the first user message.
    pub fn original_task(&self) -> Option<&str> {
        self.messages.first().and_then(|msg| {
            msg.content.iter().find_map(|block| {
                if let ContentBlock::Text { text } = block {
                    Some(text.as_str())
                } else {
                    None
                }
            })
        })
    }

    /// Get mutable access to messages (for in-place compaction stages).
    pub fn messages_mut(&mut self) -> &mut [Message] {
        &mut self.messages
    }

    /// Replace the entire message history (used by compaction stages).
    pub fn replace_messages(&mut self, messages: Vec<Message>) {
        self.messages = messages;
    }

    pub fn current_iteration(&self) -> u32 {
        self.iteration
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.start_time.elapsed().as_millis() as u64
    }

    pub fn accumulate_usage(&mut self, usage: Option<&crate::llm::types::Usage>, model: &str) {
        if let Some(u) = usage {
            self.total_input_tokens += u.input_tokens;
            self.total_output_tokens += u.output_tokens;

            // `effective_input_tokens` tracks the *current* context size, so it is
            // assigned, not accumulated. Each response's `input_tokens` already counts
            // the whole prompt; summing them across iterations measures cumulative
            // spend and makes compaction fire on iteration count rather than on real
            // context pressure.
            //
            // Cached prefix tokens are counted separately by the API but still occupy
            // the context window, so they belong in the total.
            self.effective_input_tokens = u.input_tokens
                + u.cache_read_input_tokens.unwrap_or(0)
                + u.cache_creation_input_tokens.unwrap_or(0);

            // Cache reads and writes are billed separately from `input_tokens`.
            self.total_cost += compute_cost_with_cache(model, u);
        }
    }

    pub fn total_cost(&self) -> f64 {
        self.total_cost
    }

    pub fn total_input_tokens(&self) -> u32 {
        self.total_input_tokens
    }

    pub fn total_output_tokens(&self) -> u32 {
        self.total_output_tokens
    }

    /// Returns the effective input token count for compaction decisions.
    /// This tracks the actual context size and resets after compaction,
    /// unlike `total_input_tokens` which is cumulative for reporting.
    pub fn effective_input_tokens(&self) -> u32 {
        self.effective_input_tokens
    }

    /// Reset effective input tokens after compaction.
    /// `estimated_tokens` is a rough estimate of the compacted context size.
    pub fn reset_effective_tokens(&mut self, estimated_tokens: u32) {
        self.effective_input_tokens = estimated_tokens;
    }

    /// Whether `stage` should run, given what already ran and at what context size.
    ///
    /// A stage is worth repeating only once the context has grown past the point where
    /// it last ran. Otherwise the same stage fires on every iteration, rewriting the
    /// message list without reducing anything.
    pub fn should_run_compaction_stage(
        &self,
        stage: crate::agent::compaction_stages::CompactionStage,
    ) -> bool {
        match self.last_compaction {
            Some((last_stage, tokens_at_run)) if last_stage == stage => {
                self.effective_input_tokens > tokens_at_run
            }
            _ => true,
        }
    }

    /// Inject `warning` only the first time it is seen.
    ///
    /// Loop detectors re-scan the same window every iteration, so once tripped they
    /// produce the identical warning again and again until the window rolls over —
    /// which buries the signal in repetition and burns context.
    pub fn inject_warning_once(&mut self, warning: &str) {
        if self.delivered_warnings.insert(warning.to_string()) {
            self.inject_system_notification(warning);
        }
    }

    /// Record that `stage` ran at the current context size.
    pub fn record_compaction_stage(
        &mut self,
        stage: crate::agent::compaction_stages::CompactionStage,
    ) {
        self.last_compaction = Some((stage, self.effective_input_tokens));
    }

    pub fn steps(&self) -> &[StepRecord] {
        &self.steps
    }

    /// Inject a system notification as a user message (e.g., inbox messages).
    pub fn inject_system_notification(&mut self, notification: &str) {
        self.messages.push(Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: notification.to_string(),
            }],
        });
    }

    /// Compact the message history by replacing older messages with a summary.
    /// Preserves the most recent `preserve_n` messages.
    pub fn compact(&mut self, summary: &str, preserve_n: usize) {
        if self.messages.len() <= preserve_n {
            return;
        }
        let mut preserve_start = self.messages.len() - preserve_n;

        // Ensure we don't split in the middle of a tool_use/tool_result pair.
        // If the first preserved message contains ToolResult blocks, the matching
        // ToolUse assistant message was compacted away, which causes an API error.
        // Walk backwards until the first preserved message has no orphaned ToolResults.
        while preserve_start > 0 && Self::has_tool_result(&self.messages[preserve_start]) {
            preserve_start -= 1;
        }

        let preserved: Vec<Message> = self.messages.drain(preserve_start..).collect();
        self.messages.clear();
        // Insert summary as the first message
        self.messages.push(Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: format!("<summary>\n{summary}\n</summary>"),
            }],
        });
        self.messages.extend(preserved);
        // Reset effective tokens; caller will set the estimated value
        self.effective_input_tokens = 0;
    }

    /// Check if a message contains any ToolResult content blocks.
    fn has_tool_result(message: &Message) -> bool {
        message
            .content
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
    }

    pub fn into_result(self, status: AgentStatus, final_text: Option<String>) -> AgentResult {
        let total_iterations = self.iteration;
        let duration = self.elapsed_ms();
        let input_tokens = if self.total_input_tokens > 0 {
            Some(self.total_input_tokens)
        } else {
            None
        };
        let output_tokens = if self.total_output_tokens > 0 {
            Some(self.total_output_tokens)
        } else {
            None
        };
        let mut result = match status {
            AgentStatus::Success => {
                let mut r =
                    AgentResult::success(final_text.unwrap_or_default(), self.steps, duration);
                r.total_iterations = total_iterations;
                r
            }
            AgentStatus::Error => {
                let mut r =
                    AgentResult::error(final_text.unwrap_or_default(), self.steps, duration);
                r.total_iterations = total_iterations;
                r
            }
            AgentStatus::Timeout => {
                let mut r = AgentResult::timeout(self.steps, duration);
                r.total_iterations = total_iterations;
                r
            }
            AgentStatus::MaxIterations => {
                AgentResult::max_iterations(self.steps, total_iterations, duration)
            }
            AgentStatus::BudgetExceeded => AgentResult::budget_exceeded(
                self.steps,
                total_iterations,
                duration,
                self.total_cost,
            ),
        };
        result.total_input_tokens = input_tokens;
        result.total_output_tokens = output_tokens;
        // Report cost on every outcome, not just BudgetExceeded. Cost was tracked all
        // along but only ever surfaced on a status the loop never constructed, so batch
        // runs reported no spend at all.
        if self.total_cost > 0.0 {
            result.total_cost_usd = Some(self.total_cost);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::types::ToolResultContent;
    use serde_json::json;

    #[test]
    fn test_new_creates_initial_user_message() {
        let state = AgentState::new("Do something");
        assert_eq!(state.messages().len(), 1);
        assert!(matches!(state.messages()[0].role, Role::User));
        match &state.messages()[0].content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "Do something"),
            _ => panic!("Expected Text content block"),
        }
        assert_eq!(state.current_iteration(), 0);
    }

    #[test]
    fn test_add_assistant_message() {
        let mut state = AgentState::new("task");
        state.add_assistant_message(vec![ContentBlock::Text {
            text: "response".to_string(),
        }]);
        assert_eq!(state.messages().len(), 2);
        assert!(matches!(state.messages()[1].role, Role::Assistant));
        match &state.messages()[1].content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "response"),
            _ => panic!("Expected Text content block"),
        }
    }

    #[test]
    fn test_add_tool_results() {
        let mut state = AgentState::new("task");
        state.add_tool_results(vec![ContentBlock::ToolResult {
            tool_use_id: "toolu_01".to_string(),
            content: ToolResultContent::Text("result data".to_string()),
            is_error: None,
        }]);
        assert_eq!(state.messages().len(), 2);
        assert!(matches!(state.messages()[1].role, Role::User));
        match &state.messages()[1].content[0] {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_use_id, "toolu_01");
                assert_eq!(*content, ToolResultContent::Text("result data".to_string()));
                assert!(is_error.is_none());
            }
            _ => panic!("Expected ToolResult content block"),
        }
    }

    #[test]
    fn test_record_step() {
        let mut state = AgentState::new("task");
        let step = StepRecord {
            iteration: 1,
            tool: "navigate".to_string(),
            input: json!({"url": "https://example.com"}),
            output: json!({"success": true}),
            duration_ms: 500,
            is_error: None,
        };
        state.record_step(step.clone());
        let result = state.into_result(AgentStatus::Success, Some("done".to_string()));
        assert_eq!(result.steps.len(), 1);
        assert_eq!(result.steps[0].tool, "navigate");
    }

    #[test]
    fn test_increment_iteration() {
        let mut state = AgentState::new("task");
        assert_eq!(state.current_iteration(), 0);
        state.increment_iteration();
        assert_eq!(state.current_iteration(), 1);
        state.increment_iteration();
        assert_eq!(state.current_iteration(), 2);
    }

    #[test]
    fn test_elapsed_ms() {
        let state = AgentState::new("task");
        // elapsed should be >= 0 (it just started)
        assert!(state.elapsed_ms() < 1000);
    }

    #[test]
    fn test_into_result_success() {
        let mut state = AgentState::new("task");
        state.increment_iteration();
        let result = state.into_result(AgentStatus::Success, Some("completed".to_string()));
        assert_eq!(result.status, AgentStatus::Success);
        assert_eq!(result.result, Some("completed".to_string()));
        assert_eq!(result.total_iterations, 1);
        assert!(result.error.is_none());
    }

    #[test]
    fn test_into_result_error() {
        let mut state = AgentState::new("task");
        state.increment_iteration();
        let result = state.into_result(AgentStatus::Error, Some("something broke".to_string()));
        assert_eq!(result.status, AgentStatus::Error);
        assert_eq!(result.error, Some("something broke".to_string()));
        assert_eq!(result.total_iterations, 1);
        assert!(result.result.is_none());
    }

    #[test]
    fn test_into_result_timeout() {
        let mut state = AgentState::new("task");
        state.increment_iteration();
        state.increment_iteration();
        let result = state.into_result(AgentStatus::Timeout, None);
        assert_eq!(result.status, AgentStatus::Timeout);
        assert_eq!(result.total_iterations, 2);
        assert_eq!(result.error, Some("Agent timed out".to_string()));
    }

    #[test]
    fn test_into_result_max_iterations() {
        let mut state = AgentState::new("task");
        for _ in 0..5 {
            state.increment_iteration();
        }
        let result = state.into_result(AgentStatus::MaxIterations, None);
        assert_eq!(result.status, AgentStatus::MaxIterations);
        assert_eq!(result.total_iterations, 5);
        assert_eq!(result.error, Some("Max iterations (5) reached".to_string()));
    }

    #[test]
    fn test_into_result_success_default_text() {
        let state = AgentState::new("task");
        let result = state.into_result(AgentStatus::Success, None);
        assert_eq!(result.result, Some(String::new()));
    }

    #[test]
    fn test_accumulate_usage_with_some() {
        use crate::llm::types::Usage;

        let mut state = AgentState::new("task");
        let usage1 = Usage {
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        };
        let usage2 = Usage {
            input_tokens: 200,
            output_tokens: 75,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        };
        state.accumulate_usage(Some(&usage1), "claude-sonnet-4-20250514");
        state.accumulate_usage(Some(&usage2), "claude-sonnet-4-20250514");
        assert_eq!(state.total_input_tokens, 300);
        assert_eq!(state.total_output_tokens, 125);
    }

    #[test]
    fn test_accumulate_usage_with_none() {
        let mut state = AgentState::new("task");
        state.accumulate_usage(None, "claude-sonnet-4-20250514");
        assert_eq!(state.total_input_tokens, 0);
        assert_eq!(state.total_output_tokens, 0);
    }

    #[test]
    fn test_into_result_includes_token_usage() {
        use crate::llm::types::Usage;

        let mut state = AgentState::new("task");
        state.increment_iteration();
        state.accumulate_usage(
            Some(&Usage {
                input_tokens: 150,
                output_tokens: 80,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            }),
            "claude-sonnet-4-20250514",
        );
        let result = state.into_result(AgentStatus::Success, Some("done".to_string()));
        assert_eq!(result.total_input_tokens, Some(150));
        assert_eq!(result.total_output_tokens, Some(80));
    }

    #[test]
    fn test_into_result_no_token_usage_when_zero() {
        let mut state = AgentState::new("task");
        state.increment_iteration();
        let result = state.into_result(AgentStatus::Success, Some("done".to_string()));
        assert_eq!(result.total_input_tokens, None);
        assert_eq!(result.total_output_tokens, None);
    }

    #[test]
    fn test_compact_replaces_old_messages_with_summary() {
        let mut state = AgentState::new("initial task");
        // Add some messages: initial user + 4 more
        state.add_assistant_message(vec![ContentBlock::Text {
            text: "response 1".to_string(),
        }]);
        state.add_tool_results(vec![ContentBlock::ToolResult {
            tool_use_id: "t1".to_string(),
            content: ToolResultContent::Text("result 1".to_string()),
            is_error: None,
        }]);
        state.add_assistant_message(vec![ContentBlock::Text {
            text: "response 2".to_string(),
        }]);
        state.add_tool_results(vec![ContentBlock::ToolResult {
            tool_use_id: "t2".to_string(),
            content: ToolResultContent::Text("result 2".to_string()),
            is_error: None,
        }]);
        // Total: 5 messages
        assert_eq!(state.messages().len(), 5);

        state.compact("This is a summary of the conversation", 2);

        // Should have: 1 summary + 2 preserved = 3
        assert_eq!(state.messages().len(), 3);
        // First message should be summary
        match &state.messages()[0].content[0] {
            ContentBlock::Text { text } => {
                assert!(text.contains("<summary>"));
                assert!(text.contains("This is a summary"));
            }
            _ => panic!("Expected Text content block"),
        }
    }

    #[test]
    fn test_compact_noop_when_few_messages() {
        let mut state = AgentState::new("task");
        state.add_assistant_message(vec![ContentBlock::Text {
            text: "resp".to_string(),
        }]);
        // 2 messages, preserve_n=4 → no compaction
        state.compact("summary", 4);
        assert_eq!(state.messages().len(), 2);
    }

    #[test]
    fn test_total_input_tokens_accessor() {
        let mut state = AgentState::new("task");
        assert_eq!(state.total_input_tokens(), 0);
        state.accumulate_usage(
            Some(&crate::llm::types::Usage {
                input_tokens: 100,
                output_tokens: 50,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            }),
            "claude-sonnet-4-20250514",
        );
        assert_eq!(state.total_input_tokens(), 100);
        assert_eq!(state.total_output_tokens(), 50);
    }

    #[test]
    fn test_steps_accessor() {
        let mut state = AgentState::new("task");
        assert!(state.steps().is_empty());
        state.record_step(StepRecord {
            iteration: 1,
            tool: "test".to_string(),
            input: json!({}),
            output: json!({}),
            duration_ms: 0,
            is_error: None,
        });
        assert_eq!(state.steps().len(), 1);
    }

    #[test]
    fn test_from_snapshot_restores_state() {
        use crate::llm::types::Role;
        use crate::session::types::{SessionId, SessionMetadata, SessionSnapshot, SessionStatus};
        use chrono::Utc;

        let now = Utc::now();
        let snapshot = SessionSnapshot {
            metadata: SessionMetadata {
                id: SessionId("test-session".to_string()),
                created_at: now,
                updated_at: now,
                task: "original task".to_string(),
                status: SessionStatus::InProgress,
                total_input_tokens: Some(500),
                total_output_tokens: Some(200),
                parent_session_id: None,
            },
            messages: vec![
                crate::llm::types::Message {
                    role: Role::User,
                    content: vec![ContentBlock::Text {
                        text: "original task".to_string(),
                    }],
                },
                crate::llm::types::Message {
                    role: Role::Assistant,
                    content: vec![ContentBlock::Text {
                        text: "response".to_string(),
                    }],
                },
            ],
            steps: vec![StepRecord {
                iteration: 1,
                tool: "navigate".to_string(),
                input: json!({"url": "https://example.com"}),
                output: json!({"success": true}),
                duration_ms: 100,
                is_error: None,
            }],
            system_prompt: Some("system prompt".to_string()),
            iteration: 3,
        };

        let state = AgentState::from_snapshot(&snapshot);
        assert_eq!(state.messages().len(), 2);
        assert_eq!(state.steps().len(), 1);
        assert_eq!(state.current_iteration(), 3);
        assert_eq!(state.total_input_tokens(), 500);
        assert_eq!(state.total_output_tokens(), 200);
    }

    #[test]
    fn test_inject_system_notification() {
        let mut state = AgentState::new("task");
        assert_eq!(state.messages().len(), 1);
        state.inject_system_notification("<inbox_messages>\nHello from worker\n</inbox_messages>");
        assert_eq!(state.messages().len(), 2);
        assert!(matches!(state.messages()[1].role, Role::User));
        match &state.messages()[1].content[0] {
            ContentBlock::Text { text } => {
                assert!(text.contains("inbox_messages"));
                assert!(text.contains("Hello from worker"));
            }
            _ => panic!("Expected Text content block"),
        }
    }

    #[test]
    fn test_from_snapshot_empty() {
        use crate::session::types::{SessionId, SessionMetadata, SessionSnapshot, SessionStatus};
        use chrono::Utc;

        let now = Utc::now();
        let snapshot = SessionSnapshot {
            metadata: SessionMetadata {
                id: SessionId("empty-session".to_string()),
                created_at: now,
                updated_at: now,
                task: "empty".to_string(),
                status: SessionStatus::InProgress,
                total_input_tokens: None,
                total_output_tokens: None,
                parent_session_id: None,
            },
            messages: vec![],
            steps: vec![],
            system_prompt: None,
            iteration: 0,
        };

        let state = AgentState::from_snapshot(&snapshot);
        assert!(state.messages().is_empty());
        assert!(state.steps().is_empty());
        assert_eq!(state.current_iteration(), 0);
        assert_eq!(state.total_input_tokens(), 0);
        assert_eq!(state.total_output_tokens(), 0);
        assert_eq!(state.effective_input_tokens(), 0);
    }

    #[test]
    fn test_effective_input_tokens_starts_at_zero() {
        let state = AgentState::new("task");
        assert_eq!(state.effective_input_tokens(), 0);
    }

    #[test]
    fn test_effective_input_tokens_tracks_latest_prompt_size() {
        use crate::llm::types::Usage;

        let mut state = AgentState::new("task");
        state.accumulate_usage(
            Some(&Usage {
                input_tokens: 100,
                output_tokens: 50,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            }),
            "claude-sonnet-4-20250514",
        );
        assert_eq!(state.effective_input_tokens(), 100);
        assert_eq!(state.total_input_tokens(), 100);

        state.accumulate_usage(
            Some(&Usage {
                input_tokens: 200,
                output_tokens: 75,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            }),
            "claude-sonnet-4-20250514",
        );
        // `effective` is the size of the latest prompt, not a running sum: the API
        // reports the whole context in every response's `input_tokens`.
        assert_eq!(state.effective_input_tokens(), 200);
        // `total` stays cumulative, for spend reporting.
        assert_eq!(state.total_input_tokens(), 300);
    }

    #[test]
    fn test_effective_input_tokens_includes_cached_prefix() {
        use crate::llm::types::Usage;

        let mut state = AgentState::new("task");
        state.accumulate_usage(
            Some(&Usage {
                input_tokens: 1_000,
                output_tokens: 50,
                cache_creation_input_tokens: Some(400),
                cache_read_input_tokens: Some(8_000),
            }),
            "claude-sonnet-4-20250514",
        );
        // Cached tokens are billed separately but still occupy the context window.
        assert_eq!(state.effective_input_tokens(), 9_400);
    }

    #[test]
    fn test_effective_input_tokens_resets_after_compact() {
        use crate::llm::types::Usage;

        let mut state = AgentState::new("task");
        state.add_assistant_message(vec![ContentBlock::Text {
            text: "response 1".to_string(),
        }]);
        state.add_tool_results(vec![ContentBlock::ToolResult {
            tool_use_id: "t1".to_string(),
            content: ToolResultContent::Text("result 1".to_string()),
            is_error: None,
        }]);
        state.add_assistant_message(vec![ContentBlock::Text {
            text: "response 2".to_string(),
        }]);
        state.add_tool_results(vec![ContentBlock::ToolResult {
            tool_use_id: "t2".to_string(),
            content: ToolResultContent::Text("result 2".to_string()),
            is_error: None,
        }]);

        // Accumulate some usage
        state.accumulate_usage(
            Some(&Usage {
                input_tokens: 190_000,
                output_tokens: 1000,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            }),
            "claude-sonnet-4-20250514",
        );
        assert_eq!(state.effective_input_tokens(), 190_000);
        assert_eq!(state.total_input_tokens(), 190_000);

        // Compact resets effective tokens to 0
        state.compact("Summary of conversation", 2);
        assert_eq!(state.effective_input_tokens(), 0);
        // total_input_tokens remains cumulative
        assert_eq!(state.total_input_tokens(), 190_000);
    }

    #[test]
    fn test_reset_effective_tokens() {
        use crate::llm::types::Usage;

        let mut state = AgentState::new("task");
        state.accumulate_usage(
            Some(&Usage {
                input_tokens: 100_000,
                output_tokens: 500,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            }),
            "claude-sonnet-4-20250514",
        );
        assert_eq!(state.effective_input_tokens(), 100_000);

        state.reset_effective_tokens(5_000);
        assert_eq!(state.effective_input_tokens(), 5_000);
        // total_input_tokens is unaffected
        assert_eq!(state.total_input_tokens(), 100_000);
    }

    #[test]
    fn test_effective_tokens_independent_from_total_after_compaction() {
        use crate::llm::types::Usage;

        let mut state = AgentState::new("task");
        state.add_assistant_message(vec![ContentBlock::Text {
            text: "response".to_string(),
        }]);
        state.add_tool_results(vec![ContentBlock::ToolResult {
            tool_use_id: "t1".to_string(),
            content: ToolResultContent::Text("result".to_string()),
            is_error: None,
        }]);
        state.add_assistant_message(vec![ContentBlock::Text {
            text: "response 2".to_string(),
        }]);

        // First accumulation
        state.accumulate_usage(
            Some(&Usage {
                input_tokens: 150_000,
                output_tokens: 500,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            }),
            "claude-sonnet-4-20250514",
        );

        // Compact and reset effective tokens
        state.compact("Summary", 1);
        state.reset_effective_tokens(2_000);

        // Accumulate more usage after compaction
        state.accumulate_usage(
            Some(&Usage {
                input_tokens: 10_000,
                output_tokens: 200,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            }),
            "claude-sonnet-4-20250514",
        );

        // effective tracks only the latest prompt, which is the post-compaction context
        assert_eq!(state.effective_input_tokens(), 10_000);
        // total is cumulative across the whole session
        assert_eq!(state.total_input_tokens(), 160_000);
    }

    #[test]
    fn test_from_snapshot_effective_tokens_starts_at_zero() {
        use crate::llm::types::Role;
        use crate::session::types::{SessionId, SessionMetadata, SessionSnapshot, SessionStatus};
        use chrono::Utc;

        let now = Utc::now();
        let snapshot = SessionSnapshot {
            metadata: SessionMetadata {
                id: SessionId("test-session".to_string()),
                created_at: now,
                updated_at: now,
                task: "task".to_string(),
                status: SessionStatus::InProgress,
                total_input_tokens: Some(50_000),
                total_output_tokens: Some(5_000),
                parent_session_id: None,
            },
            messages: vec![crate::llm::types::Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "task".to_string(),
                }],
            }],
            steps: vec![],
            system_prompt: None,
            iteration: 5,
        };

        let state = AgentState::from_snapshot(&snapshot);
        // effective_input_tokens starts fresh on resume
        assert_eq!(state.effective_input_tokens(), 0);
        // total is restored from snapshot
        assert_eq!(state.total_input_tokens(), 50_000);
    }

    #[test]
    fn test_compact_noop_does_not_reset_effective_tokens() {
        use crate::llm::types::Usage;

        let mut state = AgentState::new("task");
        state.accumulate_usage(
            Some(&Usage {
                input_tokens: 5_000,
                output_tokens: 100,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            }),
            "claude-sonnet-4-20250514",
        );
        // Only 1 message, preserve_n=4 → no compaction
        state.compact("summary", 4);
        // effective should remain unchanged since compact was a no-op
        assert_eq!(state.effective_input_tokens(), 5_000);
    }

    #[test]
    fn test_has_written_files_defaults_to_false() {
        let state = AgentState::new("task");
        assert!(!state.has_written_files());
    }

    #[test]
    fn test_has_written_files_set_on_write_file() {
        let mut state = AgentState::new("task");
        state.record_step(StepRecord {
            iteration: 1,
            tool: "write_file".to_string(),
            input: json!({"path": "out.txt"}),
            output: json!("ok"),
            duration_ms: 10,
            is_error: None,
        });
        assert!(state.has_written_files());
    }

    #[test]
    fn test_has_written_files_set_on_edit_file() {
        let mut state = AgentState::new("task");
        state.record_step(StepRecord {
            iteration: 1,
            tool: "edit_file".to_string(),
            input: json!({"path": "out.txt"}),
            output: json!("ok"),
            duration_ms: 10,
            is_error: None,
        });
        assert!(state.has_written_files());
    }

    #[test]
    fn test_has_written_files_not_set_on_other_tools() {
        let mut state = AgentState::new("task");
        state.record_step(StepRecord {
            iteration: 1,
            tool: "navigate".to_string(),
            input: json!({}),
            output: json!({}),
            duration_ms: 10,
            is_error: None,
        });
        assert!(!state.has_written_files());
    }

    #[test]
    fn test_from_snapshot_detects_written_files() {
        use crate::llm::types::Role;
        use crate::session::types::{SessionId, SessionMetadata, SessionSnapshot, SessionStatus};
        use chrono::Utc;

        let now = Utc::now();
        let snapshot = SessionSnapshot {
            metadata: SessionMetadata {
                id: SessionId("test".to_string()),
                created_at: now,
                updated_at: now,
                task: "task".to_string(),
                status: SessionStatus::InProgress,
                total_input_tokens: None,
                total_output_tokens: None,
                parent_session_id: None,
            },
            messages: vec![crate::llm::types::Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "task".to_string(),
                }],
            }],
            steps: vec![StepRecord {
                iteration: 1,
                tool: "edit_file".to_string(),
                input: json!({}),
                output: json!({}),
                duration_ms: 0,
                is_error: None,
            }],
            system_prompt: None,
            iteration: 2,
        };

        let state = AgentState::from_snapshot(&snapshot);
        assert!(state.has_written_files());
    }

    #[test]
    fn test_original_task_returns_first_message_text() {
        let state = AgentState::new("Write a compressor for data.txt");
        assert_eq!(
            state.original_task(),
            Some("Write a compressor for data.txt")
        );
    }

    #[test]
    fn test_original_task_survives_additional_messages() {
        let mut state = AgentState::new("Solve the puzzle");
        state.add_assistant_message(vec![ContentBlock::Text {
            text: "Working on it...".to_string(),
        }]);
        state.inject_system_notification("Reminder: keep going");
        assert_eq!(state.original_task(), Some("Solve the puzzle"));
    }

    #[test]
    fn test_compact_preserves_tool_use_result_pairing() {
        let mut state = AgentState::new("task");
        // msg 0: user task
        // msg 1: assistant with tool_use
        state.add_assistant_message(vec![ContentBlock::ToolUse {
            id: "toolu_01".to_string(),
            name: "bash".to_string(),
            input: json!({"command": "ls"}),
        }]);
        // msg 2: user with tool_result
        state.add_tool_results(vec![ContentBlock::ToolResult {
            tool_use_id: "toolu_01".to_string(),
            content: ToolResultContent::Text("file1.txt".to_string()),
            is_error: None,
        }]);
        // msg 3: assistant with tool_use
        state.add_assistant_message(vec![ContentBlock::ToolUse {
            id: "toolu_02".to_string(),
            name: "bash".to_string(),
            input: json!({"command": "cat file1.txt"}),
        }]);
        // msg 4: user with tool_result
        state.add_tool_results(vec![ContentBlock::ToolResult {
            tool_use_id: "toolu_02".to_string(),
            content: ToolResultContent::Text("contents".to_string()),
            is_error: None,
        }]);
        // msg 5: assistant text
        state.add_assistant_message(vec![ContentBlock::Text {
            text: "Done reading.".to_string(),
        }]);
        // msg 6: user text
        state.inject_system_notification("keep going");
        assert_eq!(state.messages().len(), 7);

        // preserve_n=4 would normally start at index 3 (assistant tool_use).
        // But index 4 is a tool_result, so we'd have the tool_use at index 3
        // which is fine. Let's test preserve_n=3 which starts at index 4 (tool_result).
        state.compact("Summary of early work", 3);

        // Should have backed up to include index 3 (the matching assistant tool_use)
        // Result: summary + messages[3..7] = 5 messages
        assert_eq!(state.messages().len(), 5);

        // First message must be the summary
        match &state.messages()[0].content[0] {
            ContentBlock::Text { text } => {
                assert!(text.contains("<summary>"));
                assert!(text.contains("Summary of early work"));
            }
            _ => panic!("Expected summary text"),
        }

        // Second message should be the assistant with tool_use (not orphaned tool_result)
        assert!(matches!(state.messages()[1].role, Role::Assistant));
        assert!(matches!(
            &state.messages()[1].content[0],
            ContentBlock::ToolUse { .. }
        ));
    }

    #[test]
    fn test_compact_no_adjustment_when_no_tool_results_at_boundary() {
        let mut state = AgentState::new("task");
        // msg 0: user task
        // msg 1: assistant text
        state.add_assistant_message(vec![ContentBlock::Text {
            text: "response 1".to_string(),
        }]);
        // msg 2: user text
        state.inject_system_notification("nudge");
        // msg 3: assistant text
        state.add_assistant_message(vec![ContentBlock::Text {
            text: "response 2".to_string(),
        }]);
        // msg 4: user text
        state.inject_system_notification("another nudge");
        assert_eq!(state.messages().len(), 5);

        // preserve_n=2 starts at index 3, which is a text assistant message — no adjustment needed
        state.compact("Summary", 2);
        // summary + 2 preserved = 3
        assert_eq!(state.messages().len(), 3);
    }
}
