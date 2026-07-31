use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::{info, warn};

use crate::browser::mcp::{ToolExecutionResult, ToolExecutor};
use crate::config::schema::SubagentConfig;
use crate::error::AgentError;
use crate::llm::types::ToolDefinition;

use super::types::SubagentDefinition;

/// Decorator that adds a `spawn_agent` virtual tool.
///
/// NOTE: The actual agent spawning requires an LlmProvider which cannot be
/// provided at the executor level. This executor validates the input and
/// returns a structured SubagentDefinition as JSON. The actual spawning
/// is handled by the AgentRunner when it processes spawn_agent results.
pub struct SubagentExecutor<T: ToolExecutor> {
    inner: T,
    config: SubagentConfig,
    all_tools: Vec<ToolDefinition>,
}

/// The `spawn_agent` tool definition.
///
/// Single source of truth: `CoordinationExecutor` and `SubagentExecutor` both advertise
/// this tool and previously carried byte-identical copies of the schema, so a change to
/// one silently diverged from the other.
pub fn spawn_agent_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "spawn_agent".to_string(),
        description: "Spawn a child agent to handle a subtask. The child agent runs independently with its own conversation and can use a subset of available tools.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name for the subagent"
                },
                "task": {
                    "type": "string",
                    "description": "The task for the subagent to accomplish"
                },
                "system_prompt": {
                    "type": "string",
                    "description": "Optional system prompt for the subagent"
                },
                "allowed_tools": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Tool name patterns this subagent can use (regex). If empty, uses all parent tools except spawn_agent."
                },
                "max_iterations": {
                    "type": "integer",
                    "description": "Maximum iterations for the subagent (default from config)"
                }
            },
            "required": ["name", "task"]
        }),
        cache_control: None,
        read_only: false,
    }
}

impl<T: ToolExecutor> SubagentExecutor<T> {
    pub fn new(inner: T, config: SubagentConfig) -> Self {
        let mut all_tools = Vec::new();

        if config.enabled {
            all_tools.push(spawn_agent_tool_definition());
        }

        // Add inner tools after virtual tools
        all_tools.extend_from_slice(inner.tool_definitions());

        Self {
            inner,
            config,
            all_tools,
        }
    }

    pub fn into_inner(self) -> T {
        self.inner
    }

    pub fn config(&self) -> &SubagentConfig {
        &self.config
    }

    fn parse_spawn_request(&self, arguments: &Value) -> Result<SubagentDefinition, String> {
        let name = arguments
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or("Missing required field 'name'")?
            .to_string();

        let task = arguments
            .get("task")
            .and_then(|v| v.as_str())
            .ok_or("Missing required field 'task'")?
            .to_string();

        let system_prompt = arguments
            .get("system_prompt")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let allowed_tools: Vec<String> = arguments
            .get("allowed_tools")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let max_iterations = arguments
            .get("max_iterations")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            .unwrap_or(self.config.max_iterations);

        Ok(SubagentDefinition {
            name,
            description: task,
            system_prompt,
            allowed_tools,
            max_iterations,
            timeout_secs: self.config.timeout_secs,
        })
    }
}

#[async_trait]
impl<T: ToolExecutor> ToolExecutor for SubagentExecutor<T> {
    fn tool_definitions(&self) -> &[ToolDefinition] {
        &self.all_tools
    }

    async fn execute_tool(
        &self,
        name: &str,
        arguments: Value,
    ) -> Result<ToolExecutionResult, AgentError> {
        if name == "spawn_agent" {
            if !self.config.enabled {
                return Ok(ToolExecutionResult {
                    content: "Subagent spawning is disabled".to_string(),
                    is_error: true,
                });
            }

            match self.parse_spawn_request(&arguments) {
                Ok(definition) => {
                    info!(
                        subagent = %definition.name,
                        task = %definition.description,
                        "Subagent spawn requested"
                    );
                    // Return the definition as JSON for the AgentRunner to handle
                    let json = serde_json::to_string_pretty(&definition)
                        .unwrap_or_else(|e| format!("Failed to serialize: {e}"));
                    Ok(ToolExecutionResult {
                        content: json,
                        is_error: false,
                    })
                }
                Err(e) => {
                    warn!(error = %e, "Invalid spawn_agent request");
                    Ok(ToolExecutionResult {
                        content: format!("Invalid spawn_agent request: {e}"),
                        is_error: true,
                    })
                }
            }
        } else {
            self.inner.execute_tool(name, arguments).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Mock ToolExecutor for testing.
    struct MockExecutor {
        tools: Vec<ToolDefinition>,
    }

    impl MockExecutor {
        fn new(tool_names: &[&str]) -> Self {
            Self {
                tools: tool_names
                    .iter()
                    .map(|name| ToolDefinition {
                        name: name.to_string(),
                        description: format!("Tool {name}"),
                        input_schema: json!({"type": "object"}),
                        cache_control: None,
                        read_only: false,
                    })
                    .collect(),
            }
        }
    }

    #[async_trait]
    impl ToolExecutor for MockExecutor {
        fn tool_definitions(&self) -> &[ToolDefinition] {
            &self.tools
        }

        async fn execute_tool(
            &self,
            name: &str,
            _arguments: Value,
        ) -> Result<ToolExecutionResult, AgentError> {
            Ok(ToolExecutionResult {
                content: format!("Executed {name}"),
                is_error: false,
            })
        }
    }

    fn enabled_config() -> SubagentConfig {
        SubagentConfig {
            enabled: true,
            max_iterations: 10,
            timeout_secs: 120,
        }
    }

    fn disabled_config() -> SubagentConfig {
        SubagentConfig {
            enabled: false,
            max_iterations: 10,
            timeout_secs: 120,
        }
    }

    #[test]
    fn test_tool_definitions_includes_spawn_agent_when_enabled() {
        let executor = MockExecutor::new(&["navigate", "click"]);
        let subagent_exec = SubagentExecutor::new(executor, enabled_config());

        let tools = subagent_exec.tool_definitions();
        assert_eq!(tools.len(), 3); // spawn_agent + navigate + click
        assert_eq!(tools[0].name, "spawn_agent");
        assert_eq!(tools[1].name, "navigate");
        assert_eq!(tools[2].name, "click");
    }

    #[test]
    fn test_tool_definitions_excludes_spawn_agent_when_disabled() {
        let executor = MockExecutor::new(&["navigate", "click"]);
        let subagent_exec = SubagentExecutor::new(executor, disabled_config());

        let tools = subagent_exec.tool_definitions();
        assert_eq!(tools.len(), 2); // only navigate + click
        assert_eq!(tools[0].name, "navigate");
        assert_eq!(tools[1].name, "click");
    }

    #[test]
    fn test_spawn_agent_tool_definition_schema() {
        let executor = MockExecutor::new(&[]);
        let subagent_exec = SubagentExecutor::new(executor, enabled_config());

        let spawn_tool = &subagent_exec.tool_definitions()[0];
        assert_eq!(spawn_tool.name, "spawn_agent");
        assert!(spawn_tool.description.contains("child agent"));

        let schema = &spawn_tool.input_schema;
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["name"].is_object());
        assert!(schema["properties"]["task"].is_object());
        assert!(schema["properties"]["system_prompt"].is_object());
        assert!(schema["properties"]["allowed_tools"].is_object());
        assert!(schema["properties"]["max_iterations"].is_object());

        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("name")));
        assert!(required.contains(&json!("task")));
    }

    #[tokio::test]
    async fn test_execute_delegates_non_spawn_tools() {
        let executor = MockExecutor::new(&["navigate"]);
        let subagent_exec = SubagentExecutor::new(executor, enabled_config());

        let result = subagent_exec
            .execute_tool("navigate", json!({"url": "https://example.com"}))
            .await
            .unwrap();
        assert_eq!(result.content, "Executed navigate");
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_execute_spawn_agent_valid_request() {
        let executor = MockExecutor::new(&["navigate"]);
        let subagent_exec = SubagentExecutor::new(executor, enabled_config());

        let result = subagent_exec
            .execute_tool(
                "spawn_agent",
                json!({
                    "name": "researcher",
                    "task": "Find the answer",
                    "system_prompt": "You are helpful",
                    "allowed_tools": ["navigate"],
                    "max_iterations": 5
                }),
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        let def: SubagentDefinition = serde_json::from_str(&result.content).unwrap();
        assert_eq!(def.name, "researcher");
        assert_eq!(def.description, "Find the answer");
        assert_eq!(def.system_prompt, Some("You are helpful".to_string()));
        assert_eq!(def.allowed_tools, vec!["navigate"]);
        assert_eq!(def.max_iterations, 5);
        assert_eq!(def.timeout_secs, 120);
    }

    #[tokio::test]
    async fn test_execute_spawn_agent_minimal_request() {
        let executor = MockExecutor::new(&[]);
        let subagent_exec = SubagentExecutor::new(executor, enabled_config());

        let result = subagent_exec
            .execute_tool(
                "spawn_agent",
                json!({
                    "name": "worker",
                    "task": "Do something"
                }),
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        let def: SubagentDefinition = serde_json::from_str(&result.content).unwrap();
        assert_eq!(def.name, "worker");
        assert_eq!(def.description, "Do something");
        assert!(def.system_prompt.is_none());
        assert!(def.allowed_tools.is_empty());
        assert_eq!(def.max_iterations, 10); // default from config
        assert_eq!(def.timeout_secs, 120);
    }

    #[tokio::test]
    async fn test_execute_spawn_agent_missing_name() {
        let executor = MockExecutor::new(&[]);
        let subagent_exec = SubagentExecutor::new(executor, enabled_config());

        let result = subagent_exec
            .execute_tool(
                "spawn_agent",
                json!({
                    "task": "Do something"
                }),
            )
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(result.content.contains("Missing required field 'name'"));
    }

    #[tokio::test]
    async fn test_execute_spawn_agent_missing_task() {
        let executor = MockExecutor::new(&[]);
        let subagent_exec = SubagentExecutor::new(executor, enabled_config());

        let result = subagent_exec
            .execute_tool(
                "spawn_agent",
                json!({
                    "name": "worker"
                }),
            )
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(result.content.contains("Missing required field 'task'"));
    }

    #[tokio::test]
    async fn test_execute_spawn_agent_disabled() {
        let executor = MockExecutor::new(&[]);
        let subagent_exec = SubagentExecutor::new(executor, disabled_config());

        let result = subagent_exec
            .execute_tool(
                "spawn_agent",
                json!({
                    "name": "worker",
                    "task": "Do something"
                }),
            )
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(result.content.contains("disabled"));
    }

    #[test]
    fn test_parse_spawn_request_all_fields() {
        let executor = MockExecutor::new(&[]);
        let subagent_exec = SubagentExecutor::new(executor, enabled_config());

        let args = json!({
            "name": "coder",
            "task": "Write a function",
            "system_prompt": "You are a coder",
            "allowed_tools": ["bash", "write_file"],
            "max_iterations": 20
        });

        let def = subagent_exec.parse_spawn_request(&args).unwrap();
        assert_eq!(def.name, "coder");
        assert_eq!(def.description, "Write a function");
        assert_eq!(def.system_prompt, Some("You are a coder".to_string()));
        assert_eq!(def.allowed_tools, vec!["bash", "write_file"]);
        assert_eq!(def.max_iterations, 20);
        assert_eq!(def.timeout_secs, 120);
    }

    #[test]
    fn test_parse_spawn_request_minimal_fields() {
        let executor = MockExecutor::new(&[]);
        let subagent_exec = SubagentExecutor::new(executor, enabled_config());

        let args = json!({
            "name": "worker",
            "task": "Do work"
        });

        let def = subagent_exec.parse_spawn_request(&args).unwrap();
        assert_eq!(def.name, "worker");
        assert_eq!(def.description, "Do work");
        assert!(def.system_prompt.is_none());
        assert!(def.allowed_tools.is_empty());
        assert_eq!(def.max_iterations, 10);
        assert_eq!(def.timeout_secs, 120);
    }

    #[test]
    fn test_parse_spawn_request_name_not_string() {
        let executor = MockExecutor::new(&[]);
        let subagent_exec = SubagentExecutor::new(executor, enabled_config());

        let args = json!({
            "name": 42,
            "task": "Do work"
        });

        let err = subagent_exec.parse_spawn_request(&args).unwrap_err();
        assert!(err.contains("name"));
    }

    #[test]
    fn test_parse_spawn_request_task_not_string() {
        let executor = MockExecutor::new(&[]);
        let subagent_exec = SubagentExecutor::new(executor, enabled_config());

        let args = json!({
            "name": "worker",
            "task": 123
        });

        let err = subagent_exec.parse_spawn_request(&args).unwrap_err();
        assert!(err.contains("task"));
    }

    #[test]
    fn test_into_inner_recovers_executor() {
        let executor = MockExecutor::new(&["navigate"]);
        let subagent_exec = SubagentExecutor::new(executor, enabled_config());

        let inner = subagent_exec.into_inner();
        assert_eq!(inner.tool_definitions().len(), 1);
        assert_eq!(inner.tool_definitions()[0].name, "navigate");
    }

    #[test]
    fn test_config_accessor() {
        let executor = MockExecutor::new(&[]);
        let subagent_exec = SubagentExecutor::new(executor, enabled_config());

        let config = subagent_exec.config();
        assert!(config.enabled);
        assert_eq!(config.max_iterations, 10);
        assert_eq!(config.timeout_secs, 120);
    }

    #[test]
    fn test_config_accessor_disabled() {
        let executor = MockExecutor::new(&[]);
        let subagent_exec = SubagentExecutor::new(executor, disabled_config());

        assert!(!subagent_exec.config().enabled);
    }

    #[tokio::test]
    async fn test_execute_spawn_agent_empty_object() {
        let executor = MockExecutor::new(&[]);
        let subagent_exec = SubagentExecutor::new(executor, enabled_config());

        let result = subagent_exec
            .execute_tool("spawn_agent", json!({}))
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(result.content.contains("Missing required field"));
    }

    #[test]
    fn test_tool_definitions_order_spawn_first() {
        let executor = MockExecutor::new(&["alpha", "beta"]);
        let subagent_exec = SubagentExecutor::new(executor, enabled_config());

        let names: Vec<&str> = subagent_exec
            .tool_definitions()
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(names, vec!["spawn_agent", "alpha", "beta"]);
    }

    #[test]
    fn test_no_inner_tools_only_spawn() {
        let executor = MockExecutor::new(&[]);
        let subagent_exec = SubagentExecutor::new(executor, enabled_config());

        let tools = subagent_exec.tool_definitions();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "spawn_agent");
    }

    #[test]
    fn test_no_inner_tools_disabled_means_empty() {
        let executor = MockExecutor::new(&[]);
        let subagent_exec = SubagentExecutor::new(executor, disabled_config());

        assert!(subagent_exec.tool_definitions().is_empty());
    }
}
