use crate::config::schema::CoordinationConfig;

pub const COORDINATION_PROMPT: &str = r#"<coordination_instructions>
You have access to multi-agent coordination tools that allow you to delegate work to parallel child agents. Use these tools to break complex tasks into smaller pieces and execute them concurrently.

## Available Tools

### Task Management
- **task_create** - Create a task on the shared task board with a subject, description, and status
- **task_list** - List all tasks and their current status
- **task_get** - Get full details of a specific task by ID
- **task_update** - Update a task's status, subject, description, or assignment

### Team Management
- **team_create** - Create a named team before spawning worker agents

### Agent Spawning
- **spawn_agent** - Spawn a child worker agent with a specific task, system prompt, and tool restrictions

### Messaging
- **send_message** - Send a direct message to another agent by name for coordination

## Workflow

1. **Create a team** using `team_create` with a descriptive name
2. **Create tasks** on the shared board using `task_create` for each unit of work
3. **Spawn workers** using `spawn_agent`, assigning them specific tasks and restricting their tools as needed
4. Workers **claim tasks** via `task_update`, execute them, and mark them complete
5. Use `send_message` for real-time coordination between agents
6. Monitor progress with `task_list` and `task_get`

## Constraints

- Workers have iteration and timeout limits — keep tasks focused and achievable
- There is a maximum number of concurrent workers — plan accordingly
- Workers can only use tools you explicitly allow when spawning them
- Use the task board as the source of truth for progress tracking
</coordination_instructions>"#;

/// Inject coordination instructions into the system prompt if coordination is enabled.
pub fn inject_coordination_into_system_prompt(config: &CoordinationConfig) -> Option<String> {
    if !config.enabled {
        return None;
    }
    Some(format!(
        "{}\n\n## Current Configuration\n- Maximum concurrent workers: {}\n- Maximum iterations per worker: {}\n- Worker timeout: {} seconds",
        COORDINATION_PROMPT,
        config.max_workers,
        config.max_worker_iterations,
        config.worker_timeout_secs
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_returns_none_when_disabled() {
        let config = CoordinationConfig {
            enabled: false,
            ..Default::default()
        };
        assert!(inject_coordination_into_system_prompt(&config).is_none());
    }

    #[test]
    fn test_returns_some_when_enabled() {
        let config = CoordinationConfig {
            enabled: true,
            ..Default::default()
        };
        assert!(inject_coordination_into_system_prompt(&config).is_some());
    }

    #[test]
    fn test_prompt_contains_xml_tags() {
        let config = CoordinationConfig::default();
        let result = inject_coordination_into_system_prompt(&config).unwrap();
        assert!(result.contains("<coordination_instructions>"));
        assert!(result.contains("</coordination_instructions>"));
    }

    #[test]
    fn test_prompt_contains_tool_names() {
        let config = CoordinationConfig::default();
        let result = inject_coordination_into_system_prompt(&config).unwrap();
        assert!(result.contains("task_create"));
        assert!(result.contains("task_list"));
        assert!(result.contains("task_get"));
        assert!(result.contains("task_update"));
        assert!(result.contains("team_create"));
        assert!(result.contains("spawn_agent"));
        assert!(result.contains("send_message"));
    }

    #[test]
    fn test_prompt_contains_workflow_info() {
        let config = CoordinationConfig::default();
        let result = inject_coordination_into_system_prompt(&config).unwrap();
        assert!(result.contains("Workflow"));
        assert!(result.contains("Create a team"));
        assert!(result.contains("Spawn workers"));
    }

    #[test]
    fn test_prompt_contains_messaging_info() {
        let config = CoordinationConfig::default();
        let result = inject_coordination_into_system_prompt(&config).unwrap();
        assert!(result.contains("send_message"));
        assert!(result.contains("direct message"));
    }

    #[test]
    fn test_prompt_contains_config_values() {
        let config = CoordinationConfig::default();
        let result = inject_coordination_into_system_prompt(&config).unwrap();
        assert!(result.contains("## Current Configuration"));
        assert!(result.contains("Maximum concurrent workers: 5"));
        assert!(result.contains("Maximum iterations per worker: 10"));
        assert!(result.contains("Worker timeout: 120 seconds"));
    }

    #[test]
    fn test_prompt_reflects_custom_config_values() {
        let config = CoordinationConfig {
            enabled: true,
            max_workers: 8,
            max_worker_iterations: 20,
            worker_timeout_secs: 300,
            ..Default::default()
        };
        let result = inject_coordination_into_system_prompt(&config).unwrap();
        assert!(result.contains("Maximum concurrent workers: 8"));
        assert!(result.contains("Maximum iterations per worker: 20"));
        assert!(result.contains("Worker timeout: 300 seconds"));
    }
}
