use serde_json::Value;
use std::time::Instant;
use tracing::{debug, info, warn};

use crate::browser::mcp::ToolExecutor;
use crate::config::credentials::{inject_credentials_into_system_prompt, Credential};
use crate::config::schema::AgentConfig;
use crate::error::AgentError;
use crate::llm::client::LlmProvider;
use crate::llm::types::{ContentBlock, StopReason};
use crate::output::result::{AgentResult, AgentStatus, StepRecord};

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

    pub async fn run(
        &self,
        task: &str,
        credentials: &[Credential],
    ) -> Result<AgentResult, AgentError> {
        let mut state = AgentState::new(task);

        let mut system_parts = Vec::new();
        if let Some(ref prompt) = self.config.system_prompt {
            system_parts.push(prompt.clone());
        }
        if let Some(cred_prompt) = inject_credentials_into_system_prompt(credentials) {
            system_parts.push(cred_prompt);
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
                return Ok(state.into_result(AgentStatus::MaxIterations, None));
            }

            if state.elapsed_ms() >= timeout_ms {
                info!(elapsed_ms = state.elapsed_ms(), "Timeout reached");
                return Ok(state.into_result(AgentStatus::Timeout, None));
            }

            state.increment_iteration();
            debug!(iteration = state.current_iteration(), "Starting iteration");

            let response = self
                .llm
                .send_messages(system_prompt.as_deref(), state.messages(), Some(tool_defs))
                .await?;

            match response.stop_reason {
                StopReason::ToolUse => {
                    let assistant_content = response.content.clone();
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

                    return Ok(state.into_result(AgentStatus::Success, Some(final_text)));
                }
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
        let result = runner.run("Navigate to example.com", &[]).await.unwrap();

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
        let result = runner.run("Just say hello", &[]).await.unwrap();

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
        let result = runner.run("Loop forever", &[]).await.unwrap();

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
        let result = runner.run("Navigate to bad.com", &[]).await.unwrap();

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
        let result = runner.run("Open two pages", &[]).await.unwrap();

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

        let credentials = vec![Credential {
            name: "test_cred".to_string(),
            credential_type: crate::config::credentials::CredentialType::UsernamePassword,
            fields: [("username".to_string(), "admin".to_string())]
                .into_iter()
                .collect(),
            url_pattern: Some("*.example.com".to_string()),
            metadata: Default::default(),
            username: None,
            password: None,
        }];

        let runner = AgentRunner::new(llm, tools, config);
        let result = runner
            .run("Login to example.com", &credentials)
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
        let result = runner.run("Fail", &[]).await;

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
        let result = runner.run("Navigate", &[]).await.unwrap();

        // JSON content should be parsed into a Value::Object, not a string
        assert_eq!(
            result.steps[0].output,
            json!({"title":"Example","url":"https://example.com"})
        );
    }
}
