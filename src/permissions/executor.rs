use async_trait::async_trait;
use serde_json::Value;

use crate::browser::mcp::{ToolExecutionResult, ToolExecutor};
use crate::error::AgentError;
use crate::llm::types::ToolDefinition;

use super::types::{PermissionDecision, PermissionMode, PermissionPolicy};

/// Read-only tools allowed in Plan mode (must match `PLAN_MODE_ALLOWED` in types.rs).
const PLAN_MODE_ALLOWED: &[&str] = &[
    "read_file",
    "grep",
    "glob",
    "load_skill",
    "read_skill_resource",
];

/// Permission request sent through the channel for interactive TUI prompts.
#[derive(Debug)]
pub struct PermissionRequest {
    pub tool_name: String,
    pub tool_input: serde_json::Value,
    pub respond: tokio::sync::oneshot::Sender<bool>,
}

/// Decorator that enforces permission policies before delegating tool calls.
pub struct PermissionAwareExecutor<T: ToolExecutor> {
    inner: T,
    policy: PermissionPolicy,
    /// Cached filtered tool definitions for Plan mode.
    filtered_tools: Vec<ToolDefinition>,
    /// Optional channel for sending permission requests to a TUI.
    permission_tx: Option<tokio::sync::mpsc::Sender<PermissionRequest>>,
}

impl<T: ToolExecutor> PermissionAwareExecutor<T> {
    pub fn new(inner: T, policy: PermissionPolicy) -> Self {
        let filtered_tools = if policy.mode == PermissionMode::Plan {
            inner
                .tool_definitions()
                .iter()
                .filter(|t| PLAN_MODE_ALLOWED.contains(&t.name.as_str()))
                .cloned()
                .collect()
        } else {
            Vec::new()
        };

        Self {
            inner,
            policy,
            filtered_tools,
            permission_tx: None,
        }
    }

    /// Set the permission channel for interactive TUI prompts.
    pub fn with_permission_channel(
        mut self,
        tx: tokio::sync::mpsc::Sender<PermissionRequest>,
    ) -> Self {
        self.permission_tx = Some(tx);
        self
    }

    pub fn into_inner(self) -> T {
        self.inner
    }

    /// Borrow the wrapped executor.
    ///
    /// Needed because this decorator is outermost in the chain, so callers reaching for
    /// an inner executor's own API (e.g. `CoordinationExecutor::wait_for_children`)
    /// have to go through it.
    pub fn inner(&self) -> &T {
        &self.inner
    }

    pub fn policy(&self) -> &PermissionPolicy {
        &self.policy
    }
}

#[async_trait]
impl<T: ToolExecutor> ToolExecutor for PermissionAwareExecutor<T> {
    fn tool_definitions(&self) -> &[ToolDefinition] {
        if self.policy.mode == PermissionMode::Plan {
            &self.filtered_tools
        } else {
            self.inner.tool_definitions()
        }
    }

    async fn execute_tool(
        &self,
        name: &str,
        arguments: Value,
    ) -> Result<ToolExecutionResult, AgentError> {
        match self.policy.check(name) {
            PermissionDecision::Allow => self.inner.execute_tool(name, arguments).await,
            PermissionDecision::Deny { reason } => Ok(ToolExecutionResult {
                content: format!("Permission denied: {reason}"),
                is_error: true,
            }),
            PermissionDecision::Ask => {
                if let Some(ref tx) = self.permission_tx {
                    let (respond_tx, respond_rx) = tokio::sync::oneshot::channel();
                    let request = PermissionRequest {
                        tool_name: name.to_string(),
                        tool_input: arguments.clone(),
                        respond: respond_tx,
                    };
                    // Send the request; if the channel is closed, deny.
                    if tx.send(request).await.is_err() {
                        return Ok(ToolExecutionResult {
                            content: "Permission denied: permission channel closed".to_string(),
                            is_error: true,
                        });
                    }
                    // Await response with a 30-second timeout.
                    let allowed =
                        match tokio::time::timeout(std::time::Duration::from_secs(30), respond_rx)
                            .await
                        {
                            Ok(Ok(true)) => true,
                            Ok(Ok(false)) => false,
                            Ok(Err(_)) => false, // Sender dropped → deny
                            Err(_) => false,     // Timeout → deny
                        };
                    if allowed {
                        self.inner.execute_tool(name, arguments).await
                    } else {
                        Ok(ToolExecutionResult {
                            content: format!("Permission denied: user rejected tool '{name}'"),
                            is_error: true,
                        })
                    }
                } else {
                    // In non-interactive mode, treat Ask as Allow.
                    self.inner.execute_tool(name, arguments).await
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Mock executor that records calls and returns a configurable response.
    struct MockExecutor {
        tools: Vec<ToolDefinition>,
        response: ToolExecutionResult,
    }

    #[async_trait]
    impl ToolExecutor for MockExecutor {
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

    fn mock_with_tools(names: &[&str]) -> MockExecutor {
        let tools = names
            .iter()
            .map(|n| ToolDefinition {
                name: n.to_string(),
                description: format!("{n} tool"),
                input_schema: json!({"type": "object"}),
                cache_control: None,
                read_only: false,
            })
            .collect();
        MockExecutor {
            tools,
            response: ToolExecutionResult {
                content: "ok".to_string(),
                is_error: false,
            },
        }
    }

    fn default_mock() -> MockExecutor {
        mock_with_tools(&[
            "read_file",
            "write_file",
            "bash",
            "grep",
            "glob",
            "navigate",
        ])
    }

    // --- tool_definitions delegation ---

    #[test]
    fn test_tool_definitions_delegates_in_default_mode() {
        let inner = default_mock();
        let executor = PermissionAwareExecutor::new(inner, PermissionPolicy::default());
        assert_eq!(executor.tool_definitions().len(), 6);
    }

    #[test]
    fn test_tool_definitions_filtered_in_plan_mode() {
        let inner = default_mock();
        let policy = PermissionPolicy {
            mode: PermissionMode::Plan,
            ..Default::default()
        };
        let executor = PermissionAwareExecutor::new(inner, policy);
        let names: Vec<&str> = executor
            .tool_definitions()
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        // Only read_file, grep, glob should remain (navigate, write_file, bash filtered out)
        assert_eq!(names.len(), 3);
        assert!(names.contains(&"read_file"));
        assert!(names.contains(&"grep"));
        assert!(names.contains(&"glob"));
        assert!(!names.contains(&"write_file"));
        assert!(!names.contains(&"bash"));
        assert!(!names.contains(&"navigate"));
    }

    #[test]
    fn test_tool_definitions_not_filtered_in_bypass_mode() {
        let inner = default_mock();
        let policy = PermissionPolicy {
            mode: PermissionMode::BypassPermissions,
            ..Default::default()
        };
        let executor = PermissionAwareExecutor::new(inner, policy);
        assert_eq!(executor.tool_definitions().len(), 6);
    }

    #[test]
    fn test_tool_definitions_not_filtered_in_accept_edits_mode() {
        let inner = default_mock();
        let policy = PermissionPolicy {
            mode: PermissionMode::AcceptEdits,
            ..Default::default()
        };
        let executor = PermissionAwareExecutor::new(inner, policy);
        assert_eq!(executor.tool_definitions().len(), 6);
    }

    // --- execute_tool with BypassPermissions ---

    #[tokio::test]
    async fn test_bypass_passes_through() {
        let inner = default_mock();
        let policy = PermissionPolicy {
            mode: PermissionMode::BypassPermissions,
            ..Default::default()
        };
        let executor = PermissionAwareExecutor::new(inner, policy);
        let result = executor
            .execute_tool("bash", json!({"command": "ls"}))
            .await
            .unwrap();
        assert_eq!(result.content, "ok");
        assert!(!result.is_error);
    }

    // --- execute_tool with denied tool ---

    #[tokio::test]
    async fn test_denied_tool_returns_error() {
        let inner = default_mock();
        let policy = PermissionPolicy {
            mode: PermissionMode::Default,
            allowed_tools: vec![],
            denied_tools: vec!["bash".to_string()],
        };
        let executor = PermissionAwareExecutor::new(inner, policy);
        let result = executor
            .execute_tool("bash", json!({"command": "rm -rf /"}))
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Permission denied"));
    }

    // --- execute_tool with Plan mode ---

    #[tokio::test]
    async fn test_plan_mode_allows_read_file() {
        let inner = default_mock();
        let policy = PermissionPolicy {
            mode: PermissionMode::Plan,
            ..Default::default()
        };
        let executor = PermissionAwareExecutor::new(inner, policy);
        let result = executor
            .execute_tool("read_file", json!({"path": "/tmp/test"}))
            .await
            .unwrap();
        assert_eq!(result.content, "ok");
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_plan_mode_blocks_write_file() {
        let inner = default_mock();
        let policy = PermissionPolicy {
            mode: PermissionMode::Plan,
            ..Default::default()
        };
        let executor = PermissionAwareExecutor::new(inner, policy);
        let result = executor
            .execute_tool("write_file", json!({"path": "/tmp/test", "content": "x"}))
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Permission denied"));
    }

    #[tokio::test]
    async fn test_plan_mode_blocks_bash() {
        let inner = default_mock();
        let policy = PermissionPolicy {
            mode: PermissionMode::Plan,
            ..Default::default()
        };
        let executor = PermissionAwareExecutor::new(inner, policy);
        let result = executor
            .execute_tool("bash", json!({"command": "echo hi"}))
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Plan mode"));
    }

    // --- Ask decision forwards to inner ---

    #[tokio::test]
    async fn test_ask_forwards_to_inner() {
        let inner = default_mock();
        let policy = PermissionPolicy::default();
        let executor = PermissionAwareExecutor::new(inner, policy);
        // Default mode with no patterns → Ask → forwards
        let result = executor
            .execute_tool("navigate", json!({"url": "https://example.com"}))
            .await
            .unwrap();
        assert_eq!(result.content, "ok");
        assert!(!result.is_error);
    }

    // --- into_inner ---

    #[test]
    fn test_into_inner_recovers_executor() {
        let inner = default_mock();
        let executor = PermissionAwareExecutor::new(inner, PermissionPolicy::default());
        let recovered = executor.into_inner();
        assert_eq!(recovered.tool_definitions().len(), 6);
    }

    // --- policy() accessor ---

    #[test]
    fn test_policy_accessor() {
        let inner = default_mock();
        let policy = PermissionPolicy {
            mode: PermissionMode::Plan,
            allowed_tools: vec!["x".to_string()],
            denied_tools: vec!["y".to_string()],
        };
        let executor = PermissionAwareExecutor::new(inner, policy);
        assert_eq!(executor.policy().mode, PermissionMode::Plan);
        assert_eq!(executor.policy().allowed_tools, vec!["x"]);
        assert_eq!(executor.policy().denied_tools, vec!["y"]);
    }

    // --- Regex patterns in executor context ---

    #[tokio::test]
    async fn test_regex_allowed_pattern_passes_through() {
        let inner = default_mock();
        let policy = PermissionPolicy {
            mode: PermissionMode::Default,
            allowed_tools: vec!["nav.*".to_string()],
            denied_tools: vec![],
        };
        let executor = PermissionAwareExecutor::new(inner, policy);
        let result = executor.execute_tool("navigate", json!({})).await.unwrap();
        assert_eq!(result.content, "ok");
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_regex_denied_pattern_blocks() {
        let inner = default_mock();
        let policy = PermissionPolicy {
            mode: PermissionMode::Default,
            allowed_tools: vec![],
            denied_tools: vec!["write_.*".to_string()],
        };
        let executor = PermissionAwareExecutor::new(inner, policy);
        let result = executor
            .execute_tool("write_file", json!({}))
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Permission denied"));
    }

    // --- Plan mode with skill tools ---

    #[tokio::test]
    async fn test_plan_mode_allows_load_skill() {
        let inner = mock_with_tools(&["load_skill", "run_skill_script", "read_skill_resource"]);
        let policy = PermissionPolicy {
            mode: PermissionMode::Plan,
            ..Default::default()
        };
        let executor = PermissionAwareExecutor::new(inner, policy);
        let result = executor
            .execute_tool("load_skill", json!({}))
            .await
            .unwrap();
        assert!(!result.is_error);

        let result = executor
            .execute_tool("read_skill_resource", json!({}))
            .await
            .unwrap();
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_plan_mode_blocks_run_skill_script() {
        let inner = mock_with_tools(&["load_skill", "run_skill_script"]);
        let policy = PermissionPolicy {
            mode: PermissionMode::Plan,
            ..Default::default()
        };
        let executor = PermissionAwareExecutor::new(inner, policy);
        let result = executor
            .execute_tool("run_skill_script", json!({}))
            .await
            .unwrap();
        assert!(result.is_error);
    }

    // --- Plan mode tool_definitions includes skill read tools ---

    #[test]
    fn test_plan_mode_definitions_include_skill_read_tools() {
        let inner = mock_with_tools(&[
            "read_file",
            "write_file",
            "bash",
            "grep",
            "glob",
            "load_skill",
            "run_skill_script",
            "read_skill_resource",
        ]);
        let policy = PermissionPolicy {
            mode: PermissionMode::Plan,
            ..Default::default()
        };
        let executor = PermissionAwareExecutor::new(inner, policy);
        let names: Vec<&str> = executor
            .tool_definitions()
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(names.len(), 5);
        assert!(names.contains(&"read_file"));
        assert!(names.contains(&"grep"));
        assert!(names.contains(&"glob"));
        assert!(names.contains(&"load_skill"));
        assert!(names.contains(&"read_skill_resource"));
    }

    // --- Denied pattern with allowed catch-all ---

    #[tokio::test]
    async fn test_deny_overrides_allow_all() {
        let inner = default_mock();
        let policy = PermissionPolicy {
            mode: PermissionMode::Default,
            allowed_tools: vec![".*".to_string()],
            denied_tools: vec!["bash".to_string()],
        };
        let executor = PermissionAwareExecutor::new(inner, policy);
        let result = executor.execute_tool("bash", json!({})).await.unwrap();
        assert!(result.is_error);

        // Other tools still pass
        let result = executor.execute_tool("read_file", json!({})).await.unwrap();
        assert!(!result.is_error);
    }

    // --- Permission channel tests ---

    #[tokio::test]
    async fn test_permission_channel_allow() {
        let inner = default_mock();
        let policy = PermissionPolicy::default();
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let executor = PermissionAwareExecutor::new(inner, policy).with_permission_channel(tx);

        // Spawn a task to respond with Allow
        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                assert_eq!(req.tool_name, "navigate");
                req.respond.send(true).unwrap();
            }
        });

        let result = executor
            .execute_tool("navigate", json!({"url": "https://example.com"}))
            .await
            .unwrap();
        assert_eq!(result.content, "ok");
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_permission_channel_deny() {
        let inner = default_mock();
        let policy = PermissionPolicy::default();
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let executor = PermissionAwareExecutor::new(inner, policy).with_permission_channel(tx);

        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                req.respond.send(false).unwrap();
            }
        });

        let result = executor.execute_tool("navigate", json!({})).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Permission denied"));
    }

    #[tokio::test]
    async fn test_permission_channel_timeout() {
        let inner = default_mock();
        let policy = PermissionPolicy::default();
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let executor = PermissionAwareExecutor::new(inner, policy).with_permission_channel(tx);

        // Receive but never respond — will timeout
        tokio::spawn(async move {
            let _req = rx.recv().await;
            // Drop the request without responding, causing oneshot RecvError
        });

        let result = executor.execute_tool("navigate", json!({})).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Permission denied"));
    }

    #[tokio::test]
    async fn test_no_permission_channel_auto_allows() {
        let inner = default_mock();
        let policy = PermissionPolicy::default();
        // No channel set — should auto-allow on Ask
        let executor = PermissionAwareExecutor::new(inner, policy);

        let result = executor.execute_tool("navigate", json!({})).await.unwrap();
        assert_eq!(result.content, "ok");
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_permission_channel_closed_denies() {
        let inner = default_mock();
        let policy = PermissionPolicy::default();
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let executor = PermissionAwareExecutor::new(inner, policy).with_permission_channel(tx);

        // Drop the receiver immediately
        drop(rx);

        let result = executor.execute_tool("navigate", json!({})).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Permission denied"));
        assert!(result.content.contains("channel closed"));
    }

    #[tokio::test]
    async fn test_permission_channel_not_used_for_allowed_tools() {
        let inner = default_mock();
        let policy = PermissionPolicy {
            mode: PermissionMode::Default,
            allowed_tools: vec!["navigate".to_string()],
            denied_tools: vec![],
        };
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let executor = PermissionAwareExecutor::new(inner, policy).with_permission_channel(tx);

        // If the channel were used, this would hang since nobody reads from rx
        let result = executor.execute_tool("navigate", json!({})).await.unwrap();
        assert_eq!(result.content, "ok");
        assert!(!result.is_error);

        // Verify nothing was sent
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn test_permission_channel_not_used_for_denied_tools() {
        let inner = default_mock();
        let policy = PermissionPolicy {
            mode: PermissionMode::Default,
            allowed_tools: vec![],
            denied_tools: vec!["bash".to_string()],
        };
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let executor = PermissionAwareExecutor::new(inner, policy).with_permission_channel(tx);

        let result = executor.execute_tool("bash", json!({})).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Permission denied"));

        // Verify nothing was sent to the channel
        assert!(rx.try_recv().is_err());
    }
}
