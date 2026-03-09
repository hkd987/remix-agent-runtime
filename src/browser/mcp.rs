use async_trait::async_trait;
use serde_json::Value;

use crate::error::AgentError;
use crate::llm::types::ToolDefinition;

use super::convert::{convert_mcp_tools, McpToolInfo};

/// Result from executing a tool.
#[derive(Debug, Clone)]
pub struct ToolExecutionResult {
    pub content: String,
    pub is_error: bool,
}

/// Trait for executing tools - abstracted for testability.
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    fn tool_definitions(&self) -> &[ToolDefinition];
    async fn execute_tool(
        &self,
        name: &str,
        arguments: Value,
    ) -> Result<ToolExecutionResult, AgentError>;

    /// Gracefully shut down this executor and release resources.
    /// Default implementation does nothing.
    fn shutdown(self: Box<Self>) {}

    /// Check for inter-agent inbox messages. Returns None if not supported.
    async fn check_inbox(&self) -> Option<Vec<String>> {
        None
    }
}

/// MCP client that connects to remix-browser and executes browser tools.
pub struct McpBrowserClient {
    tools: Vec<ToolDefinition>,
    peer: rmcp::Peer<rmcp::RoleClient>,
    cancel_token: rmcp::service::RunningServiceCancellationToken,
    /// Holds the RunningService alive so the background MCP transport task
    /// is not cancelled when this struct is created. Dropped on shutdown.
    _service_guard: Box<dyn std::any::Any + Send + Sync>,
}

impl McpBrowserClient {
    /// Connect to an MCP server by spawning a child process from the given command.
    /// Establishes the MCP handshake, lists available tools, and caches them.
    pub async fn connect(command: tokio::process::Command) -> Result<Self, AgentError> {
        let transport = rmcp::transport::TokioChildProcess::new(command)
            .map_err(|e| AgentError::Mcp(format!("Failed to spawn MCP process: {e}")))?;

        let service = ()
            .serve(transport)
            .await
            .map_err(|e| AgentError::Mcp(format!("Failed to establish MCP connection: {e}")))?;

        let peer = service.peer().clone();
        let cancel_token = service.cancellation_token();

        let tools_result = peer
            .list_all_tools()
            .await
            .map_err(|e| AgentError::Mcp(format!("Failed to list MCP tools: {e}")))?;

        let mcp_tools: Vec<McpToolInfo> = tools_result
            .into_iter()
            .map(|t| McpToolInfo {
                name: t.name.to_string(),
                description: t.description.map(|d| d.to_string()).unwrap_or_default(),
                input_schema: Value::Object(t.input_schema.as_ref().clone()),
            })
            .collect();

        let tools = convert_mcp_tools(&mcp_tools);

        Ok(Self {
            tools,
            peer,
            cancel_token,
            _service_guard: Box::new(service),
        })
    }

    /// Gracefully shut down the MCP connection.
    pub fn shutdown(self) {
        self.cancel_token.cancel();
    }
}

#[async_trait]
impl ToolExecutor for McpBrowserClient {
    fn tool_definitions(&self) -> &[ToolDefinition] {
        &self.tools
    }

    fn shutdown(self: Box<Self>) {
        self.cancel_token.cancel();
    }

    async fn execute_tool(
        &self,
        name: &str,
        arguments: Value,
    ) -> Result<ToolExecutionResult, AgentError> {
        let args_map = match arguments {
            Value::Object(map) => Some(map),
            Value::Null => None,
            _ => {
                return Err(AgentError::ToolExecution(
                    "Tool arguments must be a JSON object or null".to_string(),
                ))
            }
        };

        let result = self
            .peer
            .call_tool(rmcp::model::CallToolRequestParams {
                meta: None,
                name: name.to_string().into(),
                arguments: args_map,
                task: None,
            })
            .await
            .map_err(|e| {
                AgentError::ToolExecution(format!("MCP tool call '{name}' failed: {e}"))
            })?;

        let content_text = result
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
            .collect::<Vec<_>>()
            .join("\n");

        Ok(ToolExecutionResult {
            content: content_text,
            is_error: result.is_error.unwrap_or(false),
        })
    }
}

/// Blanket implementation so `Box<dyn ToolExecutor>` can be used as a generic
/// `T: ToolExecutor` parameter (e.g. in `CoordinationExecutor<Box<dyn ToolExecutor>>`).
#[async_trait]
impl ToolExecutor for Box<dyn ToolExecutor> {
    fn tool_definitions(&self) -> &[ToolDefinition] {
        (**self).tool_definitions()
    }

    async fn execute_tool(
        &self,
        name: &str,
        arguments: Value,
    ) -> Result<ToolExecutionResult, AgentError> {
        (**self).execute_tool(name, arguments).await
    }

    fn shutdown(self: Box<Self>) {
        // Cannot call `shutdown(self: Box<Self>)` on the inner trait object
        // because of double-boxing ownership rules. Child process cleanup
        // happens via Drop on the underlying transport.
    }

    async fn check_inbox(&self) -> Option<Vec<String>> {
        (**self).check_inbox().await
    }
}

// Import ServiceExt for the .serve() method
use rmcp::ServiceExt;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_tool_execution_result_construction() {
        let result = ToolExecutionResult {
            content: "Page loaded successfully".to_string(),
            is_error: false,
        };
        assert_eq!(result.content, "Page loaded successfully");
        assert!(!result.is_error);
    }

    #[test]
    fn test_tool_execution_result_error() {
        let result = ToolExecutionResult {
            content: "Element not found".to_string(),
            is_error: true,
        };
        assert_eq!(result.content, "Element not found");
        assert!(result.is_error);
    }

    #[test]
    fn test_tool_executor_is_object_safe() {
        // Verify the ToolExecutor trait is object-safe by constructing a trait object type.
        fn _assert_object_safe(_: &dyn ToolExecutor) {}
    }

    #[test]
    fn test_tool_execution_result_clone() {
        let result = ToolExecutionResult {
            content: "test".to_string(),
            is_error: false,
        };
        let cloned = result.clone();
        assert_eq!(cloned.content, result.content);
        assert_eq!(cloned.is_error, result.is_error);
    }

    #[test]
    fn test_tool_execution_result_debug() {
        let result = ToolExecutionResult {
            content: "debug test".to_string(),
            is_error: true,
        };
        let debug = format!("{:?}", result);
        assert!(debug.contains("debug test"));
        assert!(debug.contains("true"));
    }

    /// Mock implementation of ToolExecutor for testing consumers of the trait.
    struct MockToolExecutor {
        tools: Vec<ToolDefinition>,
        response: ToolExecutionResult,
    }

    #[async_trait]
    impl ToolExecutor for MockToolExecutor {
        fn tool_definitions(&self) -> &[ToolDefinition] {
            &self.tools
        }

        async fn execute_tool(
            &self,
            _name: &str,
            _arguments: Value,
        ) -> Result<ToolExecutionResult, AgentError> {
            Ok(self.response.clone())
        }
    }

    #[tokio::test]
    async fn test_mock_tool_executor() {
        let executor = MockToolExecutor {
            tools: vec![ToolDefinition {
                name: "navigate".to_string(),
                description: "Navigate to URL".to_string(),
                input_schema: json!({"type": "object", "properties": {"url": {"type": "string"}}}),
                cache_control: None,
            }],
            response: ToolExecutionResult {
                content: "Navigated to https://example.com".to_string(),
                is_error: false,
            },
        };

        assert_eq!(executor.tool_definitions().len(), 1);
        assert_eq!(executor.tool_definitions()[0].name, "navigate");

        let result = executor
            .execute_tool("navigate", json!({"url": "https://example.com"}))
            .await
            .unwrap();
        assert_eq!(result.content, "Navigated to https://example.com");
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_mock_tool_executor_as_trait_object() {
        let executor: Box<dyn ToolExecutor> = Box::new(MockToolExecutor {
            tools: vec![],
            response: ToolExecutionResult {
                content: "ok".to_string(),
                is_error: false,
            },
        });

        assert!(executor.tool_definitions().is_empty());
        let result = executor.execute_tool("any", json!({})).await.unwrap();
        assert_eq!(result.content, "ok");
    }

    // Integration tests that require a real remix-browser binary
    #[tokio::test]
    #[ignore]
    async fn test_connect_to_remix_browser() {
        let config = crate::config::schema::BrowserConfig::default();
        let command = super::super::manager::BrowserManager::build_command(&config);
        let client = McpBrowserClient::connect(command).await;
        assert!(client.is_ok(), "Should connect to remix-browser");
        let client = client.unwrap();
        assert!(
            !client.tool_definitions().is_empty(),
            "Should have tools available"
        );
        client.shutdown();
    }

    #[tokio::test]
    #[ignore]
    async fn test_execute_navigate_tool() {
        let config = crate::config::schema::BrowserConfig::default();
        let command = super::super::manager::BrowserManager::build_command(&config);
        let client = McpBrowserClient::connect(command).await.unwrap();

        let result = client
            .execute_tool("navigate", json!({"url": "https://example.com"}))
            .await;
        assert!(result.is_ok());
        client.shutdown();
    }
}
