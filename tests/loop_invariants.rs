//! Conversation-invariant tests for the agent loop.
//!
//! The unit suite tests each harness feature in isolation. These tests instead assert a
//! property of the *whole* loop: whatever mutation a feature performs — nudge, goal
//! check, self-critique, reminder injection, or any compaction stage — the message list
//! handed to the LLM must always satisfy the Anthropic Messages API's structural rules.
//!
//! A `ValidatingLlm` sits in the provider slot and runs `validate_conversation` on every
//! request, recording any violation. A violation here corresponds to an HTTP 400 in
//! production.

use async_trait::async_trait;
use remix_agent_runtime::agent::{validate_conversation, AgentRunner};
use remix_agent_runtime::browser::mcp::{ToolExecutionResult, ToolExecutor};
use remix_agent_runtime::config::schema::{
    AgentConfig, CompactionConfig, SelfCritiqueConfig, StageThresholds,
};
use remix_agent_runtime::error::AgentError;
use remix_agent_runtime::llm::types::{
    ContentBlock, Message, MessagesResponse, StopReason, SystemContent, ToolDefinition, Usage,
};
use remix_agent_runtime::llm::LlmProvider;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

/// An LLM mock that validates the conversation on every request.
struct ValidatingLlm {
    responses: Arc<Mutex<Vec<MessagesResponse>>>,
    violations: Arc<Mutex<Vec<String>>>,
    /// Response returned once the scripted list is exhausted, so a loop that keeps
    /// going does not mask a violation behind an unrelated "no more responses" error.
    fallback: MessagesResponse,
}

type Violations = Arc<Mutex<Vec<String>>>;

impl ValidatingLlm {
    /// Returns the mock plus a handle to the violations it records, so the caller can
    /// read them after the runner has consumed the mock.
    fn new(responses: Vec<MessagesResponse>) -> (Self, Violations) {
        let violations: Violations = Arc::new(Mutex::new(Vec::new()));
        let llm = Self {
            responses: Arc::new(Mutex::new(responses)),
            violations: Arc::clone(&violations),
            fallback: end_turn("done"),
        };
        (llm, violations)
    }
}

#[async_trait]
impl LlmProvider for ValidatingLlm {
    async fn send_messages(
        &self,
        _system: Option<&[SystemContent]>,
        messages: &[Message],
        _tools: Option<&[ToolDefinition]>,
    ) -> Result<MessagesResponse, AgentError> {
        if let Err(e) = validate_conversation(messages) {
            self.violations.lock().unwrap().push(e);
        }
        let mut responses = self.responses.lock().unwrap();
        if responses.is_empty() {
            Ok(self.fallback.clone())
        } else {
            Ok(responses.remove(0))
        }
    }
}

struct EchoTools {
    tools: Vec<ToolDefinition>,
    /// Bytes of filler in each tool result, to drive up context size for compaction.
    payload_bytes: usize,
}

impl EchoTools {
    fn new() -> Self {
        Self {
            tools: vec![ToolDefinition {
                name: "bash".to_string(),
                description: "Run a shell command".to_string(),
                input_schema: json!({"type": "object", "properties": {"command": {"type": "string"}}}),
                cache_control: None,
                read_only: false,
            }],
            payload_bytes: 0,
        }
    }

    fn with_payload(mut self, bytes: usize) -> Self {
        self.payload_bytes = bytes;
        self
    }
}

#[async_trait]
impl ToolExecutor for EchoTools {
    fn tool_definitions(&self) -> &[ToolDefinition] {
        &self.tools
    }

    async fn execute_tool(
        &self,
        _name: &str,
        _arguments: Value,
    ) -> Result<ToolExecutionResult, AgentError> {
        Ok(ToolExecutionResult {
            content: format!("ok{}", "x".repeat(self.payload_bytes)),
            is_error: false,
        })
    }
}

fn end_turn(text: &str) -> MessagesResponse {
    MessagesResponse {
        id: "msg_test".to_string(),
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        model: "test-model".to_string(),
        stop_reason: StopReason::EndTurn,
        usage: Some(Usage {
            input_tokens: 1000,
            output_tokens: 100,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        }),
    }
}

fn tool_use(id: &str) -> MessagesResponse {
    MessagesResponse {
        id: "msg_test".to_string(),
        content: vec![ContentBlock::ToolUse {
            id: id.to_string(),
            name: "bash".to_string(),
            input: json!({"command": "echo hi"}),
        }],
        model: "test-model".to_string(),
        stop_reason: StopReason::ToolUse,
        usage: Some(Usage {
            input_tokens: 1000,
            output_tokens: 100,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        }),
    }
}

/// An assistant turn carrying both prose and a tool call, with `stop_reason=end_turn`.
/// The API can legitimately return this shape.
fn end_turn_with_tool_use(id: &str) -> MessagesResponse {
    MessagesResponse {
        id: "msg_test".to_string(),
        content: vec![
            ContentBlock::Text {
                text: "I'll check that.".to_string(),
            },
            ContentBlock::ToolUse {
                id: id.to_string(),
                name: "bash".to_string(),
                input: json!({"command": "echo hi"}),
            },
        ],
        model: "test-model".to_string(),
        stop_reason: StopReason::EndTurn,
        usage: Some(Usage {
            input_tokens: 1000,
            output_tokens: 100,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        }),
    }
}

fn base_config() -> AgentConfig {
    AgentConfig {
        max_iterations: 12,
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

async fn run(
    llm: ValidatingLlm,
    violations: Violations,
    tools: EchoTools,
    config: AgentConfig,
) -> Vec<String> {
    let runner = AgentRunner::new(llm, tools, config);
    let _ = runner
        .run(
            "do the thing",
            &Default::default(),
            &Default::default(),
            &None,
        )
        .await;
    let v = violations.lock().unwrap().clone();
    v
}

async fn run_with_compaction(
    llm: ValidatingLlm,
    violations: Violations,
    tools: EchoTools,
    config: AgentConfig,
    compaction: CompactionConfig,
) -> Vec<String> {
    let runner = AgentRunner::new(llm, tools, config);
    let _ = runner
        .run_with_options(
            "do the thing",
            &Default::default(),
            &Default::default(),
            &None,
            None,
            Some(&compaction),
            None,
        )
        .await;
    let v = violations.lock().unwrap().clone();
    v
}

// --- baseline ---------------------------------------------------------------

#[tokio::test]
async fn plain_tool_loop_holds_invariants() {
    let (llm, violations) =
        ValidatingLlm::new(vec![tool_use("t1"), tool_use("t2"), end_turn("done")]);
    let violations = run(llm, violations, EchoTools::new(), base_config()).await;
    assert!(violations.is_empty(), "violations: {violations:#?}");
}

#[tokio::test]
async fn nudge_holds_invariants() {
    let mut config = base_config();
    config.nudge_on_text_only = true;
    let (llm, violations) = ValidatingLlm::new(vec![
        end_turn("just thinking out loud"),
        tool_use("t1"),
        end_turn("done"),
    ]);
    let violations = run(llm, violations, EchoTools::new(), config).await;
    assert!(violations.is_empty(), "violations: {violations:#?}");
}

// --- C1: self-critique rejection --------------------------------------------

/// Self-critique rejection appends the assistant message (which carries `tool_use`)
/// and then injects a plain user text message, orphaning the tool call.
#[tokio::test]
async fn self_critique_rejection_holds_invariants() {
    let mut config = base_config();
    config.self_critique = Some(SelfCritiqueConfig {
        enabled: true,
        ..Default::default()
    });

    // The critique call consumes a response of its own. `parse_critique_response`
    // looks for an `approved:` line and rejects only when it does not contain "yes",
    // so the verdict must be spelled exactly this way to exercise the reject path.
    let (llm, violations) = ValidatingLlm::new(vec![
        tool_use("t1"),
        end_turn("APPROVED: no\nREASONING: that command is destructive"),
        tool_use("t2"),
        end_turn("done"),
    ]);

    let violations = run(llm, violations, EchoTools::new(), config).await;
    assert!(
        violations.is_empty(),
        "self-critique rejection orphaned a tool_use: {violations:#?}"
    );
}

// --- C2: goal check ---------------------------------------------------------

/// The goal check fires on the `end_turn` path even when the response still carries
/// `tool_use` blocks, appending them and then injecting a user text message.
#[tokio::test]
async fn goal_check_with_pending_tool_use_holds_invariants() {
    let mut config = base_config();
    config.goal_check_on_complete = true;

    // The goal check requires > 5 iterations, so burn six tool-use turns first.
    let mut responses: Vec<MessagesResponse> =
        (1..=6).map(|i| tool_use(&format!("t{i}"))).collect();
    responses.push(end_turn_with_tool_use("t7"));
    responses.push(end_turn("done"));

    let (llm, violations) = ValidatingLlm::new(responses);
    let violations = run(llm, violations, EchoTools::new(), config).await;
    assert!(
        violations.is_empty(),
        "goal check orphaned a tool_use: {violations:#?}"
    );
}

#[tokio::test]
async fn goal_check_on_text_only_holds_invariants() {
    let mut config = base_config();
    config.goal_check_on_complete = true;

    let mut responses: Vec<MessagesResponse> =
        (1..=6).map(|i| tool_use(&format!("t{i}"))).collect();
    responses.push(end_turn("I think I'm finished"));
    responses.push(end_turn("confirmed done"));

    let (llm, violations) = ValidatingLlm::new(responses);
    let violations = run(llm, violations, EchoTools::new(), config).await;
    assert!(violations.is_empty(), "violations: {violations:#?}");
}

// --- C4 / C5: compaction stages ---------------------------------------------

/// Drive enough large tool results through the loop to trip the progressive
/// compaction stages, and assert none of them orphans a tool_result or breaks
/// role alternation.
#[tokio::test]
async fn progressive_compaction_holds_invariants() {
    let config = base_config();
    let compaction = CompactionConfig {
        enabled: true,
        trigger_threshold: 0.5,
        // A small window so the stages trip within a short test run.
        context_window_tokens: 10_000,
        preserve_recent_n: 4,
        compaction_model: None,
        compaction_max_tokens: None,
        stage_thresholds: Some(StageThresholds::default()),
    };

    let responses: Vec<MessagesResponse> = (1..=10).map(|i| tool_use(&format!("t{i}"))).collect();
    let (llm, violations) = ValidatingLlm::new(responses);

    let violations = run_with_compaction(
        llm,
        violations,
        EchoTools::new().with_payload(4000),
        config,
        compaction,
    )
    .await;
    assert!(
        violations.is_empty(),
        "compaction broke conversation invariants: {violations:#?}"
    );
}

#[tokio::test]
async fn legacy_compaction_holds_invariants() {
    let config = base_config();
    let compaction = CompactionConfig {
        enabled: true,
        trigger_threshold: 0.5,
        context_window_tokens: 10_000,
        preserve_recent_n: 4,
        compaction_model: None,
        compaction_max_tokens: None,
        stage_thresholds: None,
    };

    let responses: Vec<MessagesResponse> = (1..=10).map(|i| tool_use(&format!("t{i}"))).collect();
    let (llm, violations) = ValidatingLlm::new(responses);

    let violations = run_with_compaction(
        llm,
        violations,
        EchoTools::new().with_payload(4000),
        config,
        compaction,
    )
    .await;
    assert!(
        violations.is_empty(),
        "legacy compaction broke conversation invariants: {violations:#?}"
    );
}

// --- reminders / loop detection ---------------------------------------------

#[tokio::test]
async fn reminder_injection_holds_invariants() {
    let mut config = base_config();
    config.action_reminder_interval = Some(2);

    let responses: Vec<MessagesResponse> = (1..=8).map(|i| tool_use(&format!("t{i}"))).collect();
    let (llm, violations) = ValidatingLlm::new(responses);
    let violations = run(llm, violations, EchoTools::new(), config).await;
    assert!(violations.is_empty(), "violations: {violations:#?}");
}
