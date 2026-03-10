use async_trait::async_trait;
use regex::Regex;
use serde_json::Value;

use crate::browser::mcp::{ToolExecutionResult, ToolExecutor};
use crate::error::AgentError;
use crate::llm::types::ToolDefinition;

/// A ToolExecutor wrapper that filters tools based on allowed patterns.
pub struct FilteredToolExecutor<T: ToolExecutor> {
    inner: T,
    allowed_patterns: Vec<Regex>,
    filtered_tools: Vec<ToolDefinition>,
}

impl<T: ToolExecutor> std::fmt::Debug for FilteredToolExecutor<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FilteredToolExecutor")
            .field("allowed_patterns", &self.allowed_patterns)
            .field(
                "filtered_tools",
                &self
                    .filtered_tools
                    .iter()
                    .map(|t| &t.name)
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl<T: ToolExecutor> FilteredToolExecutor<T> {
    pub fn new(inner: T, allowed_patterns: Vec<String>) -> Result<Self, AgentError> {
        let patterns: Vec<Regex> = allowed_patterns
            .iter()
            .map(|p| {
                Regex::new(&format!("^({p})$"))
                    .map_err(|e| AgentError::Subagent(format!("Invalid tool pattern '{p}': {e}")))
            })
            .collect::<Result<Vec<_>, _>>()?;

        let filtered_tools: Vec<ToolDefinition> = inner
            .tool_definitions()
            .iter()
            .filter(|t| patterns.iter().any(|re| re.is_match(&t.name)))
            .cloned()
            .collect();

        Ok(Self {
            inner,
            allowed_patterns: patterns,
            filtered_tools,
        })
    }

    pub fn into_inner(self) -> T {
        self.inner
    }

    fn is_tool_allowed(&self, name: &str) -> bool {
        self.allowed_patterns.iter().any(|re| re.is_match(name))
    }
}

#[async_trait]
impl<T: ToolExecutor> ToolExecutor for FilteredToolExecutor<T> {
    fn tool_definitions(&self) -> &[ToolDefinition] {
        &self.filtered_tools
    }

    async fn execute_tool(
        &self,
        name: &str,
        arguments: Value,
    ) -> Result<ToolExecutionResult, AgentError> {
        if !self.is_tool_allowed(name) {
            return Ok(ToolExecutionResult {
                content: format!("Tool '{name}' is not allowed for this subagent"),
                is_error: true,
            });
        }
        self.inner.execute_tool(name, arguments).await
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

    #[test]
    fn test_filters_exact_tool_names() {
        let executor = MockExecutor::new(&["navigate", "click", "screenshot", "bash"]);
        let filtered =
            FilteredToolExecutor::new(executor, vec!["navigate".to_string(), "click".to_string()])
                .unwrap();

        let tools = filtered.tool_definitions();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "navigate");
        assert_eq!(tools[1].name, "click");
    }

    #[test]
    fn test_filters_with_regex_wildcard() {
        let executor = MockExecutor::new(&["read_file", "read_page", "write_file", "bash"]);
        let filtered = FilteredToolExecutor::new(executor, vec!["read_.*".to_string()]).unwrap();

        let tools = filtered.tool_definitions();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "read_file");
        assert_eq!(tools[1].name, "read_page");
    }

    #[test]
    fn test_filters_with_pipe_separated_regex() {
        let executor = MockExecutor::new(&["navigate", "click", "screenshot", "bash"]);
        let filtered =
            FilteredToolExecutor::new(executor, vec!["navigate|click".to_string()]).unwrap();

        let tools = filtered.tool_definitions();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "navigate");
        assert_eq!(tools[1].name, "click");
    }

    #[test]
    fn test_empty_patterns_means_no_tools() {
        let executor = MockExecutor::new(&["navigate", "click"]);
        let filtered = FilteredToolExecutor::new(executor, vec![]).unwrap();

        assert!(filtered.tool_definitions().is_empty());
    }

    #[test]
    fn test_invalid_regex_returns_error() {
        let executor = MockExecutor::new(&["navigate"]);
        let result = FilteredToolExecutor::new(executor, vec!["[invalid".to_string()]);

        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            AgentError::Subagent(msg) => {
                assert!(msg.contains("Invalid tool pattern"));
                assert!(msg.contains("[invalid"));
            }
            other => panic!("Expected AgentError::Subagent, got {other:?}"),
        }
    }

    #[test]
    fn test_no_matching_tools() {
        let executor = MockExecutor::new(&["navigate", "click"]);
        let filtered =
            FilteredToolExecutor::new(executor, vec!["nonexistent".to_string()]).unwrap();

        assert!(filtered.tool_definitions().is_empty());
    }

    #[tokio::test]
    async fn test_execute_allowed_tool() {
        let executor = MockExecutor::new(&["navigate", "click"]);
        let filtered = FilteredToolExecutor::new(executor, vec!["navigate".to_string()]).unwrap();

        let result = filtered
            .execute_tool("navigate", json!({"url": "https://example.com"}))
            .await
            .unwrap();
        assert_eq!(result.content, "Executed navigate");
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_execute_disallowed_tool_returns_error() {
        let executor = MockExecutor::new(&["navigate", "click"]);
        let filtered = FilteredToolExecutor::new(executor, vec!["navigate".to_string()]).unwrap();

        let result = filtered.execute_tool("click", json!({})).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("not allowed"));
        assert!(result.content.contains("click"));
    }

    #[tokio::test]
    async fn test_execute_completely_unknown_tool_returns_error() {
        let executor = MockExecutor::new(&["navigate"]);
        let filtered = FilteredToolExecutor::new(executor, vec!["navigate".to_string()]).unwrap();

        let result = filtered
            .execute_tool("unknown_tool", json!({}))
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("not allowed"));
    }

    #[tokio::test]
    async fn test_execute_with_regex_pattern_match() {
        let executor = MockExecutor::new(&["read_file", "write_file"]);
        let filtered = FilteredToolExecutor::new(executor, vec!["read_.*".to_string()]).unwrap();

        let result = filtered.execute_tool("read_file", json!({})).await.unwrap();
        assert!(!result.is_error);
        assert_eq!(result.content, "Executed read_file");

        let result = filtered
            .execute_tool("write_file", json!({}))
            .await
            .unwrap();
        assert!(result.is_error);
    }

    #[test]
    fn test_into_inner_recovers_executor() {
        let executor = MockExecutor::new(&["navigate"]);
        let filtered = FilteredToolExecutor::new(executor, vec!["navigate".to_string()]).unwrap();

        let inner = filtered.into_inner();
        assert_eq!(inner.tool_definitions().len(), 1);
        assert_eq!(inner.tool_definitions()[0].name, "navigate");
    }

    #[test]
    fn test_is_tool_allowed_exact() {
        let executor = MockExecutor::new(&["navigate", "click"]);
        let filtered = FilteredToolExecutor::new(executor, vec!["navigate".to_string()]).unwrap();

        assert!(filtered.is_tool_allowed("navigate"));
        assert!(!filtered.is_tool_allowed("click"));
        assert!(!filtered.is_tool_allowed("navigate_extra"));
    }

    #[test]
    fn test_is_tool_allowed_regex() {
        let executor = MockExecutor::new(&["read_file", "read_page"]);
        let filtered = FilteredToolExecutor::new(executor, vec!["read_.*".to_string()]).unwrap();

        assert!(filtered.is_tool_allowed("read_file"));
        assert!(filtered.is_tool_allowed("read_page"));
        assert!(!filtered.is_tool_allowed("write_file"));
    }

    #[test]
    fn test_pattern_anchored_to_full_name() {
        let executor = MockExecutor::new(&["navigate", "pre_navigate_post"]);
        let filtered = FilteredToolExecutor::new(executor, vec!["navigate".to_string()]).unwrap();

        // Only exact match, not substring
        assert_eq!(filtered.tool_definitions().len(), 1);
        assert_eq!(filtered.tool_definitions()[0].name, "navigate");
    }

    #[test]
    fn test_multiple_patterns_union() {
        let executor = MockExecutor::new(&["navigate", "click", "screenshot", "bash", "read_file"]);
        let filtered = FilteredToolExecutor::new(
            executor,
            vec![
                "navigate".to_string(),
                "bash".to_string(),
                "read_.*".to_string(),
            ],
        )
        .unwrap();

        let names: Vec<&str> = filtered
            .tool_definitions()
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(names, vec!["navigate", "bash", "read_file"]);
    }

    #[tokio::test]
    async fn test_execute_with_empty_patterns_rejects_all() {
        let executor = MockExecutor::new(&["navigate"]);
        let filtered = FilteredToolExecutor::new(executor, vec![]).unwrap();

        let result = filtered.execute_tool("navigate", json!({})).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("not allowed"));
    }

    #[test]
    fn test_dot_star_matches_all_tools() {
        let executor = MockExecutor::new(&["navigate", "click", "bash"]);
        let filtered = FilteredToolExecutor::new(executor, vec![".*".to_string()]).unwrap();

        assert_eq!(filtered.tool_definitions().len(), 3);
    }
}
