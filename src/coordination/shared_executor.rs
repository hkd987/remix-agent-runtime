use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::browser::mcp::{ToolExecutionResult, ToolExecutor};
use crate::error::AgentError;
use crate::llm::types::ToolDefinition;

/// Wraps an `Arc<dyn ToolExecutor>` so multiple agents can share the same
/// underlying tool backends (MCP connections, local tools, etc.).
pub struct SharedToolExecutor {
    inner: Arc<dyn ToolExecutor>,
    tools: Vec<ToolDefinition>,
}

impl std::fmt::Debug for SharedToolExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedToolExecutor")
            .field(
                "tools",
                &self.tools.iter().map(|t| &t.name).collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl SharedToolExecutor {
    /// Create a new shared executor wrapping the given executor.
    /// Tool definitions are cached at creation time.
    pub fn new(inner: Arc<dyn ToolExecutor>) -> Self {
        let tools = inner.tool_definitions().to_vec();
        Self { inner, tools }
    }
}

#[async_trait]
impl ToolExecutor for SharedToolExecutor {
    fn tool_definitions(&self) -> &[ToolDefinition] {
        &self.tools
    }

    async fn execute_tool(
        &self,
        name: &str,
        arguments: Value,
    ) -> Result<ToolExecutionResult, AgentError> {
        self.inner.execute_tool(name, arguments).await
    }

    async fn check_inbox(&self) -> Option<Vec<String>> {
        self.inner.check_inbox().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct MockExecutor {
        tools: Vec<ToolDefinition>,
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

    #[test]
    fn test_shared_executor_caches_tool_definitions() {
        let mock = MockExecutor {
            tools: vec![
                ToolDefinition {
                    name: "navigate".to_string(),
                    description: "Navigate".to_string(),
                    input_schema: json!({"type": "object"}),
                    cache_control: None,
                },
                ToolDefinition {
                    name: "click".to_string(),
                    description: "Click".to_string(),
                    input_schema: json!({"type": "object"}),
                    cache_control: None,
                },
            ],
        };
        let shared = SharedToolExecutor::new(Arc::new(mock));
        assert_eq!(shared.tool_definitions().len(), 2);
        assert_eq!(shared.tool_definitions()[0].name, "navigate");
        assert_eq!(shared.tool_definitions()[1].name, "click");
    }

    #[tokio::test]
    async fn test_shared_executor_delegates_execute() {
        let mock = MockExecutor {
            tools: vec![ToolDefinition {
                name: "navigate".to_string(),
                description: "Navigate".to_string(),
                input_schema: json!({"type": "object"}),
                cache_control: None,
            }],
        };
        let shared = SharedToolExecutor::new(Arc::new(mock));
        let result = shared
            .execute_tool("navigate", json!({"url": "https://example.com"}))
            .await
            .unwrap();
        assert_eq!(result.content, "Executed navigate");
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_shared_executor_default_check_inbox() {
        let mock = MockExecutor { tools: vec![] };
        let shared = SharedToolExecutor::new(Arc::new(mock));
        let inbox = shared.check_inbox().await;
        assert!(inbox.is_none());
    }

    #[test]
    fn test_shared_executor_debug() {
        let mock = MockExecutor {
            tools: vec![ToolDefinition {
                name: "test".to_string(),
                description: "Test".to_string(),
                input_schema: json!({"type": "object"}),
                cache_control: None,
            }],
        };
        let shared = SharedToolExecutor::new(Arc::new(mock));
        let debug = format!("{:?}", shared);
        assert!(debug.contains("SharedToolExecutor"));
        assert!(debug.contains("test"));
    }

    #[test]
    fn test_shared_executor_multiple_arcs() {
        let mock = MockExecutor {
            tools: vec![ToolDefinition {
                name: "navigate".to_string(),
                description: "Navigate".to_string(),
                input_schema: json!({"type": "object"}),
                cache_control: None,
            }],
        };
        let inner: Arc<dyn ToolExecutor> = Arc::new(mock);
        let shared1 = SharedToolExecutor::new(inner.clone());
        let shared2 = SharedToolExecutor::new(inner);
        // Both should have the same tools
        assert_eq!(
            shared1.tool_definitions().len(),
            shared2.tool_definitions().len()
        );
    }
}
