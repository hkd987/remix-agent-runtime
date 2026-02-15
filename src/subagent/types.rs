use serde::{Deserialize, Serialize};

use crate::output::result::{AgentStatus, StepRecord};
use crate::session::types::SessionId;

/// Definition of a subagent to be spawned.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentDefinition {
    pub name: String,
    pub description: String,
    pub system_prompt: Option<String>,
    pub allowed_tools: Vec<String>,
    pub max_iterations: u32,
    pub timeout_secs: u64,
}

/// Result returned when a subagent completes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentResult {
    pub name: String,
    pub status: AgentStatus,
    pub result: Option<String>,
    pub steps: Vec<StepRecord>,
    pub session_id: Option<SessionId>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_definition() -> SubagentDefinition {
        SubagentDefinition {
            name: "researcher".to_string(),
            description: "Research the topic".to_string(),
            system_prompt: Some("You are a research assistant.".to_string()),
            allowed_tools: vec!["navigate".to_string(), "read_.*".to_string()],
            max_iterations: 10,
            timeout_secs: 120,
        }
    }

    fn sample_definition_minimal() -> SubagentDefinition {
        SubagentDefinition {
            name: "worker".to_string(),
            description: "Do the work".to_string(),
            system_prompt: None,
            allowed_tools: vec![],
            max_iterations: 5,
            timeout_secs: 60,
        }
    }

    fn sample_result() -> SubagentResult {
        SubagentResult {
            name: "researcher".to_string(),
            status: AgentStatus::Success,
            result: Some("Found 3 results".to_string()),
            steps: vec![StepRecord {
                iteration: 1,
                tool: "navigate".to_string(),
                input: json!({"url": "https://example.com"}),
                output: json!({"success": true}),
                duration_ms: 500,
                is_error: None,
            }],
            session_id: Some(SessionId("sub-session-1".to_string())),
        }
    }

    #[test]
    fn test_definition_serde_roundtrip() {
        let def = sample_definition();
        let json_str = serde_json::to_string(&def).unwrap();
        let deserialized: SubagentDefinition = serde_json::from_str(&json_str).unwrap();
        assert_eq!(deserialized.name, "researcher");
        assert_eq!(deserialized.description, "Research the topic");
        assert_eq!(
            deserialized.system_prompt,
            Some("You are a research assistant.".to_string())
        );
        assert_eq!(deserialized.allowed_tools, vec!["navigate", "read_.*"]);
        assert_eq!(deserialized.max_iterations, 10);
        assert_eq!(deserialized.timeout_secs, 120);
    }

    #[test]
    fn test_definition_minimal_serde_roundtrip() {
        let def = sample_definition_minimal();
        let json_str = serde_json::to_string(&def).unwrap();
        let deserialized: SubagentDefinition = serde_json::from_str(&json_str).unwrap();
        assert_eq!(deserialized.name, "worker");
        assert!(deserialized.system_prompt.is_none());
        assert!(deserialized.allowed_tools.is_empty());
        assert_eq!(deserialized.max_iterations, 5);
        assert_eq!(deserialized.timeout_secs, 60);
    }

    #[test]
    fn test_definition_json_structure() {
        let def = sample_definition();
        let json = serde_json::to_value(&def).unwrap();
        assert_eq!(json["name"], "researcher");
        assert_eq!(json["description"], "Research the topic");
        assert_eq!(json["system_prompt"], "You are a research assistant.");
        assert_eq!(json["allowed_tools"][0], "navigate");
        assert_eq!(json["allowed_tools"][1], "read_.*");
        assert_eq!(json["max_iterations"], 10);
        assert_eq!(json["timeout_secs"], 120);
    }

    #[test]
    fn test_definition_null_system_prompt_in_json() {
        let def = sample_definition_minimal();
        let json = serde_json::to_value(&def).unwrap();
        assert!(json["system_prompt"].is_null());
    }

    #[test]
    fn test_definition_from_json() {
        let json = json!({
            "name": "coder",
            "description": "Write some code",
            "system_prompt": null,
            "allowed_tools": ["bash", "write_file"],
            "max_iterations": 15,
            "timeout_secs": 180
        });
        let def: SubagentDefinition = serde_json::from_value(json).unwrap();
        assert_eq!(def.name, "coder");
        assert_eq!(def.description, "Write some code");
        assert!(def.system_prompt.is_none());
        assert_eq!(def.allowed_tools, vec!["bash", "write_file"]);
        assert_eq!(def.max_iterations, 15);
        assert_eq!(def.timeout_secs, 180);
    }

    #[test]
    fn test_definition_clone() {
        let def = sample_definition();
        let cloned = def.clone();
        assert_eq!(cloned.name, def.name);
        assert_eq!(cloned.description, def.description);
        assert_eq!(cloned.system_prompt, def.system_prompt);
        assert_eq!(cloned.allowed_tools, def.allowed_tools);
        assert_eq!(cloned.max_iterations, def.max_iterations);
        assert_eq!(cloned.timeout_secs, def.timeout_secs);
    }

    #[test]
    fn test_definition_debug() {
        let def = sample_definition();
        let debug = format!("{:?}", def);
        assert!(debug.contains("researcher"));
        assert!(debug.contains("Research the topic"));
    }

    #[test]
    fn test_result_serde_roundtrip() {
        let result = sample_result();
        let json_str = serde_json::to_string(&result).unwrap();
        let deserialized: SubagentResult = serde_json::from_str(&json_str).unwrap();
        assert_eq!(deserialized.name, "researcher");
        assert_eq!(deserialized.status, AgentStatus::Success);
        assert_eq!(deserialized.result, Some("Found 3 results".to_string()));
        assert_eq!(deserialized.steps.len(), 1);
        assert_eq!(
            deserialized.session_id,
            Some(SessionId("sub-session-1".to_string()))
        );
    }

    #[test]
    fn test_result_json_structure() {
        let result = sample_result();
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["name"], "researcher");
        assert_eq!(json["status"], "success");
        assert_eq!(json["result"], "Found 3 results");
        assert!(json["steps"].is_array());
        assert_eq!(json["steps"].as_array().unwrap().len(), 1);
        assert_eq!(json["session_id"], "sub-session-1");
    }

    #[test]
    fn test_result_with_error_status() {
        let result = SubagentResult {
            name: "failed-agent".to_string(),
            status: AgentStatus::Error,
            result: None,
            steps: vec![],
            session_id: None,
        };
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["status"], "error");
        assert!(json["result"].is_null());
        assert!(json["session_id"].is_null());
    }

    #[test]
    fn test_result_with_timeout_status() {
        let result = SubagentResult {
            name: "slow-agent".to_string(),
            status: AgentStatus::Timeout,
            result: None,
            steps: vec![],
            session_id: None,
        };
        let json_str = serde_json::to_string(&result).unwrap();
        let deserialized: SubagentResult = serde_json::from_str(&json_str).unwrap();
        assert_eq!(deserialized.status, AgentStatus::Timeout);
    }

    #[test]
    fn test_result_with_max_iterations_status() {
        let result = SubagentResult {
            name: "looping-agent".to_string(),
            status: AgentStatus::MaxIterations,
            result: None,
            steps: vec![],
            session_id: None,
        };
        let json_str = serde_json::to_string(&result).unwrap();
        let deserialized: SubagentResult = serde_json::from_str(&json_str).unwrap();
        assert_eq!(deserialized.status, AgentStatus::MaxIterations);
    }

    #[test]
    fn test_result_clone() {
        let result = sample_result();
        let cloned = result.clone();
        assert_eq!(cloned.name, result.name);
        assert_eq!(cloned.status, result.status);
        assert_eq!(cloned.result, result.result);
        assert_eq!(cloned.steps.len(), result.steps.len());
        assert_eq!(cloned.session_id, result.session_id);
    }

    #[test]
    fn test_result_debug() {
        let result = sample_result();
        let debug = format!("{:?}", result);
        assert!(debug.contains("researcher"));
        assert!(debug.contains("Success"));
    }

    #[test]
    fn test_result_with_multiple_steps() {
        let result = SubagentResult {
            name: "multi-step".to_string(),
            status: AgentStatus::Success,
            result: Some("All done".to_string()),
            steps: vec![
                StepRecord {
                    iteration: 1,
                    tool: "navigate".to_string(),
                    input: json!({"url": "https://example.com"}),
                    output: json!({"success": true}),
                    duration_ms: 500,
                    is_error: None,
                },
                StepRecord {
                    iteration: 2,
                    tool: "click".to_string(),
                    input: json!({"selector": "#btn"}),
                    output: json!({"clicked": true}),
                    duration_ms: 200,
                    is_error: None,
                },
                StepRecord {
                    iteration: 3,
                    tool: "read_file".to_string(),
                    input: json!({"path": "/tmp/out.txt"}),
                    output: json!({"content": "hello"}),
                    duration_ms: 100,
                    is_error: Some(false),
                },
            ],
            session_id: Some(SessionId("multi-session".to_string())),
        };
        let json_str = serde_json::to_string(&result).unwrap();
        let deserialized: SubagentResult = serde_json::from_str(&json_str).unwrap();
        assert_eq!(deserialized.steps.len(), 3);
        assert_eq!(deserialized.steps[0].tool, "navigate");
        assert_eq!(deserialized.steps[1].tool, "click");
        assert_eq!(deserialized.steps[2].tool, "read_file");
    }

    #[test]
    fn test_definition_empty_allowed_tools() {
        let def = SubagentDefinition {
            name: "all-access".to_string(),
            description: "Has access to all tools".to_string(),
            system_prompt: None,
            allowed_tools: vec![],
            max_iterations: 10,
            timeout_secs: 120,
        };
        let json = serde_json::to_value(&def).unwrap();
        assert!(json["allowed_tools"].as_array().unwrap().is_empty());
    }
}
