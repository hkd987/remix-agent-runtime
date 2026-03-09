use async_trait::async_trait;
use serde_json::{json, Value};
use std::time::Duration;

use crate::browser::mcp::{ToolExecutionResult, ToolExecutor};
use crate::error::AgentError;
use crate::llm::types::ToolDefinition;
use crate::plugins::components::hooks::{HookRegistry, HookTiming};

/// Structured output parsed from hook stdout.
///
/// Hooks may optionally emit a JSON object to stdout containing any of these
/// keys. If multiple hooks return decisions, the last one wins.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HookResult {
    /// Permission decision: "allow", "deny", or "ask"
    pub permission_decision: Option<String>,
    /// Replacement input for the tool call
    pub updated_input: Option<Value>,
    /// System message to inject into the conversation
    pub system_message: Option<String>,
}

/// Decorator that wraps a ToolExecutor and fires hooks before/after tool calls.
///
/// Hook failures are logged and ignored (never block the agent loop).
pub struct HookAwareExecutor<T: ToolExecutor> {
    inner: T,
    hook_registry: HookRegistry,
    timeout_secs: u64,
}

/// Context for lifecycle hooks, serialized as JSON to hook stdin.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LifecycleHookContext {
    #[serde(flatten)]
    pub fields: serde_json::Map<String, Value>,
}

impl<T: ToolExecutor> HookAwareExecutor<T> {
    pub fn new(inner: T, hook_registry: HookRegistry, timeout_secs: u64) -> Self {
        Self {
            inner,
            hook_registry,
            timeout_secs,
        }
    }

    pub fn into_inner(self) -> T {
        self.inner
    }

    /// Get a reference to the hook registry.
    pub fn registry(&self) -> &HookRegistry {
        &self.hook_registry
    }

    /// Fire lifecycle hooks (SessionStart, SessionEnd, Stop, PreCompact, SubagentStart/Stop).
    /// These hooks match by timing only — all hooks with the given timing will fire.
    /// Context is passed as JSON via stdin.
    pub async fn fire_lifecycle_hook(&self, timing: &HookTiming, context: Value) {
        let hooks = self.hook_registry.lifecycle_hooks(timing);
        if hooks.is_empty() {
            return;
        }

        let context_bytes = match serde_json::to_vec(&context) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to serialize lifecycle hook context");
                return;
            }
        };

        let timeout = Duration::from_secs(self.timeout_secs);

        for hook in hooks {
            let mut cmd = tokio::process::Command::new("sh");
            cmd.arg("-c")
                .arg(&hook.command)
                .current_dir(&hook.plugin_root)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true);

            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(
                        command = %hook.command,
                        timing = ?timing,
                        error = %e,
                        "Failed to spawn lifecycle hook command"
                    );
                    continue;
                }
            };

            if let Some(stdin) = child.stdin.take() {
                use tokio::io::AsyncWriteExt;
                let mut stdin = stdin;
                if let Err(e) = stdin.write_all(&context_bytes).await {
                    tracing::warn!(
                        command = %hook.command,
                        error = %e,
                        "Failed to write to lifecycle hook stdin"
                    );
                }
                drop(stdin);
            }

            match tokio::time::timeout(timeout, child.wait()).await {
                Ok(Ok(status)) => {
                    if !status.success() {
                        tracing::warn!(
                            command = %hook.command,
                            timing = ?timing,
                            exit_code = status.code().unwrap_or(-1),
                            "Lifecycle hook exited with non-zero status"
                        );
                    }
                }
                Ok(Err(e)) => {
                    tracing::warn!(
                        command = %hook.command,
                        timing = ?timing,
                        error = %e,
                        "Lifecycle hook command failed"
                    );
                }
                Err(_) => {
                    tracing::warn!(
                        command = %hook.command,
                        timing = ?timing,
                        timeout_secs = self.timeout_secs,
                        "Lifecycle hook timed out"
                    );
                }
            }
        }
    }

    /// Execute matching hooks for a given tool call and timing.
    ///
    /// Sends context via stdin as JSON:
    /// ```json
    /// { "tool_name": "...", "tool_input": {...}, "tool_output": "..." }
    /// ```
    /// `tool_output` is only present for PostToolUse hooks.
    ///
    /// Returns a [`HookResult`] parsed from hook stdout. If multiple hooks
    /// produce structured output, the last one's values win.
    async fn run_hooks(
        &self,
        tool_name: &str,
        tool_input: &Value,
        tool_output: Option<&str>,
        timing: &HookTiming,
    ) -> HookResult {
        let hooks = self.hook_registry.matching_hooks(tool_name, timing);
        if hooks.is_empty() {
            return HookResult::default();
        }

        let mut context = json!({
            "tool_name": tool_name,
            "tool_input": tool_input,
        });
        if let Some(output) = tool_output {
            context["tool_output"] = Value::String(output.to_string());
        }
        let context_bytes = match serde_json::to_vec(&context) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to serialize hook context");
                return HookResult::default();
            }
        };

        let timeout = Duration::from_secs(self.timeout_secs);
        let mut result = HookResult::default();

        for hook in hooks {
            let mut cmd = tokio::process::Command::new("sh");
            cmd.arg("-c")
                .arg(&hook.command)
                .current_dir(&hook.plugin_root)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true);

            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(
                        command = %hook.command,
                        error = %e,
                        "Failed to spawn hook command"
                    );
                    continue;
                }
            };

            // Write context to stdin
            if let Some(stdin) = child.stdin.take() {
                use tokio::io::AsyncWriteExt;
                let mut stdin = stdin;
                if let Err(e) = stdin.write_all(&context_bytes).await {
                    tracing::warn!(
                        command = %hook.command,
                        error = %e,
                        "Failed to write to hook stdin"
                    );
                }
                // Drop stdin to close the pipe so the child can read EOF
                drop(stdin);
            }

            match tokio::time::timeout(timeout, child.wait_with_output()).await {
                Ok(Ok(output)) => {
                    if !output.status.success() {
                        tracing::warn!(
                            command = %hook.command,
                            exit_code = output.status.code().unwrap_or(-1),
                            "Hook exited with non-zero status"
                        );
                    }

                    // Attempt to parse stdout as JSON for structured output
                    if let Ok(stdout_str) = String::from_utf8(output.stdout) {
                        let trimmed = stdout_str.trim();
                        if !trimmed.is_empty() {
                            if let Ok(parsed) = serde_json::from_str::<Value>(trimmed) {
                                if let Some(decision) = parsed.get("permissionDecision").and_then(|v| v.as_str()) {
                                    result.permission_decision = Some(decision.to_string());
                                }
                                if let Some(input) = parsed.get("updatedInput") {
                                    result.updated_input = Some(input.clone());
                                }
                                if let Some(msg) = parsed.get("systemMessage").and_then(|v| v.as_str()) {
                                    result.system_message = Some(msg.to_string());
                                }
                            }
                            // If JSON parsing fails, silently ignore (backwards compatible)
                        }
                    }
                }
                Ok(Err(e)) => {
                    tracing::warn!(
                        command = %hook.command,
                        error = %e,
                        "Hook command failed"
                    );
                }
                Err(_) => {
                    tracing::warn!(
                        command = %hook.command,
                        timeout_secs = self.timeout_secs,
                        "Hook timed out"
                    );
                    // kill_on_drop will handle cleanup
                }
            }
        }

        result
    }
}

#[async_trait]
impl<T: ToolExecutor> ToolExecutor for HookAwareExecutor<T> {
    fn tool_definitions(&self) -> &[ToolDefinition] {
        self.inner.tool_definitions()
    }

    async fn execute_tool(
        &self,
        name: &str,
        arguments: Value,
    ) -> Result<ToolExecutionResult, AgentError> {
        // Run PreToolUse hooks
        self.run_hooks(name, &arguments, None, &HookTiming::PreToolUse)
            .await;

        // Execute actual tool
        match self.inner.execute_tool(name, arguments.clone()).await {
            Ok(result) => {
                if result.is_error {
                    // Fire PostToolUseFailure hooks for tool-level errors
                    self.run_hooks(
                        name,
                        &arguments,
                        Some(&result.content),
                        &HookTiming::PostToolUseFailure,
                    )
                    .await;
                } else {
                    // Run PostToolUse hooks for successful results
                    self.run_hooks(
                        name,
                        &arguments,
                        Some(&result.content),
                        &HookTiming::PostToolUse,
                    )
                    .await;
                }
                Ok(result)
            }
            Err(e) => {
                // Fire PostToolUseFailure hooks for execution errors
                self.run_hooks(
                    name,
                    &arguments,
                    Some(&e.to_string()),
                    &HookTiming::PostToolUseFailure,
                )
                .await;
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::components::hooks::HookRegistry;
    use serde_json::json;
    use tempfile::TempDir;

    struct MockInnerExecutor {
        tools: Vec<ToolDefinition>,
        response: ToolExecutionResult,
    }

    #[async_trait]
    impl ToolExecutor for MockInnerExecutor {
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

    /// Mock inner executor that returns an error.
    struct FailingMockExecutor;

    #[async_trait]
    impl ToolExecutor for FailingMockExecutor {
        fn tool_definitions(&self) -> &[ToolDefinition] {
            &[]
        }

        async fn execute_tool(
            &self,
            name: &str,
            _arguments: Value,
        ) -> Result<ToolExecutionResult, AgentError> {
            Err(AgentError::ToolExecution(format!("Mock failure: {name}")))
        }
    }

    fn mock_inner() -> MockInnerExecutor {
        MockInnerExecutor {
            tools: vec![ToolDefinition {
                name: "navigate".to_string(),
                description: "Navigate to URL".to_string(),
                input_schema: json!({"type": "object"}),
            }],
            response: ToolExecutionResult {
                content: "Page loaded".to_string(),
                is_error: false,
            },
        }
    }

    fn empty_registry() -> HookRegistry {
        HookRegistry::new()
    }

    /// Build a registry with a single hook. The `plugin_root` must be a directory
    /// that outlives the registry (controls cwd for hook processes).
    fn registry_with_hook(
        plugin_root: &std::path::Path,
        matcher: &str,
        timing: HookTiming,
        command: &str,
    ) -> HookRegistry {
        let mut registry = HookRegistry::new();
        let timing_key = match timing {
            HookTiming::PreToolUse => "PreToolUse",
            HookTiming::PostToolUse => "PostToolUse",
            HookTiming::PostToolUseFailure => "PostToolUseFailure",
            HookTiming::SessionStart => "SessionStart",
            HookTiming::SessionEnd => "SessionEnd",
            HookTiming::PreCompact => "PreCompact",
            HookTiming::Stop => "Stop",
            HookTiming::SubagentStart => "SubagentStart",
            HookTiming::SubagentStop => "SubagentStop",
        };
        let hooks_json = format!(
            r#"{{ "hooks": {{ "{timing_key}": [{{ "matcher": "{matcher}", "hooks": [{{ "type": "command", "command": "{command}" }}] }}] }} }}"#,
        );
        let path = plugin_root.join("hooks.json");
        std::fs::write(&path, hooks_json).unwrap();
        registry.load_from_file(&path, plugin_root).unwrap();
        registry
    }

    #[test]
    fn test_hook_aware_executor_delegates_tool_definitions() {
        let inner = mock_inner();
        let executor = HookAwareExecutor::new(inner, empty_registry(), 30);
        assert_eq!(executor.tool_definitions().len(), 1);
        assert_eq!(executor.tool_definitions()[0].name, "navigate");
    }

    #[tokio::test]
    async fn test_execute_tool_no_hooks() {
        let inner = mock_inner();
        let executor = HookAwareExecutor::new(inner, empty_registry(), 30);
        let result = executor
            .execute_tool("navigate", json!({"url": "https://example.com"}))
            .await
            .unwrap();
        assert_eq!(result.content, "Page loaded");
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_execute_tool_with_pre_hook() {
        let dir = TempDir::new().unwrap();
        let marker = dir.path().join("pre_hook_ran");
        let command = format!("touch {}", marker.display());

        let registry = registry_with_hook(dir.path(), "navigate", HookTiming::PreToolUse, &command);
        let inner = mock_inner();
        let executor = HookAwareExecutor::new(inner, registry, 30);

        let result = executor
            .execute_tool("navigate", json!({"url": "test"}))
            .await
            .unwrap();

        // Tool should still succeed
        assert_eq!(result.content, "Page loaded");
        // Give a moment for async file creation
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(marker.exists(), "Pre-hook should have created marker file");
    }

    #[tokio::test]
    async fn test_execute_tool_with_post_hook() {
        let dir = TempDir::new().unwrap();
        let marker = dir.path().join("post_hook_ran");
        let command = format!("touch {}", marker.display());

        let registry =
            registry_with_hook(dir.path(), "navigate", HookTiming::PostToolUse, &command);
        let inner = mock_inner();
        let executor = HookAwareExecutor::new(inner, registry, 30);

        let result = executor
            .execute_tool("navigate", json!({"url": "test"}))
            .await
            .unwrap();

        assert_eq!(result.content, "Page loaded");
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(marker.exists(), "Post-hook should have created marker file");
    }

    #[tokio::test]
    async fn test_hook_receives_context_via_stdin() {
        let dir = TempDir::new().unwrap();
        let output_file = dir.path().join("stdin_content.json");
        let command = format!("cat > {}", output_file.display());

        let registry = registry_with_hook(dir.path(), "navigate", HookTiming::PreToolUse, &command);
        let inner = mock_inner();
        let executor = HookAwareExecutor::new(inner, registry, 30);

        executor
            .execute_tool("navigate", json!({"url": "https://example.com"}))
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(200)).await;
        let content = std::fs::read_to_string(&output_file).unwrap();
        let parsed: Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["tool_name"], "navigate");
        assert_eq!(parsed["tool_input"]["url"], "https://example.com");
        // PreToolUse should not have tool_output
        assert!(parsed.get("tool_output").is_none());
    }

    #[tokio::test]
    async fn test_post_hook_receives_tool_output() {
        let dir = TempDir::new().unwrap();
        let output_file = dir.path().join("post_stdin.json");
        let command = format!("cat > {}", output_file.display());

        let registry =
            registry_with_hook(dir.path(), "navigate", HookTiming::PostToolUse, &command);
        let inner = mock_inner();
        let executor = HookAwareExecutor::new(inner, registry, 30);

        executor
            .execute_tool("navigate", json!({"url": "test"}))
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(200)).await;
        let content = std::fs::read_to_string(&output_file).unwrap();
        let parsed: Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["tool_name"], "navigate");
        assert_eq!(parsed["tool_output"], "Page loaded");
    }

    #[tokio::test]
    async fn test_hook_failure_does_not_block_tool() {
        let dir = TempDir::new().unwrap();
        let registry = registry_with_hook(dir.path(), "navigate", HookTiming::PreToolUse, "exit 1");
        let inner = mock_inner();
        let executor = HookAwareExecutor::new(inner, registry, 30);

        // Tool should still succeed even though hook exits with error
        let result = executor
            .execute_tool("navigate", json!({"url": "test"}))
            .await
            .unwrap();
        assert_eq!(result.content, "Page loaded");
    }

    #[tokio::test]
    async fn test_hook_timeout_does_not_block_tool() {
        let dir = TempDir::new().unwrap();
        let registry =
            registry_with_hook(dir.path(), "navigate", HookTiming::PreToolUse, "sleep 30");
        let inner = mock_inner();
        // 1 second timeout
        let executor = HookAwareExecutor::new(inner, registry, 1);

        let result = executor
            .execute_tool("navigate", json!({"url": "test"}))
            .await
            .unwrap();
        assert_eq!(result.content, "Page loaded");
    }

    #[tokio::test]
    async fn test_non_matching_hooks_not_executed() {
        let dir = TempDir::new().unwrap();
        let marker = dir.path().join("should_not_exist");
        let command = format!("touch {}", marker.display());

        // Hook matches "click", but we execute "navigate"
        let registry = registry_with_hook(dir.path(), "click", HookTiming::PreToolUse, &command);
        let inner = mock_inner();
        let executor = HookAwareExecutor::new(inner, registry, 30);

        executor
            .execute_tool("navigate", json!({"url": "test"}))
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            !marker.exists(),
            "Hook should not run for non-matching tool"
        );
    }

    #[tokio::test]
    async fn test_inner_error_propagated_after_pre_hooks() {
        let dir = TempDir::new().unwrap();
        let registry = registry_with_hook(
            dir.path(),
            "navigate",
            HookTiming::PreToolUse,
            "echo pre ran",
        );
        let executor = HookAwareExecutor::new(FailingMockExecutor, registry, 30);

        let result = executor
            .execute_tool("navigate", json!({"url": "test"}))
            .await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Mock failure: navigate"));
    }

    #[tokio::test]
    async fn test_post_hooks_not_run_on_inner_error() {
        let dir = TempDir::new().unwrap();
        let marker = dir.path().join("post_should_not_exist");
        let command = format!("touch {}", marker.display());

        let registry =
            registry_with_hook(dir.path(), "navigate", HookTiming::PostToolUse, &command);
        let executor = HookAwareExecutor::new(FailingMockExecutor, registry, 30);

        let _ = executor
            .execute_tool("navigate", json!({"url": "test"}))
            .await;

        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            !marker.exists(),
            "Post-hooks should not run when inner tool fails"
        );
    }

    #[test]
    fn test_into_inner() {
        let inner = mock_inner();
        let executor = HookAwareExecutor::new(inner, empty_registry(), 30);
        let recovered = executor.into_inner();
        assert_eq!(recovered.tool_definitions().len(), 1);
    }

    #[tokio::test]
    async fn test_empty_registry_fast_path() {
        // With empty registry, execute_tool should not spawn any processes
        let inner = mock_inner();
        let executor = HookAwareExecutor::new(inner, empty_registry(), 30);

        let start = std::time::Instant::now();
        let result = executor
            .execute_tool("navigate", json!({"url": "test"}))
            .await
            .unwrap();
        let elapsed = start.elapsed();

        assert_eq!(result.content, "Page loaded");
        // Should be nearly instant with no hooks
        assert!(
            elapsed < Duration::from_millis(500),
            "Empty registry should not add latency"
        );
    }

    #[tokio::test]
    async fn test_hook_invalid_command_does_not_block() {
        let dir = TempDir::new().unwrap();
        let registry = registry_with_hook(
            dir.path(),
            "navigate",
            HookTiming::PreToolUse,
            "/nonexistent/binary/that/does/not/exist",
        );
        let inner = mock_inner();
        let executor = HookAwareExecutor::new(inner, registry, 30);

        // Should still succeed - bad commands are logged and ignored
        let result = executor
            .execute_tool("navigate", json!({"url": "test"}))
            .await
            .unwrap();
        assert_eq!(result.content, "Page loaded");
    }

    #[tokio::test]
    async fn test_post_tool_use_failure_hook_on_is_error() {
        let dir = TempDir::new().unwrap();
        let marker = dir.path().join("failure_hook_ran");
        let command = format!("touch {}", marker.display());

        let registry = registry_with_hook(
            dir.path(),
            "navigate",
            HookTiming::PostToolUseFailure,
            &command,
        );
        let inner = MockInnerExecutor {
            tools: vec![ToolDefinition {
                name: "navigate".to_string(),
                description: "Navigate".to_string(),
                input_schema: json!({"type": "object"}),
            }],
            response: ToolExecutionResult {
                content: "Error: not found".to_string(),
                is_error: true,
            },
        };
        let executor = HookAwareExecutor::new(inner, registry, 30);

        let result = executor
            .execute_tool("navigate", json!({"url": "test"}))
            .await
            .unwrap();
        assert!(result.is_error);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            marker.exists(),
            "PostToolUseFailure hook should fire on is_error=true"
        );
    }

    #[tokio::test]
    async fn test_post_tool_use_failure_hook_on_err() {
        let dir = TempDir::new().unwrap();
        let marker = dir.path().join("err_hook_ran");
        let command = format!("touch {}", marker.display());

        let registry = registry_with_hook(
            dir.path(),
            "navigate",
            HookTiming::PostToolUseFailure,
            &command,
        );
        let executor = HookAwareExecutor::new(FailingMockExecutor, registry, 30);

        let result = executor
            .execute_tool("navigate", json!({"url": "test"}))
            .await;
        assert!(result.is_err());
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            marker.exists(),
            "PostToolUseFailure hook should fire on Err"
        );
    }

    #[tokio::test]
    async fn test_lifecycle_hook_fires() {
        let dir = TempDir::new().unwrap();
        let marker = dir.path().join("lifecycle_ran");
        let command = format!("touch {}", marker.display());

        let registry = registry_with_hook(dir.path(), ".*", HookTiming::SessionStart, &command);
        let inner = mock_inner();
        let executor = HookAwareExecutor::new(inner, registry, 30);

        executor
            .fire_lifecycle_hook(
                &HookTiming::SessionStart,
                json!({"session_id": "test-123", "task": "test"}),
            )
            .await;

        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            marker.exists(),
            "Lifecycle hook should fire for SessionStart"
        );
    }

    #[tokio::test]
    async fn test_lifecycle_hook_receives_context() {
        let dir = TempDir::new().unwrap();
        let output_file = dir.path().join("lifecycle_context.json");
        let command = format!("cat > {}", output_file.display());

        let registry = registry_with_hook(dir.path(), ".*", HookTiming::Stop, &command);
        let inner = mock_inner();
        let executor = HookAwareExecutor::new(inner, registry, 30);

        executor
            .fire_lifecycle_hook(
                &HookTiming::Stop,
                json!({"status": "completed", "reason": "task done"}),
            )
            .await;

        tokio::time::sleep(Duration::from_millis(200)).await;
        let content = std::fs::read_to_string(&output_file).unwrap();
        let parsed: Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["status"], "completed");
        assert_eq!(parsed["reason"], "task done");
    }

    #[tokio::test]
    async fn test_lifecycle_hook_empty_registry() {
        let inner = mock_inner();
        let executor = HookAwareExecutor::new(inner, empty_registry(), 30);

        // Should complete without error even with no hooks
        executor
            .fire_lifecycle_hook(&HookTiming::SessionEnd, json!({"session_id": "test"}))
            .await;
    }

    #[test]
    fn test_registry_accessor() {
        let inner = mock_inner();
        let executor = HookAwareExecutor::new(inner, empty_registry(), 30);
        assert!(executor.registry().is_empty());
    }

    #[tokio::test]
    async fn test_success_fires_post_tool_use_not_failure() {
        let dir = TempDir::new().unwrap();
        let success_marker = dir.path().join("post_success");
        let failure_marker = dir.path().join("post_failure");
        let success_cmd = format!("touch {}", success_marker.display());
        let failure_cmd = format!("touch {}", failure_marker.display());

        let mut registry = HookRegistry::new();
        // Add both PostToolUse and PostToolUseFailure hooks
        let hooks_json = format!(
            r#"{{ "hooks": {{
                "PostToolUse": [{{ "matcher": "navigate", "hooks": [{{ "type": "command", "command": "{success_cmd}" }}] }}],
                "PostToolUseFailure": [{{ "matcher": "navigate", "hooks": [{{ "type": "command", "command": "{failure_cmd}" }}] }}]
            }} }}"#,
        );
        let path = dir.path().join("hooks.json");
        std::fs::write(&path, hooks_json).unwrap();
        registry.load_from_file(&path, dir.path()).unwrap();

        let inner = mock_inner();
        let executor = HookAwareExecutor::new(inner, registry, 30);

        executor
            .execute_tool("navigate", json!({"url": "test"}))
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            success_marker.exists(),
            "PostToolUse hook should fire on success"
        );
        assert!(
            !failure_marker.exists(),
            "PostToolUseFailure hook should NOT fire on success"
        );
    }

    // --- HookResult structured output tests ---

    #[test]
    fn test_hook_result_default() {
        let result = HookResult::default();
        assert_eq!(result.permission_decision, None);
        assert_eq!(result.updated_input, None);
        assert_eq!(result.system_message, None);
    }

    #[tokio::test]
    async fn test_run_hooks_returns_default_when_no_hooks() {
        let inner = mock_inner();
        let executor = HookAwareExecutor::new(inner, empty_registry(), 30);

        let result = executor
            .run_hooks("navigate", &json!({}), None, &HookTiming::PreToolUse)
            .await;
        assert_eq!(result, HookResult::default());
    }

    /// Helper: write a shell script that outputs the given content to stdout,
    /// and return the command string to execute it. This avoids quote-escaping
    /// issues when embedding JSON in the hooks.json command field.
    fn script_command(dir: &std::path::Path, name: &str, content: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let script_path = dir.join(name);
        let script_body = format!("#!/bin/sh\n{content}\n");
        std::fs::write(&script_path, &script_body).unwrap();
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        script_path.display().to_string()
    }

    #[tokio::test]
    async fn test_run_hooks_parses_permission_decision() {
        let dir = TempDir::new().unwrap();
        let cmd = script_command(
            dir.path(),
            "hook.sh",
            r#"printf '{"permissionDecision": "allow"}'"#,
        );
        let registry =
            registry_with_hook(dir.path(), "navigate", HookTiming::PreToolUse, &cmd);
        let inner = mock_inner();
        let executor = HookAwareExecutor::new(inner, registry, 30);

        let result = executor
            .run_hooks("navigate", &json!({}), None, &HookTiming::PreToolUse)
            .await;
        assert_eq!(result.permission_decision, Some("allow".to_string()));
        assert_eq!(result.updated_input, None);
        assert_eq!(result.system_message, None);
    }

    #[tokio::test]
    async fn test_run_hooks_parses_updated_input() {
        let dir = TempDir::new().unwrap();
        let cmd = script_command(
            dir.path(),
            "hook.sh",
            r#"printf '{"updatedInput": {"url": "https://changed.com"}}'"#,
        );
        let registry =
            registry_with_hook(dir.path(), "navigate", HookTiming::PreToolUse, &cmd);
        let inner = mock_inner();
        let executor = HookAwareExecutor::new(inner, registry, 30);

        let result = executor
            .run_hooks("navigate", &json!({}), None, &HookTiming::PreToolUse)
            .await;
        assert_eq!(
            result.updated_input,
            Some(json!({"url": "https://changed.com"}))
        );
    }

    #[tokio::test]
    async fn test_run_hooks_parses_system_message() {
        let dir = TempDir::new().unwrap();
        let cmd = script_command(
            dir.path(),
            "hook.sh",
            r#"printf '{"systemMessage": "Please be careful"}'"#,
        );
        let registry =
            registry_with_hook(dir.path(), "navigate", HookTiming::PreToolUse, &cmd);
        let inner = mock_inner();
        let executor = HookAwareExecutor::new(inner, registry, 30);

        let result = executor
            .run_hooks("navigate", &json!({}), None, &HookTiming::PreToolUse)
            .await;
        assert_eq!(
            result.system_message,
            Some("Please be careful".to_string())
        );
    }

    #[tokio::test]
    async fn test_run_hooks_parses_all_fields() {
        let dir = TempDir::new().unwrap();
        let cmd = script_command(
            dir.path(),
            "hook.sh",
            r#"printf '{"permissionDecision":"deny","updatedInput":{"x":1},"systemMessage":"blocked"}'"#,
        );
        let registry =
            registry_with_hook(dir.path(), "navigate", HookTiming::PreToolUse, &cmd);
        let inner = mock_inner();
        let executor = HookAwareExecutor::new(inner, registry, 30);

        let result = executor
            .run_hooks("navigate", &json!({}), None, &HookTiming::PreToolUse)
            .await;
        assert_eq!(result.permission_decision, Some("deny".to_string()));
        assert_eq!(result.updated_input, Some(json!({"x": 1})));
        assert_eq!(result.system_message, Some("blocked".to_string()));
    }

    #[tokio::test]
    async fn test_run_hooks_ignores_invalid_json() {
        let dir = TempDir::new().unwrap();
        let cmd = script_command(dir.path(), "hook.sh", "echo this is not json");
        let registry =
            registry_with_hook(dir.path(), "navigate", HookTiming::PreToolUse, &cmd);
        let inner = mock_inner();
        let executor = HookAwareExecutor::new(inner, registry, 30);

        let result = executor
            .run_hooks("navigate", &json!({}), None, &HookTiming::PreToolUse)
            .await;
        // Should silently ignore and return default
        assert_eq!(result, HookResult::default());
    }

    #[tokio::test]
    async fn test_run_hooks_ignores_empty_stdout() {
        let dir = TempDir::new().unwrap();
        let command = "true"; // produces no output
        let registry =
            registry_with_hook(dir.path(), "navigate", HookTiming::PreToolUse, command);
        let inner = mock_inner();
        let executor = HookAwareExecutor::new(inner, registry, 30);

        let result = executor
            .run_hooks("navigate", &json!({}), None, &HookTiming::PreToolUse)
            .await;
        assert_eq!(result, HookResult::default());
    }

    #[tokio::test]
    async fn test_run_hooks_last_hook_wins() {
        let dir = TempDir::new().unwrap();
        let cmd1 = script_command(
            dir.path(),
            "hook1.sh",
            r#"printf '{"permissionDecision": "allow"}'"#,
        );
        let cmd2 = script_command(
            dir.path(),
            "hook2.sh",
            r#"printf '{"permissionDecision": "deny"}'"#,
        );
        let hooks_json = format!(
            r#"{{
                "hooks": {{
                    "PreToolUse": [{{
                        "matcher": "navigate",
                        "hooks": [
                            {{ "type": "command", "command": "{cmd1}" }},
                            {{ "type": "command", "command": "{cmd2}" }}
                        ]
                    }}]
                }}
            }}"#,
        );
        let path = dir.path().join("hooks.json");
        std::fs::write(&path, hooks_json).unwrap();
        let mut registry = HookRegistry::new();
        registry.load_from_file(&path, dir.path()).unwrap();

        let inner = mock_inner();
        let executor = HookAwareExecutor::new(inner, registry, 30);

        let result = executor
            .run_hooks("navigate", &json!({}), None, &HookTiming::PreToolUse)
            .await;
        // Last hook should win
        assert_eq!(result.permission_decision, Some("deny".to_string()));
    }

    #[tokio::test]
    async fn test_run_hooks_partial_fields_merge() {
        let dir = TempDir::new().unwrap();
        let cmd1 = script_command(
            dir.path(),
            "hook1.sh",
            r#"printf '{"permissionDecision": "allow"}'"#,
        );
        let cmd2 = script_command(
            dir.path(),
            "hook2.sh",
            r#"printf '{"systemMessage": "hello"}'"#,
        );
        let hooks_json = format!(
            r#"{{
                "hooks": {{
                    "PreToolUse": [{{
                        "matcher": "navigate",
                        "hooks": [
                            {{ "type": "command", "command": "{cmd1}" }},
                            {{ "type": "command", "command": "{cmd2}" }}
                        ]
                    }}]
                }}
            }}"#,
        );
        let path = dir.path().join("hooks.json");
        std::fs::write(&path, hooks_json).unwrap();
        let mut registry = HookRegistry::new();
        registry.load_from_file(&path, dir.path()).unwrap();

        let inner = mock_inner();
        let executor = HookAwareExecutor::new(inner, registry, 30);

        let result = executor
            .run_hooks("navigate", &json!({}), None, &HookTiming::PreToolUse)
            .await;
        // First hook's permissionDecision should persist, second adds systemMessage
        assert_eq!(result.permission_decision, Some("allow".to_string()));
        assert_eq!(result.system_message, Some("hello".to_string()));
    }

    #[tokio::test]
    async fn test_execute_tool_backwards_compatible_with_structured_hooks() {
        // Hooks that return structured output should not break execute_tool
        let dir = TempDir::new().unwrap();
        let cmd = script_command(
            dir.path(),
            "hook.sh",
            r#"printf '{"permissionDecision": "allow", "systemMessage": "ok"}'"#,
        );
        let registry =
            registry_with_hook(dir.path(), "navigate", HookTiming::PreToolUse, &cmd);
        let inner = mock_inner();
        let executor = HookAwareExecutor::new(inner, registry, 30);

        let result = executor
            .execute_tool("navigate", json!({"url": "test"}))
            .await
            .unwrap();
        // Tool should still return normal result
        assert_eq!(result.content, "Page loaded");
        assert!(!result.is_error);
    }
}
