use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use serde_json::{json, Value};
use tracing::{info, warn};

use crate::browser::mcp::{ToolExecutionResult, ToolExecutor};
use crate::config::schema::CoordinationConfig;
use crate::error::AgentError;
use crate::llm::types::ToolDefinition;
use crate::output::result::AgentResult;

use super::context::CoordinationContext;
use super::inbox_types::{InboxMessage, MessageId, MessageType};
use super::spawn_handler::{ChildHandle, SpawnDefinition, SpawnHandler};
use super::task_types::{TaskCreateParams, TaskUpdateParams};
use super::team_types::{MemberStatus, TeamCreateParams, TeamMember};

/// Decorator that adds coordination virtual tools (task_create, task_list, task_get,
/// task_update, team_create, send_message, spawn_agent) to the tool executor chain.
///
/// Replaces SubagentExecutor at the same position in the decorator chain.
pub struct CoordinationExecutor<T: ToolExecutor> {
    inner: T,
    config: CoordinationConfig,
    context: Arc<CoordinationContext>,
    agent_name: String,
    all_tools: Vec<ToolDefinition>,
    spawn_handler: Option<Arc<dyn SpawnHandler>>,
    children: Arc<tokio::sync::Mutex<Vec<ChildHandle>>>,
}

impl<T: ToolExecutor> CoordinationExecutor<T> {
    pub fn new(
        inner: T,
        config: CoordinationConfig,
        context: Arc<CoordinationContext>,
        agent_name: String,
    ) -> Self {
        let mut all_tools = Vec::new();

        if config.enabled {
            all_tools.extend(Self::virtual_tool_definitions());
        }

        all_tools.extend_from_slice(inner.tool_definitions());

        Self {
            inner,
            config,
            context,
            agent_name,
            all_tools,
            spawn_handler: None,
            children: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        }
    }

    /// Set a spawn handler for real agent spawning.
    pub fn with_spawn_handler(mut self, handler: Arc<dyn SpawnHandler>) -> Self {
        self.spawn_handler = Some(handler);
        self
    }

    pub fn into_inner(self) -> T {
        self.inner
    }

    pub fn config(&self) -> &CoordinationConfig {
        &self.config
    }

    pub fn context(&self) -> &Arc<CoordinationContext> {
        &self.context
    }

    fn virtual_tool_definitions() -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "task_create".to_string(),
                description: "Create a new task on the shared task board.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "subject": {
                            "type": "string",
                            "description": "Brief title for the task"
                        },
                        "description": {
                            "type": "string",
                            "description": "Detailed description of what needs to be done"
                        },
                        "active_form": {
                            "type": "string",
                            "description": "Present continuous form shown in spinner (e.g., 'Running tests')"
                        },
                        "metadata": {
                            "type": "object",
                            "description": "Arbitrary metadata to attach to the task"
                        }
                    },
                    "required": ["subject", "description"]
                }),
                cache_control: None,
                read_only: false,
            },
            ToolDefinition {
                name: "task_list".to_string(),
                description: "List all tasks on the shared task board.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
                cache_control: None,
                read_only: true,
            },
            ToolDefinition {
                name: "task_get".to_string(),
                description: "Get full details of a specific task by ID.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "task_id": {
                            "type": "string",
                            "description": "The ID of the task to retrieve"
                        }
                    },
                    "required": ["task_id"]
                }),
                cache_control: None,
                read_only: true,
            },
            ToolDefinition {
                name: "task_update".to_string(),
                description: "Update an existing task (status, owner, description, etc.)."
                    .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "task_id": {
                            "type": "string",
                            "description": "The ID of the task to update"
                        },
                        "status": {
                            "type": "string",
                            "enum": ["pending", "in_progress", "completed", "deleted"],
                            "description": "New status for the task"
                        },
                        "subject": {
                            "type": "string",
                            "description": "New subject for the task"
                        },
                        "description": {
                            "type": "string",
                            "description": "New description for the task"
                        },
                        "active_form": {
                            "type": "string",
                            "description": "Present continuous form for spinner"
                        },
                        "owner": {
                            "type": "string",
                            "description": "Agent name to assign the task to"
                        },
                        "metadata": {
                            "type": "object",
                            "description": "Metadata keys to merge (null values delete keys)"
                        },
                        "add_blocks": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Task IDs that this task blocks"
                        },
                        "add_blocked_by": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Task IDs that block this task"
                        }
                    },
                    "required": ["task_id"]
                }),
                cache_control: None,
                read_only: false,
            },
            ToolDefinition {
                name: "team_create".to_string(),
                description: "Create a new team for multi-agent coordination.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "team_name": {
                            "type": "string",
                            "description": "Name for the new team"
                        },
                        "description": {
                            "type": "string",
                            "description": "Team description/purpose"
                        }
                    },
                    "required": ["team_name"]
                }),
                cache_control: None,
                read_only: false,
            },
            ToolDefinition {
                name: "send_message".to_string(),
                description: "Send a message to another agent on the team.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "type": {
                            "type": "string",
                            "enum": ["message", "broadcast", "shutdown_request", "shutdown_response"],
                            "description": "Message type"
                        },
                        "recipient": {
                            "type": "string",
                            "description": "Agent name of the recipient (required for message, shutdown_request)"
                        },
                        "content": {
                            "type": "string",
                            "description": "Message content"
                        },
                        "summary": {
                            "type": "string",
                            "description": "Brief summary of the message"
                        },
                        "request_id": {
                            "type": "string",
                            "description": "Request ID to respond to (for shutdown_response)"
                        },
                        "approve": {
                            "type": "boolean",
                            "description": "Whether to approve (for shutdown_response)"
                        }
                    },
                    "required": ["type", "content"]
                }),
                cache_control: None,
                read_only: false,
            },
            crate::subagent::executor::spawn_agent_tool_definition(),
        ]
    }

    fn is_virtual_tool(name: &str) -> bool {
        matches!(
            name,
            "task_create"
                | "task_list"
                | "task_get"
                | "task_update"
                | "team_create"
                | "send_message"
                | "spawn_agent"
        )
    }

    async fn handle_virtual_tool(
        &self,
        name: &str,
        arguments: Value,
    ) -> Result<ToolExecutionResult, AgentError> {
        match name {
            "task_create" => self.handle_task_create(arguments).await,
            "task_list" => self.handle_task_list().await,
            "task_get" => self.handle_task_get(arguments).await,
            "task_update" => self.handle_task_update(arguments).await,
            "team_create" => self.handle_team_create(arguments).await,
            "send_message" => self.handle_send_message(arguments).await,
            "spawn_agent" => self.handle_spawn_agent(arguments).await,
            _ => Ok(ToolExecutionResult {
                content: format!("Unknown virtual tool: {name}"),
                is_error: true,
            }),
        }
    }

    async fn handle_task_create(
        &self,
        arguments: Value,
    ) -> Result<ToolExecutionResult, AgentError> {
        let subject = arguments
            .get("subject")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::Coordination("Missing required field 'subject'".into()))?
            .to_string();

        let description = arguments
            .get("description")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::Coordination("Missing required field 'description'".into()))?
            .to_string();

        let active_form = arguments
            .get("active_form")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let metadata = arguments
            .get("metadata")
            .and_then(|v| v.as_object())
            .map(|obj| obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect());

        let params = TaskCreateParams {
            subject,
            description,
            active_form,
            metadata,
        };

        let task = self.context.task_store.create(params).await?;
        let json = serde_json::to_string_pretty(&task)
            .unwrap_or_else(|e| format!("Failed to serialize: {e}"));

        Ok(ToolExecutionResult {
            content: json,
            is_error: false,
        })
    }

    async fn handle_task_list(&self) -> Result<ToolExecutionResult, AgentError> {
        let summaries = self.context.task_store.list().await;
        let json = serde_json::to_string_pretty(&summaries)
            .unwrap_or_else(|e| format!("Failed to serialize: {e}"));

        Ok(ToolExecutionResult {
            content: json,
            is_error: false,
        })
    }

    async fn handle_task_get(&self, arguments: Value) -> Result<ToolExecutionResult, AgentError> {
        let task_id = arguments
            .get("task_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::Coordination("Missing required field 'task_id'".into()))?;

        match self.context.task_store.get(task_id).await {
            Ok(task) => {
                let json = serde_json::to_string_pretty(&task)
                    .unwrap_or_else(|e| format!("Failed to serialize: {e}"));
                Ok(ToolExecutionResult {
                    content: json,
                    is_error: false,
                })
            }
            Err(e) => Ok(ToolExecutionResult {
                content: e.to_string(),
                is_error: true,
            }),
        }
    }

    async fn handle_task_update(
        &self,
        arguments: Value,
    ) -> Result<ToolExecutionResult, AgentError> {
        let task_id = arguments
            .get("task_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::Coordination("Missing required field 'task_id'".into()))?
            .to_string();

        let status = arguments.get("status").and_then(|v| {
            v.as_str()
                .and_then(|s| serde_json::from_value(json!(s)).ok())
        });

        let subject = arguments
            .get("subject")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let description = arguments
            .get("description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let active_form = arguments
            .get("active_form")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let owner = arguments
            .get("owner")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let metadata = arguments
            .get("metadata")
            .and_then(|v| v.as_object())
            .map(|obj| obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect());

        let add_blocks = arguments.get("add_blocks").and_then(|v| {
            v.as_array().map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
        });

        let add_blocked_by = arguments.get("add_blocked_by").and_then(|v| {
            v.as_array().map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
        });

        let params = TaskUpdateParams {
            task_id,
            status,
            subject,
            description,
            active_form,
            owner,
            metadata,
            add_blocks,
            add_blocked_by,
        };

        match self.context.task_store.update(params).await {
            Ok(task) => {
                let json = serde_json::to_string_pretty(&task)
                    .unwrap_or_else(|e| format!("Failed to serialize: {e}"));
                Ok(ToolExecutionResult {
                    content: json,
                    is_error: false,
                })
            }
            Err(e) => Ok(ToolExecutionResult {
                content: e.to_string(),
                is_error: true,
            }),
        }
    }

    async fn handle_team_create(
        &self,
        arguments: Value,
    ) -> Result<ToolExecutionResult, AgentError> {
        let team_name = arguments
            .get("team_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::Coordination("Missing required field 'team_name'".into()))?
            .to_string();

        let description = arguments
            .get("description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let params = TeamCreateParams {
            team_name,
            description,
        };

        match self.context.team_store.create(params).await {
            Ok(team) => {
                // Also add the creator as a team member
                let member = TeamMember {
                    name: self.agent_name.clone(),
                    agent_type: Some("lead".to_string()),
                    status: MemberStatus::Active,
                };
                if let Err(e) = self.context.team_store.add_member(member).await {
                    warn!(error = %e, "Failed to add creator as team member");
                }

                let json = serde_json::to_string_pretty(&team)
                    .unwrap_or_else(|e| format!("Failed to serialize: {e}"));
                Ok(ToolExecutionResult {
                    content: json,
                    is_error: false,
                })
            }
            Err(e) => Ok(ToolExecutionResult {
                content: e.to_string(),
                is_error: true,
            }),
        }
    }

    async fn handle_send_message(
        &self,
        arguments: Value,
    ) -> Result<ToolExecutionResult, AgentError> {
        let msg_type_str = arguments
            .get("type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::Coordination("Missing required field 'type'".into()))?;

        let message_type: MessageType =
            serde_json::from_value(json!(msg_type_str)).map_err(|_| {
                AgentError::Coordination(format!("Invalid message type: {msg_type_str}"))
            })?;

        let content = arguments
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::Coordination("Missing required field 'content'".into()))?
            .to_string();

        let recipient = arguments
            .get("recipient")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let summary = arguments
            .get("summary")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let request_id = arguments
            .get("request_id")
            .and_then(|v| v.as_str())
            .map(|s| MessageId(s.to_string()));

        let approve = arguments.get("approve").and_then(|v| v.as_bool());

        let message = InboxMessage {
            id: MessageId::new(),
            message_type: message_type.clone(),
            sender: self.agent_name.clone(),
            recipient: recipient.clone(),
            content,
            summary,
            timestamp: Utc::now(),
            request_id,
            approve,
        };

        match message_type {
            MessageType::Broadcast => {
                // Get all team members except sender
                let recipients: Vec<String> =
                    if let Some(team) = self.context.team_store.get().await {
                        team.members
                            .iter()
                            .filter(|m| m.name != self.agent_name)
                            .map(|m| m.name.clone())
                            .collect()
                    } else {
                        return Ok(ToolExecutionResult {
                            content: "No team exists to broadcast to".to_string(),
                            is_error: true,
                        });
                    };

                self.context
                    .inbox_store
                    .broadcast(message, &recipients)
                    .await?;

                Ok(ToolExecutionResult {
                    content: format!("Broadcast sent to {} recipients", recipients.len()),
                    is_error: false,
                })
            }
            MessageType::Message | MessageType::ShutdownRequest | MessageType::ShutdownResponse => {
                let recipient = recipient.ok_or_else(|| {
                    AgentError::Coordination(
                        "Recipient is required for message/shutdown types".into(),
                    )
                })?;

                self.context
                    .inbox_store
                    .deliver(&recipient, message)
                    .await?;

                info!(
                    sender = %self.agent_name,
                    recipient = %recipient,
                    "Message delivered"
                );

                Ok(ToolExecutionResult {
                    content: format!("Message sent to {recipient}"),
                    is_error: false,
                })
            }
        }
    }

    async fn handle_spawn_agent(
        &self,
        arguments: Value,
    ) -> Result<ToolExecutionResult, AgentError> {
        let name = arguments
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::Coordination("Missing required field 'name'".into()))?
            .to_string();

        let task = arguments
            .get("task")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::Coordination("Missing required field 'task'".into()))?
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
            .unwrap_or(self.config.max_worker_iterations);

        // Add as team member
        let member = TeamMember {
            name: name.clone(),
            agent_type: Some("worker".to_string()),
            status: MemberStatus::Active,
        };
        if let Err(e) = self.context.team_store.add_member(member).await {
            warn!(error = %e, "Failed to add spawned agent as team member");
        }

        let spawn_def = SpawnDefinition {
            name: name.clone(),
            task: task.clone(),
            system_prompt,
            allowed_tools,
            max_iterations,
            timeout_secs: self.config.worker_timeout_secs,
        };

        // Check max_workers limit before spawning
        if self.spawn_handler.is_some() {
            let children_count = self.children.lock().await.len();
            if children_count >= self.config.max_workers as usize {
                return Ok(ToolExecutionResult {
                    content: format!(
                        "Cannot spawn agent '{}': maximum concurrent workers limit ({}) reached. {} agents currently running.",
                        name, self.config.max_workers, children_count
                    ),
                    is_error: true,
                });
            }
        }

        if let Some(ref handler) = self.spawn_handler {
            // Actually spawn the child agent
            let child = handler.spawn_child(spawn_def).await?;

            info!(agent = %name, task = %task, "Agent spawned successfully");

            let mut children = self.children.lock().await;
            children.push(child);

            Ok(ToolExecutionResult {
                content: format!("Agent '{name}' spawned successfully with task: {task}"),
                is_error: false,
            })
        } else {
            // No handler - return JSON definition (backward compatible)
            let spawn_definition = json!({
                "name": spawn_def.name,
                "task": spawn_def.task,
                "system_prompt": spawn_def.system_prompt,
                "allowed_tools": spawn_def.allowed_tools,
                "max_iterations": spawn_def.max_iterations,
                "timeout_secs": spawn_def.timeout_secs,
                "coordination_enabled": true,
            });

            info!(
                agent = %name,
                task = %task,
                "Agent spawn requested"
            );

            let json = serde_json::to_string_pretty(&spawn_definition)
                .unwrap_or_else(|e| format!("Failed to serialize: {e}"));

            Ok(ToolExecutionResult {
                content: json,
                is_error: false,
            })
        }
    }

    /// Wait for all child agents to complete, with a timeout.
    /// Returns a list of (name, result) pairs.
    pub async fn wait_for_children(
        &self,
        timeout: std::time::Duration,
    ) -> Vec<(String, Result<AgentResult, AgentError>)> {
        let mut children = self.children.lock().await;
        let mut results = Vec::new();

        for child in children.drain(..) {
            let name = child.name.clone();
            let abort_handle = child.join_handle.abort_handle();
            match tokio::time::timeout(timeout, child.join_handle).await {
                Ok(Ok(result)) => {
                    results.push((name, result));
                }
                Ok(Err(join_error)) => {
                    results.push((
                        name,
                        Err(AgentError::Coordination(format!(
                            "Child agent panicked: {join_error}"
                        ))),
                    ));
                }
                Err(_) => {
                    abort_handle.abort();
                    results.push((
                        name,
                        Err(AgentError::Coordination(
                            "Child agent timed out".to_string(),
                        )),
                    ));
                }
            }
        }

        results
    }
}

#[async_trait]
impl<T: ToolExecutor> ToolExecutor for CoordinationExecutor<T> {
    fn tool_definitions(&self) -> &[ToolDefinition] {
        &self.all_tools
    }

    async fn execute_tool(
        &self,
        name: &str,
        arguments: Value,
    ) -> Result<ToolExecutionResult, AgentError> {
        if self.config.enabled && Self::is_virtual_tool(name) {
            self.handle_virtual_tool(name, arguments).await
        } else if Self::is_virtual_tool(name) {
            Ok(ToolExecutionResult {
                content: "Coordination is disabled".to_string(),
                is_error: true,
            })
        } else {
            self.inner.execute_tool(name, arguments).await
        }
    }

    fn shutdown(self: Box<Self>) {
        Box::new(self.inner).shutdown();
    }

    async fn check_inbox(&self) -> Option<Vec<String>> {
        if !self.config.enabled {
            return None;
        }
        let messages = self
            .context
            .inbox_store
            .drain(&self.agent_name)
            .await
            .unwrap_or_default();
        if messages.is_empty() {
            return None;
        }
        Some(
            messages
                .into_iter()
                .map(|msg| {
                    format!(
                        "From: {}\nType: {:?}\n{}",
                        msg.sender, msg.message_type, msg.content
                    )
                })
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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

    fn test_config() -> CoordinationConfig {
        CoordinationConfig {
            enabled: true,
            max_worker_iterations: 10,
            worker_timeout_secs: 120,
            max_workers: 5,
            storage_dir: tempfile::TempDir::new().unwrap().keep(),
        }
    }

    fn disabled_config() -> CoordinationConfig {
        CoordinationConfig {
            enabled: false,
            ..test_config()
        }
    }

    fn make_executor(
        tool_names: &[&str],
        config: CoordinationConfig,
    ) -> CoordinationExecutor<MockExecutor> {
        let context = Arc::new(CoordinationContext::new(config.storage_dir.clone()));
        CoordinationExecutor::new(
            MockExecutor::new(tool_names),
            config,
            context,
            "lead".to_string(),
        )
    }

    #[test]
    fn test_tool_definitions_includes_virtual_tools_when_enabled() {
        let exec = make_executor(&["navigate", "click"], test_config());
        let tools = exec.tool_definitions();
        // 7 virtual + 2 inner
        assert_eq!(tools.len(), 9);
        assert_eq!(tools[0].name, "task_create");
        assert_eq!(tools[1].name, "task_list");
        assert_eq!(tools[2].name, "task_get");
        assert_eq!(tools[3].name, "task_update");
        assert_eq!(tools[4].name, "team_create");
        assert_eq!(tools[5].name, "send_message");
        assert_eq!(tools[6].name, "spawn_agent");
        assert_eq!(tools[7].name, "navigate");
        assert_eq!(tools[8].name, "click");
    }

    #[test]
    fn test_tool_definitions_excludes_virtual_tools_when_disabled() {
        let exec = make_executor(&["navigate", "click"], disabled_config());
        let tools = exec.tool_definitions();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "navigate");
        assert_eq!(tools[1].name, "click");
    }

    #[tokio::test]
    async fn test_delegates_non_virtual_tools() {
        let exec = make_executor(&["navigate"], test_config());
        let result = exec
            .execute_tool("navigate", json!({"url": "https://example.com"}))
            .await
            .unwrap();
        assert_eq!(result.content, "Executed navigate");
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_task_create() {
        let exec = make_executor(&[], test_config());
        let result = exec
            .execute_tool(
                "task_create",
                json!({
                    "subject": "Test task",
                    "description": "Do something"
                }),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        let task: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(task["subject"], "Test task");
        assert_eq!(task["description"], "Do something");
        assert_eq!(task["status"], "pending");
        assert_eq!(task["id"], "1");
    }

    #[tokio::test]
    async fn test_task_list() {
        let exec = make_executor(&[], test_config());
        // Create a task first
        exec.execute_tool(
            "task_create",
            json!({
                "subject": "Task 1",
                "description": "First task"
            }),
        )
        .await
        .unwrap();

        let result = exec.execute_tool("task_list", json!({})).await.unwrap();
        assert!(!result.is_error);
        let tasks: Vec<serde_json::Value> = serde_json::from_str(&result.content).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0]["subject"], "Task 1");
    }

    #[tokio::test]
    async fn test_task_get() {
        let exec = make_executor(&[], test_config());
        exec.execute_tool(
            "task_create",
            json!({
                "subject": "Task 1",
                "description": "First task"
            }),
        )
        .await
        .unwrap();

        let result = exec
            .execute_tool("task_get", json!({"task_id": "1"}))
            .await
            .unwrap();
        assert!(!result.is_error);
        let task: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(task["subject"], "Task 1");
    }

    #[tokio::test]
    async fn test_task_get_nonexistent() {
        let exec = make_executor(&[], test_config());
        let result = exec
            .execute_tool("task_get", json!({"task_id": "999"}))
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("not found"));
    }

    #[tokio::test]
    async fn test_task_update() {
        let exec = make_executor(&[], test_config());
        exec.execute_tool(
            "task_create",
            json!({
                "subject": "Task 1",
                "description": "First task"
            }),
        )
        .await
        .unwrap();

        let result = exec
            .execute_tool(
                "task_update",
                json!({
                    "task_id": "1",
                    "status": "in_progress",
                    "owner": "worker-1"
                }),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        let task: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(task["status"], "in_progress");
        assert_eq!(task["owner"], "worker-1");
    }

    #[tokio::test]
    async fn test_team_create() {
        let exec = make_executor(&[], test_config());
        let result = exec
            .execute_tool(
                "team_create",
                json!({
                    "team_name": "test-team",
                    "description": "A test team"
                }),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        let team: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(team["name"], "test-team");
    }

    #[tokio::test]
    async fn test_send_message() {
        let config = test_config();
        let context = Arc::new(CoordinationContext::new(config.storage_dir.clone()));

        // Create team and add recipient
        let exec = CoordinationExecutor::new(
            MockExecutor::new(&[]),
            config,
            context.clone(),
            "lead".to_string(),
        );

        exec.execute_tool("team_create", json!({"team_name": "test-team"}))
            .await
            .unwrap();

        // Add a worker member
        let member = TeamMember {
            name: "worker".to_string(),
            agent_type: Some("worker".to_string()),
            status: MemberStatus::Active,
        };
        context.team_store.add_member(member).await.unwrap();

        let result = exec
            .execute_tool(
                "send_message",
                json!({
                    "type": "message",
                    "recipient": "worker",
                    "content": "Hello worker!",
                    "summary": "Greeting"
                }),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("worker"));

        // Verify message was delivered
        let messages = context.inbox_store.drain("worker").await.unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "Hello worker!");
        assert_eq!(messages[0].sender, "lead");
    }

    #[tokio::test]
    async fn test_send_broadcast() {
        let config = test_config();
        let context = Arc::new(CoordinationContext::new(config.storage_dir.clone()));
        let exec = CoordinationExecutor::new(
            MockExecutor::new(&[]),
            config,
            context.clone(),
            "lead".to_string(),
        );

        // Create team
        exec.execute_tool("team_create", json!({"team_name": "test-team"}))
            .await
            .unwrap();

        // Add workers
        for name in &["worker1", "worker2"] {
            let member = TeamMember {
                name: name.to_string(),
                agent_type: Some("worker".to_string()),
                status: MemberStatus::Active,
            };
            context.team_store.add_member(member).await.unwrap();
        }

        let result = exec
            .execute_tool(
                "send_message",
                json!({
                    "type": "broadcast",
                    "content": "Hello everyone!",
                    "summary": "Broadcast"
                }),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("2 recipients"));

        // Both workers should have the message
        let msgs1 = context.inbox_store.drain("worker1").await.unwrap();
        let msgs2 = context.inbox_store.drain("worker2").await.unwrap();
        assert_eq!(msgs1.len(), 1);
        assert_eq!(msgs2.len(), 1);
    }

    #[tokio::test]
    async fn test_spawn_agent() {
        let exec = make_executor(&[], test_config());

        // Create team first
        exec.execute_tool("team_create", json!({"team_name": "test-team"}))
            .await
            .unwrap();

        let result = exec
            .execute_tool(
                "spawn_agent",
                json!({
                    "name": "researcher",
                    "task": "Find the answer",
                    "allowed_tools": ["navigate"],
                    "max_iterations": 5
                }),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        let def: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(def["name"], "researcher");
        assert_eq!(def["task"], "Find the answer");
        assert_eq!(def["max_iterations"], 5);
        assert_eq!(def["coordination_enabled"], true);
    }

    #[tokio::test]
    async fn test_spawn_agent_missing_name() {
        let exec = make_executor(&[], test_config());
        let result = exec
            .execute_tool("spawn_agent", json!({"task": "Do something"}))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_check_inbox_returns_messages() {
        let config = test_config();
        let context = Arc::new(CoordinationContext::new(config.storage_dir.clone()));
        let exec = CoordinationExecutor::new(
            MockExecutor::new(&[]),
            config,
            context.clone(),
            "lead".to_string(),
        );

        // Deliver a message to "lead"
        let msg = InboxMessage {
            id: MessageId::new(),
            message_type: MessageType::Message,
            sender: "worker".to_string(),
            recipient: Some("lead".to_string()),
            content: "Task complete!".to_string(),
            summary: Some("Done".to_string()),
            timestamp: Utc::now(),
            request_id: None,
            approve: None,
        };
        context.inbox_store.deliver("lead", msg).await.unwrap();

        let inbox = exec.check_inbox().await;
        assert!(inbox.is_some());
        let messages = inbox.unwrap();
        assert_eq!(messages.len(), 1);
        assert!(messages[0].contains("worker"));
        assert!(messages[0].contains("Task complete!"));

        // Second call should be empty (drained)
        let inbox2 = exec.check_inbox().await;
        assert!(inbox2.is_none());
    }

    #[tokio::test]
    async fn test_check_inbox_disabled() {
        let exec = make_executor(&[], disabled_config());
        let inbox = exec.check_inbox().await;
        assert!(inbox.is_none());
    }

    #[tokio::test]
    async fn test_virtual_tool_disabled_returns_error() {
        let exec = make_executor(&[], disabled_config());
        let result = exec
            .execute_tool(
                "task_create",
                json!({"subject": "test", "description": "test"}),
            )
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("disabled"));
    }

    #[test]
    fn test_is_virtual_tool() {
        assert!(CoordinationExecutor::<MockExecutor>::is_virtual_tool(
            "task_create"
        ));
        assert!(CoordinationExecutor::<MockExecutor>::is_virtual_tool(
            "task_list"
        ));
        assert!(CoordinationExecutor::<MockExecutor>::is_virtual_tool(
            "task_get"
        ));
        assert!(CoordinationExecutor::<MockExecutor>::is_virtual_tool(
            "task_update"
        ));
        assert!(CoordinationExecutor::<MockExecutor>::is_virtual_tool(
            "team_create"
        ));
        assert!(CoordinationExecutor::<MockExecutor>::is_virtual_tool(
            "send_message"
        ));
        assert!(CoordinationExecutor::<MockExecutor>::is_virtual_tool(
            "spawn_agent"
        ));
        assert!(!CoordinationExecutor::<MockExecutor>::is_virtual_tool(
            "navigate"
        ));
        assert!(!CoordinationExecutor::<MockExecutor>::is_virtual_tool(
            "unknown"
        ));
    }

    #[test]
    fn test_config_accessor() {
        let config = test_config();
        let exec = make_executor(&[], config.clone());
        assert!(exec.config().enabled);
        assert_eq!(exec.config().max_workers, 5);
    }

    #[test]
    fn test_into_inner() {
        let exec = make_executor(&["navigate"], test_config());
        let inner = exec.into_inner();
        assert_eq!(inner.tool_definitions().len(), 1);
        assert_eq!(inner.tool_definitions()[0].name, "navigate");
    }

    #[tokio::test]
    async fn test_task_create_missing_subject() {
        let exec = make_executor(&[], test_config());
        let result = exec
            .execute_tool("task_create", json!({"description": "no subject"}))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_send_message_missing_recipient() {
        let config = test_config();
        let context = Arc::new(CoordinationContext::new(config.storage_dir.clone()));
        let exec =
            CoordinationExecutor::new(MockExecutor::new(&[]), config, context, "lead".to_string());

        let result = exec
            .execute_tool(
                "send_message",
                json!({
                    "type": "message",
                    "content": "Hello!"
                }),
            )
            .await;
        assert!(result.is_err());
    }

    // --- SpawnHandler integration tests ---

    struct MockSpawnHandler {
        spawn_count: Arc<tokio::sync::Mutex<u32>>,
    }

    impl MockSpawnHandler {
        fn new() -> Self {
            Self {
                spawn_count: Arc::new(tokio::sync::Mutex::new(0)),
            }
        }
    }

    #[async_trait]
    impl SpawnHandler for MockSpawnHandler {
        async fn spawn_child(
            &self,
            definition: SpawnDefinition,
        ) -> Result<ChildHandle, AgentError> {
            let mut count = self.spawn_count.lock().await;
            *count += 1;
            let name = definition.name.clone();
            let task = definition.task.clone();
            let join_handle = tokio::spawn(async move {
                Ok(AgentResult::success(
                    format!("Completed: {task}"),
                    vec![],
                    100,
                ))
            });
            Ok(ChildHandle { name, join_handle })
        }
    }

    #[test]
    fn test_with_spawn_handler_builder() {
        let config = test_config();
        let context = Arc::new(CoordinationContext::new(config.storage_dir.clone()));
        let handler = Arc::new(MockSpawnHandler::new());

        let exec =
            CoordinationExecutor::new(MockExecutor::new(&[]), config, context, "lead".to_string())
                .with_spawn_handler(handler);

        assert!(exec.spawn_handler.is_some());
    }

    #[test]
    fn test_without_spawn_handler_default() {
        let exec = make_executor(&[], test_config());
        assert!(exec.spawn_handler.is_none());
    }

    #[tokio::test]
    async fn test_spawn_agent_with_handler_calls_handler() {
        let config = test_config();
        let context = Arc::new(CoordinationContext::new(config.storage_dir.clone()));
        let handler = Arc::new(MockSpawnHandler::new());
        let handler_clone = handler.clone();

        let exec =
            CoordinationExecutor::new(MockExecutor::new(&[]), config, context, "lead".to_string())
                .with_spawn_handler(handler);

        // Create team first
        exec.execute_tool("team_create", json!({"team_name": "test-team"}))
            .await
            .unwrap();

        let result = exec
            .execute_tool(
                "spawn_agent",
                json!({
                    "name": "worker-1",
                    "task": "Do research"
                }),
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("worker-1"));
        assert!(result.content.contains("spawned successfully"));

        // Verify handler was called
        let count = handler_clone.spawn_count.lock().await;
        assert_eq!(*count, 1);

        // Verify child was stored
        let children = exec.children.lock().await;
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].name, "worker-1");
    }

    #[tokio::test]
    async fn test_spawn_agent_without_handler_returns_json() {
        let exec = make_executor(&[], test_config());

        // Create team first
        exec.execute_tool("team_create", json!({"team_name": "test-team"}))
            .await
            .unwrap();

        let result = exec
            .execute_tool(
                "spawn_agent",
                json!({
                    "name": "researcher",
                    "task": "Find the answer",
                    "allowed_tools": ["navigate"],
                    "max_iterations": 5
                }),
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        // Should be parseable JSON with coordination_enabled
        let def: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(def["name"], "researcher");
        assert_eq!(def["task"], "Find the answer");
        assert_eq!(def["max_iterations"], 5);
        assert_eq!(def["coordination_enabled"], true);
    }

    #[tokio::test]
    async fn test_wait_for_children_empty() {
        let exec = make_executor(&[], test_config());
        let results = exec
            .wait_for_children(std::time::Duration::from_secs(5))
            .await;
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_wait_for_children_collects_results() {
        let config = test_config();
        let context = Arc::new(CoordinationContext::new(config.storage_dir.clone()));
        let handler = Arc::new(MockSpawnHandler::new());

        let exec =
            CoordinationExecutor::new(MockExecutor::new(&[]), config, context, "lead".to_string())
                .with_spawn_handler(handler);

        // Create team
        exec.execute_tool("team_create", json!({"team_name": "test-team"}))
            .await
            .unwrap();

        // Spawn two agents
        exec.execute_tool("spawn_agent", json!({"name": "agent-1", "task": "Task A"}))
            .await
            .unwrap();

        exec.execute_tool("spawn_agent", json!({"name": "agent-2", "task": "Task B"}))
            .await
            .unwrap();

        // Wait for results
        let results = exec
            .wait_for_children(std::time::Duration::from_secs(5))
            .await;
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "agent-1");
        assert_eq!(results[1].0, "agent-2");

        // Both should be successful
        let result_a = results[0].1.as_ref().unwrap();
        assert_eq!(result_a.result, Some("Completed: Task A".to_string()));

        let result_b = results[1].1.as_ref().unwrap();
        assert_eq!(result_b.result, Some("Completed: Task B".to_string()));

        // Children should be drained
        let children = exec.children.lock().await;
        assert!(children.is_empty());
    }

    #[tokio::test]
    async fn test_wait_for_children_timeout() {
        let exec = make_executor(&[], test_config());

        // Manually insert a child that will take forever
        {
            let mut children = exec.children.lock().await;
            let join_handle = tokio::spawn(async {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                Ok(AgentResult::success("done".to_string(), vec![], 100))
            });
            children.push(ChildHandle {
                name: "slow-agent".to_string(),
                join_handle,
            });
        }

        // Use a very short timeout
        let results = exec
            .wait_for_children(std::time::Duration::from_millis(10))
            .await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "slow-agent");
        assert!(results[0].1.is_err());
        let err = results[0].1.as_ref().unwrap_err();
        assert!(err.to_string().contains("timed out"));
    }

    #[tokio::test]
    async fn test_spawn_agent_max_workers_exceeded() {
        let mut config = test_config();
        config.max_workers = 1;
        let context = Arc::new(CoordinationContext::new(config.storage_dir.clone()));
        let handler = Arc::new(MockSpawnHandler::new());

        let exec =
            CoordinationExecutor::new(MockExecutor::new(&[]), config, context, "lead".to_string())
                .with_spawn_handler(handler);

        // Create team first
        exec.execute_tool("team_create", json!({"team_name": "test-team"}))
            .await
            .unwrap();

        // First spawn should succeed
        let result = exec
            .execute_tool("spawn_agent", json!({"name": "worker-1", "task": "Task A"}))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("spawned successfully"));

        // Second spawn should be rejected (max_workers=1)
        let result = exec
            .execute_tool("spawn_agent", json!({"name": "worker-2", "task": "Task B"}))
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("maximum concurrent workers limit"));
        assert!(result.content.contains("(1)"));
    }

    #[tokio::test]
    async fn test_wait_for_children_aborts_on_timeout() {
        let exec = make_executor(&[], test_config());

        // Manually insert a child that sleeps for a long time
        let join_handle = tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            Ok(AgentResult::success("done".to_string(), vec![], 100))
        });
        {
            let mut children = exec.children.lock().await;
            children.push(ChildHandle {
                name: "long-running-agent".to_string(),
                join_handle,
            });
        }

        // Use a very short timeout (100ms)
        let results = exec
            .wait_for_children(std::time::Duration::from_millis(100))
            .await;

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "long-running-agent");
        assert!(results[0].1.is_err());
        let err = results[0].1.as_ref().unwrap_err();
        assert!(err.to_string().contains("timed out"));
    }

    struct FailingSpawnHandler;

    #[async_trait]
    impl SpawnHandler for FailingSpawnHandler {
        async fn spawn_child(
            &self,
            _definition: SpawnDefinition,
        ) -> Result<ChildHandle, AgentError> {
            Err(AgentError::Coordination(
                "Mock spawn failure: browser not available".to_string(),
            ))
        }
    }

    #[tokio::test]
    async fn test_spawn_agent_handler_error_propagation() {
        let config = test_config();
        let context = Arc::new(CoordinationContext::new(config.storage_dir.clone()));
        let handler: Arc<dyn SpawnHandler> = Arc::new(FailingSpawnHandler);

        let exec =
            CoordinationExecutor::new(MockExecutor::new(&[]), config, context, "lead".to_string())
                .with_spawn_handler(handler);

        // Create team first
        exec.execute_tool("team_create", json!({"team_name": "test-team"}))
            .await
            .unwrap();

        // Spawn should propagate the error from the handler
        let result = exec
            .execute_tool(
                "spawn_agent",
                json!({"name": "worker-1", "task": "Do work"}),
            )
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("browser not available"),
            "Expected handler error to propagate, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_wait_for_children_multiple_mixed_results() {
        let exec = make_executor(&[], test_config());

        {
            let mut children = exec.children.lock().await;

            // Child 1: succeeds immediately
            let handle1 = tokio::spawn(async {
                Ok(AgentResult::success("Task A done".to_string(), vec![], 50))
            });
            children.push(ChildHandle {
                name: "agent-ok".to_string(),
                join_handle: handle1,
            });

            // Child 2: panics
            let handle2 = tokio::spawn(async {
                panic!("unexpected failure");
                #[allow(unreachable_code)]
                Ok(AgentResult::success("never".to_string(), vec![], 0))
            });
            children.push(ChildHandle {
                name: "agent-panic".to_string(),
                join_handle: handle2,
            });

            // Child 3: returns an error result
            let handle3 =
                tokio::spawn(async { Err(AgentError::Coordination("subtask failed".to_string())) });
            children.push(ChildHandle {
                name: "agent-err".to_string(),
                join_handle: handle3,
            });
        }

        // Give panicking task a moment to complete
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let results = exec
            .wait_for_children(std::time::Duration::from_secs(5))
            .await;

        assert_eq!(results.len(), 3);

        // First child: success
        assert_eq!(results[0].0, "agent-ok");
        assert!(results[0].1.is_ok());
        assert_eq!(
            results[0].1.as_ref().unwrap().result,
            Some("Task A done".to_string())
        );

        // Second child: panicked (JoinError)
        assert_eq!(results[1].0, "agent-panic");
        assert!(results[1].1.is_err());
        assert!(results[1]
            .1
            .as_ref()
            .unwrap_err()
            .to_string()
            .contains("panicked"));

        // Third child: returned an error
        assert_eq!(results[2].0, "agent-err");
        assert!(results[2].1.is_err());
        assert!(results[2]
            .1
            .as_ref()
            .unwrap_err()
            .to_string()
            .contains("subtask failed"));
    }

    #[tokio::test]
    async fn test_spawn_agent_increments_children_count() {
        let config = test_config();
        let context = Arc::new(CoordinationContext::new(config.storage_dir.clone()));
        let handler = Arc::new(MockSpawnHandler::new());

        let exec =
            CoordinationExecutor::new(MockExecutor::new(&[]), config, context, "lead".to_string())
                .with_spawn_handler(handler);

        // Create team first
        exec.execute_tool("team_create", json!({"team_name": "test-team"}))
            .await
            .unwrap();

        // Initially no children
        assert_eq!(exec.children.lock().await.len(), 0);

        // Spawn first child
        exec.execute_tool("spawn_agent", json!({"name": "w1", "task": "A"}))
            .await
            .unwrap();
        assert_eq!(exec.children.lock().await.len(), 1);

        // Spawn second child
        exec.execute_tool("spawn_agent", json!({"name": "w2", "task": "B"}))
            .await
            .unwrap();
        assert_eq!(exec.children.lock().await.len(), 2);

        // Spawn third child
        exec.execute_tool("spawn_agent", json!({"name": "w3", "task": "C"}))
            .await
            .unwrap();
        assert_eq!(exec.children.lock().await.len(), 3);
    }

    #[tokio::test]
    async fn test_wait_for_children_drains_children() {
        let config = test_config();
        let context = Arc::new(CoordinationContext::new(config.storage_dir.clone()));
        let handler = Arc::new(MockSpawnHandler::new());

        let exec =
            CoordinationExecutor::new(MockExecutor::new(&[]), config, context, "lead".to_string())
                .with_spawn_handler(handler);

        // Create team first
        exec.execute_tool("team_create", json!({"team_name": "test-team"}))
            .await
            .unwrap();

        // Spawn two children
        exec.execute_tool("spawn_agent", json!({"name": "w1", "task": "A"}))
            .await
            .unwrap();
        exec.execute_tool("spawn_agent", json!({"name": "w2", "task": "B"}))
            .await
            .unwrap();
        assert_eq!(exec.children.lock().await.len(), 2);

        // Wait for children -- should drain them all
        let results = exec
            .wait_for_children(std::time::Duration::from_secs(5))
            .await;
        assert_eq!(results.len(), 2);

        // After wait, children vec should be empty
        assert_eq!(exec.children.lock().await.len(), 0);

        // A second wait should return no results
        let results2 = exec
            .wait_for_children(std::time::Duration::from_secs(1))
            .await;
        assert!(results2.is_empty());
    }
}
