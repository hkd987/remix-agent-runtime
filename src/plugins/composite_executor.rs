use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;

use crate::browser::mcp::{ToolExecutionResult, ToolExecutor};
use crate::error::AgentError;
use crate::llm::types::ToolDefinition;

/// Composite executor that routes tool calls to the appropriate backend.
/// First-registered-wins on name collisions (warns on duplicates).
pub struct CompositeToolExecutor {
    all_tools: Vec<ToolDefinition>,
    tool_routing: HashMap<String, usize>, // tool_name -> backend index
    backends: Vec<Box<dyn ToolExecutor>>,
}

impl Default for CompositeToolExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl CompositeToolExecutor {
    pub fn new() -> Self {
        Self {
            all_tools: Vec::new(),
            tool_routing: HashMap::new(),
            backends: Vec::new(),
        }
    }

    /// Add a backend. Its tools are registered, first-registered-wins on collisions.
    pub fn add_backend(&mut self, backend: Box<dyn ToolExecutor>) {
        let backend_idx = self.backends.len();
        for tool in backend.tool_definitions() {
            if let Some(&existing_idx) = self.tool_routing.get(&tool.name) {
                tracing::warn!(
                    tool_name = %tool.name,
                    existing_backend = existing_idx,
                    new_backend = backend_idx,
                    "Tool name collision; keeping first-registered backend"
                );
            } else {
                self.tool_routing.insert(tool.name.clone(), backend_idx);
                self.all_tools.push(tool.clone());
            }
        }
        self.backends.push(backend);
    }

    /// Consume and return backends for shutdown.
    pub fn into_backends(self) -> Vec<Box<dyn ToolExecutor>> {
        self.backends
    }

    /// Consume the executor and shut down all backends.
    pub fn shutdown_all(self) {
        for backend in self.backends {
            backend.shutdown();
        }
    }
}

#[async_trait]
impl ToolExecutor for CompositeToolExecutor {
    fn tool_definitions(&self) -> &[ToolDefinition] {
        &self.all_tools
    }

    async fn execute_tool(
        &self,
        name: &str,
        arguments: Value,
    ) -> Result<ToolExecutionResult, AgentError> {
        let idx = self
            .tool_routing
            .get(name)
            .ok_or_else(|| AgentError::ToolExecution(format!("Unknown tool: {name}")))?;
        self.backends[*idx].execute_tool(name, arguments).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Mock backend that records tool calls and returns a configurable response.
    struct MockBackend {
        tools: Vec<ToolDefinition>,
        prefix: String,
    }

    impl MockBackend {
        fn new(prefix: &str, tool_names: &[&str]) -> Self {
            let tools = tool_names
                .iter()
                .map(|name| ToolDefinition {
                    name: name.to_string(),
                    description: format!("{prefix} tool: {name}"),
                    input_schema: json!({"type": "object"}),
                    cache_control: None,
                    read_only: false,
                })
                .collect();
            Self {
                tools,
                prefix: prefix.to_string(),
            }
        }
    }

    #[async_trait]
    impl ToolExecutor for MockBackend {
        fn tool_definitions(&self) -> &[ToolDefinition] {
            &self.tools
        }

        async fn execute_tool(
            &self,
            name: &str,
            _arguments: Value,
        ) -> Result<ToolExecutionResult, AgentError> {
            Ok(ToolExecutionResult {
                content: format!("{}:{name}", self.prefix),
                is_error: false,
            })
        }
    }

    #[test]
    fn test_new_is_empty() {
        let executor = CompositeToolExecutor::new();
        assert!(executor.tool_definitions().is_empty());
        assert!(executor.tool_routing.is_empty());
        assert!(executor.backends.is_empty());
    }

    #[test]
    fn test_add_single_backend() {
        let mut executor = CompositeToolExecutor::new();
        executor.add_backend(Box::new(MockBackend::new(
            "browser",
            &["navigate", "click"],
        )));

        assert_eq!(executor.tool_definitions().len(), 2);
        let names: Vec<&str> = executor
            .tool_definitions()
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert!(names.contains(&"navigate"));
        assert!(names.contains(&"click"));
    }

    #[test]
    fn test_add_multiple_backends() {
        let mut executor = CompositeToolExecutor::new();
        executor.add_backend(Box::new(MockBackend::new(
            "browser",
            &["navigate", "click"],
        )));
        executor.add_backend(Box::new(MockBackend::new(
            "local",
            &["read_file", "write_file"],
        )));

        assert_eq!(executor.tool_definitions().len(), 4);
        let names: Vec<&str> = executor
            .tool_definitions()
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert!(names.contains(&"navigate"));
        assert!(names.contains(&"click"));
        assert!(names.contains(&"read_file"));
        assert!(names.contains(&"write_file"));
    }

    #[test]
    fn test_name_collision_first_wins() {
        let mut executor = CompositeToolExecutor::new();
        executor.add_backend(Box::new(MockBackend::new(
            "first",
            &["shared_tool", "unique_a"],
        )));
        executor.add_backend(Box::new(MockBackend::new(
            "second",
            &["shared_tool", "unique_b"],
        )));

        // Only 3 tools total (shared_tool registered once)
        assert_eq!(executor.tool_definitions().len(), 3);

        // shared_tool should route to backend 0 (first)
        assert_eq!(executor.tool_routing["shared_tool"], 0);
        // unique tools route to their respective backends
        assert_eq!(executor.tool_routing["unique_a"], 0);
        assert_eq!(executor.tool_routing["unique_b"], 1);
    }

    #[tokio::test]
    async fn test_execute_routes_to_correct_backend() {
        let mut executor = CompositeToolExecutor::new();
        executor.add_backend(Box::new(MockBackend::new("browser", &["navigate"])));
        executor.add_backend(Box::new(MockBackend::new("local", &["read_file"])));

        let result = executor.execute_tool("navigate", json!({})).await.unwrap();
        assert_eq!(result.content, "browser:navigate");

        let result = executor.execute_tool("read_file", json!({})).await.unwrap();
        assert_eq!(result.content, "local:read_file");
    }

    #[tokio::test]
    async fn test_execute_collision_uses_first_backend() {
        let mut executor = CompositeToolExecutor::new();
        executor.add_backend(Box::new(MockBackend::new("first", &["shared_tool"])));
        executor.add_backend(Box::new(MockBackend::new("second", &["shared_tool"])));

        let result = executor
            .execute_tool("shared_tool", json!({}))
            .await
            .unwrap();
        assert_eq!(result.content, "first:shared_tool");
    }

    #[tokio::test]
    async fn test_execute_unknown_tool_returns_error() {
        let mut executor = CompositeToolExecutor::new();
        executor.add_backend(Box::new(MockBackend::new("browser", &["navigate"])));

        let err = executor
            .execute_tool("nonexistent", json!({}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Unknown tool: nonexistent"));
    }

    #[tokio::test]
    async fn test_execute_empty_executor_returns_error() {
        let executor = CompositeToolExecutor::new();
        let err = executor
            .execute_tool("anything", json!({}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Unknown tool: anything"));
    }

    #[test]
    fn test_into_backends_returns_all() {
        let mut executor = CompositeToolExecutor::new();
        executor.add_backend(Box::new(MockBackend::new("a", &["tool_a"])));
        executor.add_backend(Box::new(MockBackend::new("b", &["tool_b"])));

        let backends = executor.into_backends();
        assert_eq!(backends.len(), 2);
    }

    #[test]
    fn test_tool_definitions_preserve_descriptions() {
        let mut executor = CompositeToolExecutor::new();
        executor.add_backend(Box::new(MockBackend::new("browser", &["navigate"])));

        let tool = &executor.tool_definitions()[0];
        assert_eq!(tool.name, "navigate");
        assert_eq!(tool.description, "browser tool: navigate");
    }

    #[test]
    fn test_add_backend_with_no_tools() {
        let mut executor = CompositeToolExecutor::new();
        executor.add_backend(Box::new(MockBackend::new("empty", &[])));

        assert!(executor.tool_definitions().is_empty());
        assert_eq!(executor.into_backends().len(), 1);
    }

    #[tokio::test]
    async fn test_routing_indices_are_correct_with_three_backends() {
        let mut executor = CompositeToolExecutor::new();
        executor.add_backend(Box::new(MockBackend::new("a", &["tool_a"])));
        executor.add_backend(Box::new(MockBackend::new("b", &["tool_b"])));
        executor.add_backend(Box::new(MockBackend::new("c", &["tool_c"])));

        assert_eq!(executor.tool_routing["tool_a"], 0);
        assert_eq!(executor.tool_routing["tool_b"], 1);
        assert_eq!(executor.tool_routing["tool_c"], 2);

        let result = executor.execute_tool("tool_c", json!({})).await.unwrap();
        assert_eq!(result.content, "c:tool_c");
    }
}
