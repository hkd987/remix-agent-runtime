use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt;
use tokio::task::JoinHandle;

use crate::error::AgentError;
use crate::output::result::AgentResult;

/// Definition of a child agent to be spawned.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnDefinition {
    pub name: String,
    pub task: String,
    pub system_prompt: Option<String>,
    pub allowed_tools: Vec<String>,
    pub max_iterations: u32,
    pub timeout_secs: u64,
}

/// Handle to a running child agent.
pub struct ChildHandle {
    pub name: String,
    pub join_handle: JoinHandle<Result<AgentResult, AgentError>>,
}

impl fmt::Debug for ChildHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChildHandle")
            .field("name", &self.name)
            .field("join_handle", &"<JoinHandle>")
            .finish()
    }
}

/// Trait for spawning child agents.
#[async_trait]
pub trait SpawnHandler: Send + Sync {
    async fn spawn_child(&self, definition: SpawnDefinition) -> Result<ChildHandle, AgentError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spawn_definition_serde_roundtrip() {
        let def = SpawnDefinition {
            name: "worker-1".to_string(),
            task: "Scrape the homepage".to_string(),
            system_prompt: Some("You are a web scraper.".to_string()),
            allowed_tools: vec!["navigate".to_string(), "click".to_string()],
            max_iterations: 10,
            timeout_secs: 120,
        };

        let json = serde_json::to_string(&def).unwrap();
        let deserialized: SpawnDefinition = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.name, "worker-1");
        assert_eq!(deserialized.task, "Scrape the homepage");
        assert_eq!(
            deserialized.system_prompt,
            Some("You are a web scraper.".to_string())
        );
        assert_eq!(deserialized.allowed_tools, vec!["navigate", "click"]);
        assert_eq!(deserialized.max_iterations, 10);
        assert_eq!(deserialized.timeout_secs, 120);
    }

    #[test]
    fn test_spawn_definition_with_defaults() {
        let def = SpawnDefinition {
            name: "minimal".to_string(),
            task: "Do something".to_string(),
            system_prompt: None,
            allowed_tools: vec![],
            max_iterations: 5,
            timeout_secs: 60,
        };

        let json = serde_json::to_string(&def).unwrap();
        let deserialized: SpawnDefinition = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.name, "minimal");
        assert!(deserialized.system_prompt.is_none());
        assert!(deserialized.allowed_tools.is_empty());
    }

    #[test]
    fn test_spawn_definition_with_all_fields() {
        let json = serde_json::json!({
            "name": "researcher",
            "task": "Find pricing information",
            "system_prompt": "You are a research agent specialized in finding pricing.",
            "allowed_tools": ["navigate", "read_page", "grep"],
            "max_iterations": 20,
            "timeout_secs": 300
        });

        let def: SpawnDefinition = serde_json::from_value(json).unwrap();
        assert_eq!(def.name, "researcher");
        assert_eq!(def.task, "Find pricing information");
        assert_eq!(
            def.system_prompt,
            Some("You are a research agent specialized in finding pricing.".to_string())
        );
        assert_eq!(def.allowed_tools.len(), 3);
        assert_eq!(def.max_iterations, 20);
        assert_eq!(def.timeout_secs, 300);
    }

    #[tokio::test]
    async fn test_child_handle_debug() {
        let handle = ChildHandle {
            name: "test-agent".to_string(),
            join_handle: tokio::spawn(async {
                Ok(AgentResult::success("done".to_string(), vec![], 100))
            }),
        };

        let debug_str = format!("{:?}", handle);
        assert!(debug_str.contains("test-agent"));
        assert!(debug_str.contains("<JoinHandle>"));

        handle.join_handle.abort();
    }

    #[test]
    fn test_spawn_definition_json_output_format() {
        let def = SpawnDefinition {
            name: "worker".to_string(),
            task: "task".to_string(),
            system_prompt: None,
            allowed_tools: vec![],
            max_iterations: 10,
            timeout_secs: 120,
        };

        let value = serde_json::to_value(&def).unwrap();
        assert!(value.is_object());
        assert!(value.get("name").is_some());
        assert!(value.get("task").is_some());
        assert!(value.get("system_prompt").is_some()); // serialized as null
        assert!(value.get("allowed_tools").is_some());
        assert!(value.get("max_iterations").is_some());
        assert!(value.get("timeout_secs").is_some());
    }
}
