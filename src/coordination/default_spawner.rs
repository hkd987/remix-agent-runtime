use std::sync::Arc;

use async_trait::async_trait;
use tracing::{info, warn};

use crate::agent::AgentRunner;
use crate::browser::manager::BrowserManager;
use crate::browser::mcp::{McpBrowserClient, ToolExecutor};
use crate::config::credentials::CredentialSet;
use crate::config::schema::{
    AgentConfig, BrowserConfig, CoordinationConfig, LocalToolsConfig, SkillsConfig,
};
use crate::coordination::context::CoordinationContext;
use crate::coordination::executor::CoordinationExecutor;
use crate::coordination::spawn_handler::{ChildHandle, SpawnDefinition, SpawnHandler};
use crate::error::AgentError;
use crate::llm::client::LlmProvider;
use crate::local_tools::executor::LocalToolsExecutor;
use crate::permissions::executor::PermissionAwareExecutor;
use crate::permissions::types::PermissionPolicy;
use crate::plugins::components::hooks::HookRegistry;
use crate::plugins::composite_executor::CompositeToolExecutor;
use crate::plugins::hook_executor::HookAwareExecutor;
use crate::skills::executor::SkillAwareExecutor;
use crate::skills::SkillSet;
use crate::subagent::filtered_executor::FilteredToolExecutor;

/// Default spawn handler that creates real child agents with their own
/// browser instances and full decorator chains.
pub struct DefaultSpawnHandler<L: LlmProvider + 'static> {
    llm: Arc<L>,
    browser_config: BrowserConfig,
    local_tools_config: LocalToolsConfig,
    skills_config: SkillsConfig,
    skill_set: SkillSet,
    coordination_config: CoordinationConfig,
    coordination_context: Arc<CoordinationContext>,
    tool_result_max_bytes: usize,
    /// The parent's policy. A child must never hold more authority than its parent.
    permission_policy: PermissionPolicy,
    hook_registry: HookRegistry,
    hook_timeout_secs: u64,
}

impl<L: LlmProvider + 'static> DefaultSpawnHandler<L> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        llm: Arc<L>,
        browser_config: BrowserConfig,
        local_tools_config: LocalToolsConfig,
        skills_config: SkillsConfig,
        skill_set: SkillSet,
        coordination_config: CoordinationConfig,
        coordination_context: Arc<CoordinationContext>,
        tool_result_max_bytes: usize,
        permission_policy: PermissionPolicy,
        hook_registry: HookRegistry,
        hook_timeout_secs: u64,
    ) -> Self {
        Self {
            llm,
            browser_config,
            local_tools_config,
            skills_config,
            skill_set,
            coordination_config,
            coordination_context,
            tool_result_max_bytes,
            permission_policy,
            hook_registry,
            hook_timeout_secs,
        }
    }
}

/// The child agent's fully-decorated tool executor.
type ChildExecutor = PermissionAwareExecutor<CoordinationExecutor<Box<dyn ToolExecutor>>>;

/// Build the child agent's tool executor chain from a base executor.
///
/// Factored out for testability: tests can supply a mock `Box<dyn ToolExecutor>`
/// instead of a real MCP browser client.
#[allow(clippy::too_many_arguments)]
fn build_child_chain(
    base_executor: Box<dyn ToolExecutor>,
    definition: &SpawnDefinition,
    skills_config: &SkillsConfig,
    skill_set: &SkillSet,
    local_tools_config: &LocalToolsConfig,
    coordination_config: &CoordinationConfig,
    coordination_context: Arc<CoordinationContext>,
    tool_result_max_bytes: usize,
    permission_policy: PermissionPolicy,
    hook_registry: HookRegistry,
    hook_timeout_secs: u64,
) -> Result<ChildExecutor, AgentError> {
    // 1. Wrap base in CompositeToolExecutor
    let mut composite = CompositeToolExecutor::new();
    composite.add_backend(base_executor);

    // 2. Wrap in SkillAwareExecutor
    let skill_exec = SkillAwareExecutor::new(
        composite,
        skill_set.clone(),
        skills_config.script_timeout_secs,
    );

    // 3. Wrap in LocalToolsExecutor
    let local_exec = LocalToolsExecutor::new(
        skill_exec,
        local_tools_config.clone(),
        tool_result_max_bytes,
    )?;

    // 4. Optionally wrap in FilteredToolExecutor.
    //
    // Note this narrowing comes from `allowed_tools` in the model's own spawn request,
    // so it is a convenience, not a control — the permission layer added in step 6 is
    // what actually bounds the child.
    let executor: Box<dyn ToolExecutor> = if !definition.allowed_tools.is_empty() {
        Box::new(FilteredToolExecutor::new(
            local_exec,
            definition.allowed_tools.clone(),
        )?)
    } else {
        Box::new(local_exec)
    };

    // 5. Wrap in HookAwareExecutor so the child's tool calls fire the same hooks as
    //    the parent's.
    let hook_exec: Box<dyn ToolExecutor> = Box::new(HookAwareExecutor::new(
        executor,
        hook_registry,
        hook_timeout_secs,
    ));

    // 6. Wrap in CoordinationExecutor (shared context, child's name, no spawn_handler)
    let coord_exec = CoordinationExecutor::new(
        hook_exec,
        coordination_config.clone(),
        coordination_context,
        definition.name.clone(),
    );

    // 7. Wrap in PermissionAwareExecutor, outermost, carrying the *parent's* policy.
    //
    // Without this a child spawned from a Plan-mode or DontAsk-mode parent would run
    // with unrestricted bash/write_file — spawn_agent would be a complete escape from
    // the parent's policy. Outermost placement also means the coordination virtual
    // tools are policy-checked, matching the parent chain.
    Ok(PermissionAwareExecutor::new(coord_exec, permission_policy))
}

/// Connect to an MCP browser with retry logic.
///
/// Attempts up to `MAX_ATTEMPTS` connections with increasing delays between
/// failures. This handles transient startup races where the browser process
/// is not yet ready to accept the MCP handshake.
async fn connect_with_retry(
    browser_config: &BrowserConfig,
    agent_name: &str,
) -> Result<McpBrowserClient, AgentError> {
    let retry_delays = [
        Some(std::time::Duration::from_millis(500)),
        Some(std::time::Duration::from_secs(1)),
        None, // final attempt, no retry after this
    ];

    for (attempt, delay) in retry_delays.iter().enumerate() {
        let command = BrowserManager::build_command(browser_config);
        match McpBrowserClient::connect(command).await {
            Ok(client) => {
                if attempt > 0 {
                    info!(
                        agent = %agent_name,
                        attempt = attempt + 1,
                        "Browser connection succeeded after retry"
                    );
                }
                return Ok(client);
            }
            Err(e) => {
                if let Some(d) = delay {
                    warn!(
                        agent = %agent_name,
                        attempt = attempt + 1,
                        max_attempts = retry_delays.len(),
                        error = %e,
                        "Browser connection failed, retrying in {:?}",
                        d
                    );
                    tokio::time::sleep(*d).await;
                } else {
                    return Err(e);
                }
            }
        }
    }

    unreachable!()
}

#[async_trait]
impl<L: LlmProvider + 'static> SpawnHandler for DefaultSpawnHandler<L> {
    async fn spawn_child(&self, definition: SpawnDefinition) -> Result<ChildHandle, AgentError> {
        info!(
            agent = %definition.name,
            task = %definition.task,
            max_iterations = definition.max_iterations,
            timeout_secs = definition.timeout_secs,
            "Spawning child agent"
        );

        // 1. Spawn a new remix-browser via MCP (with retry) if browser is enabled
        let base_executor: Box<dyn ToolExecutor> = if self.browser_config.enabled {
            let mcp_client = connect_with_retry(&self.browser_config, &definition.name).await?;
            Box::new(mcp_client)
        } else {
            info!(
                agent = %definition.name,
                "Browser disabled, spawning child in terminal-only mode"
            );
            // Use an empty CompositeToolExecutor as the base (no browser tools)
            Box::new(CompositeToolExecutor::new())
        };

        // 2. Build child tool chain
        let coord_exec = build_child_chain(
            base_executor,
            &definition,
            &self.skills_config,
            &self.skill_set,
            &self.local_tools_config,
            &self.coordination_config,
            self.coordination_context.clone(),
            self.tool_result_max_bytes,
            self.permission_policy.clone(),
            self.hook_registry.clone(),
            self.hook_timeout_secs,
        )?;

        // 3. Build child AgentConfig
        let child_config = AgentConfig {
            max_iterations: definition.max_iterations,
            system_prompt: definition.system_prompt.clone(),
            timeout_secs: definition.timeout_secs,
            coordination_config: Some(self.coordination_config.clone()),
            tool_result_max_bytes: self.tool_result_max_bytes,
            max_budget_usd: None,
            lazy_tool_discovery: false,
            plan_mode: false,
            reminders: Vec::new(),
            self_critique: None,
            nudge_on_text_only: false,
            nudge_max_count: crate::config::schema::default_nudge_max_count(),
            goal_check_on_complete: false,
            action_reminder_interval: None,
            loop_detection: None,
            reasoning_stages: None,
            iteration_budget_warning_threshold: None,
        };

        // 4. Create AgentRunner and tokio::spawn
        let runner = AgentRunner::new(self.llm.clone(), coord_exec, child_config);
        let task_str = definition.task.clone();
        let name = definition.name.clone();

        let credential_set = CredentialSet::default();
        let skill_set = self.skill_set.clone();

        let join_handle = tokio::spawn(async move {
            runner
                .run(&task_str, &credential_set, &skill_set, &None)
                .await
        });

        Ok(ChildHandle { name, join_handle })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::mcp::ToolExecutionResult;
    use crate::llm::types::{Message, MessagesResponse, SystemContent, ToolDefinition};
    use crate::permissions::types::PermissionMode;
    use serde_json::json;
    use std::sync::Mutex;

    // --- Mock LLM ---
    struct MockLlm {
        responses: Mutex<Vec<MessagesResponse>>,
    }

    #[async_trait]
    impl LlmProvider for MockLlm {
        async fn send_messages(
            &self,
            _system: Option<&[SystemContent]>,
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

    // --- Mock ToolExecutor ---
    struct MockToolExecutor {
        tools: Vec<ToolDefinition>,
    }

    impl MockToolExecutor {
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
    impl ToolExecutor for MockToolExecutor {
        fn tool_definitions(&self) -> &[ToolDefinition] {
            &self.tools
        }

        async fn execute_tool(
            &self,
            name: &str,
            _arguments: serde_json::Value,
        ) -> Result<ToolExecutionResult, AgentError> {
            Ok(ToolExecutionResult {
                content: format!("Executed {name}"),
                is_error: false,
            })
        }
    }

    fn test_coordination_config() -> CoordinationConfig {
        CoordinationConfig {
            enabled: true,
            max_worker_iterations: 10,
            worker_timeout_secs: 120,
            max_workers: 5,
            storage_dir: tempfile::TempDir::new().unwrap().path().to_path_buf(),
        }
    }

    fn test_spawn_definition() -> SpawnDefinition {
        SpawnDefinition {
            name: "worker-1".to_string(),
            task: "Do something".to_string(),
            system_prompt: Some("You are a worker.".to_string()),
            allowed_tools: vec![],
            max_iterations: 10,
            timeout_secs: 120,
        }
    }

    #[test]
    fn test_default_spawn_handler_creation() {
        let llm = Arc::new(MockLlm {
            responses: Mutex::new(vec![]),
        });
        let config = test_coordination_config();
        let context = Arc::new(CoordinationContext::new(config.storage_dir.clone()));

        let handler = DefaultSpawnHandler::new(
            llm,
            BrowserConfig::default(),
            LocalToolsConfig::default(),
            SkillsConfig::default(),
            SkillSet::new(),
            config,
            context,
            32_768,
            PermissionPolicy {
                mode: crate::permissions::types::PermissionMode::BypassPermissions,
                ..Default::default()
            },
            HookRegistry::new(),
            30,
        );

        assert!(handler.browser_config.headless);
        assert!(handler.local_tools_config.enabled);
        assert!(handler.skills_config.enabled);
        assert!(handler.coordination_config.enabled);
    }

    #[test]
    fn test_build_child_chain_without_filter() {
        let config = test_coordination_config();
        let context = Arc::new(CoordinationContext::new(config.storage_dir.clone()));
        let base = Box::new(MockToolExecutor::new(&["navigate", "click"]));

        let definition = test_spawn_definition();
        let local_config = LocalToolsConfig {
            enabled: false,
            ..Default::default()
        };

        let chain = build_child_chain(
            base,
            &definition,
            &SkillsConfig::default(),
            &SkillSet::new(),
            &local_config,
            &config,
            context,
            32_768,
            PermissionPolicy {
                mode: crate::permissions::types::PermissionMode::BypassPermissions,
                ..Default::default()
            },
            HookRegistry::new(),
            30,
        )
        .unwrap();

        let tool_names: Vec<&str> = chain
            .tool_definitions()
            .iter()
            .map(|t| t.name.as_str())
            .collect();

        // Should have: 7 coordination virtual tools + navigate + click = 9
        assert_eq!(chain.tool_definitions().len(), 9);
        assert!(tool_names.contains(&"navigate"));
        assert!(tool_names.contains(&"click"));
        assert!(tool_names.contains(&"task_create"));
        assert!(tool_names.contains(&"spawn_agent"));
    }

    #[test]
    fn test_build_child_chain_with_filter() {
        let config = test_coordination_config();
        let context = Arc::new(CoordinationContext::new(config.storage_dir.clone()));
        let base = Box::new(MockToolExecutor::new(&["navigate", "click", "screenshot"]));

        let definition = SpawnDefinition {
            allowed_tools: vec!["navigate".to_string()],
            ..test_spawn_definition()
        };
        let local_config = LocalToolsConfig {
            enabled: false,
            ..Default::default()
        };

        let chain = build_child_chain(
            base,
            &definition,
            &SkillsConfig::default(),
            &SkillSet::new(),
            &local_config,
            &config,
            context,
            32_768,
            PermissionPolicy {
                mode: crate::permissions::types::PermissionMode::BypassPermissions,
                ..Default::default()
            },
            HookRegistry::new(),
            30,
        )
        .unwrap();

        let tool_names: Vec<&str> = chain
            .tool_definitions()
            .iter()
            .map(|t| t.name.as_str())
            .collect();

        // Should have: 7 coordination virtual tools + navigate (filtered) = 8
        assert_eq!(chain.tool_definitions().len(), 8);
        assert!(tool_names.contains(&"navigate"));
        assert!(!tool_names.contains(&"click"));
        assert!(!tool_names.contains(&"screenshot"));
    }

    #[test]
    fn test_build_child_chain_with_local_tools_enabled() {
        let config = test_coordination_config();
        let context = Arc::new(CoordinationContext::new(config.storage_dir.clone()));
        let base = Box::new(MockToolExecutor::new(&["navigate"]));

        let definition = test_spawn_definition();
        let local_config = LocalToolsConfig::default();

        let chain = build_child_chain(
            base,
            &definition,
            &SkillsConfig::default(),
            &SkillSet::new(),
            &local_config,
            &config,
            context,
            32_768,
            PermissionPolicy {
                mode: crate::permissions::types::PermissionMode::BypassPermissions,
                ..Default::default()
            },
            HookRegistry::new(),
            30,
        )
        .unwrap();

        let tool_names: Vec<&str> = chain
            .tool_definitions()
            .iter()
            .map(|t| t.name.as_str())
            .collect();

        // 7 coordination + 1 navigate + 8 local tools = 16
        assert_eq!(chain.tool_definitions().len(), 16);
        assert!(tool_names.contains(&"navigate"));
        assert!(tool_names.contains(&"read_file"));
        assert!(tool_names.contains(&"write_file"));
        assert!(tool_names.contains(&"bash"));
        assert!(tool_names.contains(&"web_fetch"));
    }

    #[test]
    fn test_build_child_chain_coordination_disabled() {
        let config = CoordinationConfig {
            enabled: false,
            ..test_coordination_config()
        };
        let context = Arc::new(CoordinationContext::new(config.storage_dir.clone()));
        let base = Box::new(MockToolExecutor::new(&["navigate"]));

        let definition = test_spawn_definition();
        let local_config = LocalToolsConfig {
            enabled: false,
            ..Default::default()
        };

        let chain = build_child_chain(
            base,
            &definition,
            &SkillsConfig::default(),
            &SkillSet::new(),
            &local_config,
            &config,
            context,
            32_768,
            PermissionPolicy {
                mode: crate::permissions::types::PermissionMode::BypassPermissions,
                ..Default::default()
            },
            HookRegistry::new(),
            30,
        )
        .unwrap();

        let tool_names: Vec<&str> = chain
            .tool_definitions()
            .iter()
            .map(|t| t.name.as_str())
            .collect();

        // No coordination tools, just navigate
        assert_eq!(chain.tool_definitions().len(), 1);
        assert!(tool_names.contains(&"navigate"));
        assert!(!tool_names.contains(&"task_create"));
    }

    #[tokio::test]
    async fn test_box_dyn_tool_executor_impl() {
        let executor: Box<dyn ToolExecutor> =
            Box::new(MockToolExecutor::new(&["navigate", "click"]));

        assert_eq!(executor.tool_definitions().len(), 2);
        assert_eq!(executor.tool_definitions()[0].name, "navigate");

        let result = executor
            .execute_tool("navigate", json!({"url": "https://example.com"}))
            .await
            .unwrap();
        assert_eq!(result.content, "Executed navigate");
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_box_dyn_tool_executor_check_inbox() {
        let executor: Box<dyn ToolExecutor> = Box::new(MockToolExecutor::new(&[]));
        let inbox = executor.check_inbox().await;
        assert!(inbox.is_none());
    }

    #[tokio::test]
    async fn test_build_child_chain_executes_tools() {
        let config = test_coordination_config();
        let context = Arc::new(CoordinationContext::new(config.storage_dir.clone()));
        let base = Box::new(MockToolExecutor::new(&["navigate"]));

        let definition = test_spawn_definition();
        let local_config = LocalToolsConfig {
            enabled: false,
            ..Default::default()
        };

        let chain = build_child_chain(
            base,
            &definition,
            &SkillsConfig::default(),
            &SkillSet::new(),
            &local_config,
            &config,
            context,
            32_768,
            PermissionPolicy {
                mode: crate::permissions::types::PermissionMode::BypassPermissions,
                ..Default::default()
            },
            HookRegistry::new(),
            30,
        )
        .unwrap();

        // Execute a tool through the chain
        let result = chain
            .execute_tool("navigate", json!({"url": "test"}))
            .await
            .unwrap();
        assert_eq!(result.content, "Executed navigate");
        assert!(!result.is_error);

        // Execute a coordination virtual tool
        let result = chain
            .execute_tool(
                "task_create",
                json!({"subject": "Test", "description": "Testing"}),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
    }

    #[test]
    fn test_build_child_chain_invalid_filter_pattern() {
        let config = test_coordination_config();
        let context = Arc::new(CoordinationContext::new(config.storage_dir.clone()));
        let base = Box::new(MockToolExecutor::new(&["navigate"]));

        let definition = SpawnDefinition {
            allowed_tools: vec!["[invalid".to_string()],
            ..test_spawn_definition()
        };
        let local_config = LocalToolsConfig {
            enabled: false,
            ..Default::default()
        };

        let result = build_child_chain(
            base,
            &definition,
            &SkillsConfig::default(),
            &SkillSet::new(),
            &local_config,
            &config,
            context,
            32_768,
            PermissionPolicy {
                mode: crate::permissions::types::PermissionMode::BypassPermissions,
                ..Default::default()
            },
            HookRegistry::new(),
            30,
        );
        assert!(result.is_err());
    }

    // -- Retry helper tests --

    /// Test that connect_with_retry returns an error when the browser binary
    /// is not available (validates all 3 attempts are exhausted).
    #[tokio::test]
    async fn test_connect_with_retry_fails_after_max_attempts() {
        let config = BrowserConfig {
            browser_path: Some("/nonexistent/browser".to_string()),
            ..Default::default()
        };

        let start = std::time::Instant::now();
        let result = connect_with_retry(&config, "test-agent").await;
        let elapsed = start.elapsed();

        // Should fail after all retry attempts
        assert!(result.is_err());

        // Should have waited at least 500ms + 1s = 1.5s for the two retry delays.
        // Use a lower bound to account for timing variance.
        assert!(
            elapsed >= std::time::Duration::from_millis(1400),
            "Expected >= 1.4s of retry delays, but only {elapsed:?} elapsed"
        );
    }

    /// Test that the retry constants are sensible.
    #[test]
    fn test_connect_with_retry_constants() {
        // Mirrors the retry_delays array in connect_with_retry:
        // Some(delay) = attempt followed by sleep, None = final attempt.
        let retry_delays: [Option<std::time::Duration>; 3] = [
            Some(std::time::Duration::from_millis(500)),
            Some(std::time::Duration::from_secs(1)),
            None,
        ];

        // 3 total attempts
        assert_eq!(retry_delays.len(), 3);

        // Last entry must be None (final attempt, no retry after)
        assert!(retry_delays.last().unwrap().is_none());

        // Total max wait time should be 1.5s (reasonable for browser startup)
        let total: std::time::Duration = retry_delays.iter().flatten().sum();
        assert_eq!(total, std::time::Duration::from_millis(1500));
    }

    // --- Security: child containment (S1b) ---
    //
    // A child spawned via `spawn_agent` must never hold more authority than the parent
    // that spawned it. Before these, `build_child_chain` had no permission layer at
    // all, so a Plan-mode parent could spawn a child with unrestricted bash/write_file.

    /// Build a child chain under a given parent policy, with local tools enabled so
    /// bash/write_file are genuinely present in the inner chain.
    fn child_chain_under(policy: PermissionPolicy) -> ChildExecutor {
        let base: Box<dyn ToolExecutor> = Box::new(MockToolExecutor::new(&[]));
        let definition = SpawnDefinition {
            name: "child".to_string(),
            ..test_spawn_definition()
        };
        build_child_chain(
            base,
            &definition,
            &SkillsConfig::default(),
            &SkillSet::new(),
            &LocalToolsConfig {
                enabled: true,
                ..Default::default()
            },
            &CoordinationConfig::default(),
            Arc::new(CoordinationContext::new(std::env::temp_dir())),
            32_768,
            policy,
            HookRegistry::new(),
            30,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn test_child_inherits_plan_mode_and_cannot_write() {
        let chain = child_chain_under(PermissionPolicy {
            mode: PermissionMode::Plan,
            ..Default::default()
        });

        let result = chain
            .execute_tool("write_file", json!({"path": "x.txt", "content": "hi"}))
            .await
            .unwrap();

        assert!(
            result.is_error,
            "a Plan-mode parent must not be able to spawn a child that writes files"
        );
        assert!(
            result.content.contains("Permission denied"),
            "got: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn test_child_inherits_denied_tools() {
        let chain = child_chain_under(PermissionPolicy {
            mode: PermissionMode::Default,
            allowed_tools: vec![],
            denied_tools: vec!["bash".to_string()],
        });

        let result = chain
            .execute_tool("bash", json!({"command": "echo hi"}))
            .await
            .unwrap();

        assert!(result.is_error, "child ignored the parent's denied_tools");
        assert!(
            result.content.contains("Permission denied"),
            "got: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn test_child_coordination_tools_are_permission_checked() {
        // CoordinationExecutor intercepts its virtual tools before delegating inward,
        // so it must sit *inside* the permission layer for them to be checked at all.
        let chain = child_chain_under(PermissionPolicy {
            mode: PermissionMode::Plan,
            ..Default::default()
        });

        let result = chain
            .execute_tool("task_create", json!({"subject": "s", "description": "d"}))
            .await
            .unwrap();

        assert!(
            result.is_error,
            "coordination tools bypassed the permission layer"
        );
    }

    #[test]
    fn test_child_plan_mode_hides_write_tools_from_the_model() {
        let chain = child_chain_under(PermissionPolicy {
            mode: PermissionMode::Plan,
            ..Default::default()
        });
        let names: Vec<&str> = chain
            .tool_definitions()
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert!(!names.contains(&"write_file"), "advertised: {names:?}");
        assert!(!names.contains(&"bash"), "advertised: {names:?}");
        assert!(!names.contains(&"spawn_agent"), "advertised: {names:?}");
    }
}
