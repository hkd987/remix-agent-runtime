use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The final result of an agent run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentResult {
    pub status: AgentStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    pub steps: Vec<StepRecord>,
    pub total_iterations: u32,
    pub total_duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Status of the agent run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Success,
    Error,
    Timeout,
    MaxIterations,
}

/// A single step executed during the agent run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StepRecord {
    pub iteration: u32,
    pub tool: String,
    pub input: Value,
    pub output: Value,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

impl AgentResult {
    /// Create a successful result.
    pub fn success(result: String, steps: Vec<StepRecord>, duration_ms: u64) -> Self {
        Self {
            status: AgentStatus::Success,
            result: Some(result),
            steps,
            total_iterations: 0,
            total_duration_ms: duration_ms,
            error: None,
        }
    }

    /// Create an error result.
    pub fn error(error: String, steps: Vec<StepRecord>, duration_ms: u64) -> Self {
        Self {
            status: AgentStatus::Error,
            result: None,
            steps,
            total_iterations: 0,
            total_duration_ms: duration_ms,
            error: Some(error),
        }
    }

    /// Create a timeout result.
    pub fn timeout(steps: Vec<StepRecord>, duration_ms: u64) -> Self {
        Self {
            status: AgentStatus::Timeout,
            result: None,
            steps,
            total_iterations: 0,
            total_duration_ms: duration_ms,
            error: Some("Agent timed out".to_string()),
        }
    }

    /// Create a max-iterations result.
    pub fn max_iterations(steps: Vec<StepRecord>, iterations: u32, duration_ms: u64) -> Self {
        Self {
            status: AgentStatus::MaxIterations,
            result: None,
            steps,
            total_iterations: iterations,
            total_duration_ms: duration_ms,
            error: Some(format!("Max iterations ({iterations}) reached")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_step() -> StepRecord {
        StepRecord {
            iteration: 1,
            tool: "navigate".to_string(),
            input: json!({ "url": "https://example.com" }),
            output: json!({ "success": true }),
            duration_ms: 1200,
            is_error: None,
        }
    }

    fn sample_step_with_error() -> StepRecord {
        StepRecord {
            iteration: 2,
            tool: "click".to_string(),
            input: json!({ "selector": "#btn" }),
            output: json!({ "error": "element not found" }),
            duration_ms: 500,
            is_error: Some(true),
        }
    }

    #[test]
    fn test_serialization_matches_prd_format() {
        let result = AgentResult {
            status: AgentStatus::Success,
            result: Some("task completed".to_string()),
            steps: vec![sample_step()],
            total_iterations: 5,
            total_duration_ms: 15000,
            error: None,
        };

        let json = serde_json::to_value(&result).unwrap();

        assert_eq!(json["status"], "success");
        assert_eq!(json["result"], "task completed");
        assert_eq!(json["total_iterations"], 5);
        assert_eq!(json["total_duration_ms"], 15000);

        let step = &json["steps"][0];
        assert_eq!(step["iteration"], 1);
        assert_eq!(step["tool"], "navigate");
        assert_eq!(step["input"]["url"], "https://example.com");
        assert_eq!(step["output"]["success"], true);
        assert_eq!(step["duration_ms"], 1200);
        // is_error should be omitted when None
        assert!(step.get("is_error").is_none());
    }

    #[test]
    fn test_status_serializes_to_snake_case() {
        assert_eq!(
            serde_json::to_value(AgentStatus::Success).unwrap(),
            json!("success")
        );
        assert_eq!(
            serde_json::to_value(AgentStatus::Error).unwrap(),
            json!("error")
        );
        assert_eq!(
            serde_json::to_value(AgentStatus::Timeout).unwrap(),
            json!("timeout")
        );
        assert_eq!(
            serde_json::to_value(AgentStatus::MaxIterations).unwrap(),
            json!("max_iterations")
        );
    }

    #[test]
    fn test_status_deserializes_from_snake_case() {
        assert_eq!(
            serde_json::from_str::<AgentStatus>("\"success\"").unwrap(),
            AgentStatus::Success
        );
        assert_eq!(
            serde_json::from_str::<AgentStatus>("\"error\"").unwrap(),
            AgentStatus::Error
        );
        assert_eq!(
            serde_json::from_str::<AgentStatus>("\"timeout\"").unwrap(),
            AgentStatus::Timeout
        );
        assert_eq!(
            serde_json::from_str::<AgentStatus>("\"max_iterations\"").unwrap(),
            AgentStatus::MaxIterations
        );
    }

    #[test]
    fn test_success_constructor() {
        let steps = vec![sample_step()];
        let result = AgentResult::success("done".to_string(), steps.clone(), 5000);

        assert_eq!(result.status, AgentStatus::Success);
        assert_eq!(result.result, Some("done".to_string()));
        assert_eq!(result.steps, steps);
        assert_eq!(result.total_duration_ms, 5000);
        assert!(result.error.is_none());
    }

    #[test]
    fn test_error_constructor() {
        let steps = vec![sample_step()];
        let result = AgentResult::error("something broke".to_string(), steps.clone(), 3000);

        assert_eq!(result.status, AgentStatus::Error);
        assert!(result.result.is_none());
        assert_eq!(result.steps, steps);
        assert_eq!(result.total_duration_ms, 3000);
        assert_eq!(result.error, Some("something broke".to_string()));
    }

    #[test]
    fn test_timeout_constructor() {
        let steps = vec![sample_step()];
        let result = AgentResult::timeout(steps.clone(), 30000);

        assert_eq!(result.status, AgentStatus::Timeout);
        assert!(result.result.is_none());
        assert_eq!(result.steps, steps);
        assert_eq!(result.total_duration_ms, 30000);
        assert_eq!(result.error, Some("Agent timed out".to_string()));
    }

    #[test]
    fn test_max_iterations_constructor() {
        let steps = vec![sample_step()];
        let result = AgentResult::max_iterations(steps.clone(), 50, 20000);

        assert_eq!(result.status, AgentStatus::MaxIterations);
        assert!(result.result.is_none());
        assert_eq!(result.steps, steps);
        assert_eq!(result.total_iterations, 50);
        assert_eq!(result.total_duration_ms, 20000);
        assert_eq!(
            result.error,
            Some("Max iterations (50) reached".to_string())
        );
    }

    #[test]
    fn test_deserialization_round_trip() {
        let original = AgentResult {
            status: AgentStatus::Success,
            result: Some("completed".to_string()),
            steps: vec![sample_step(), sample_step_with_error()],
            total_iterations: 2,
            total_duration_ms: 1700,
            error: None,
        };

        let json_str = serde_json::to_string(&original).unwrap();
        let deserialized: AgentResult = serde_json::from_str(&json_str).unwrap();

        assert_eq!(original, deserialized);
    }

    #[test]
    fn test_optional_fields_omitted_when_none() {
        let result = AgentResult {
            status: AgentStatus::Error,
            result: None,
            steps: vec![],
            total_iterations: 0,
            total_duration_ms: 100,
            error: Some("fail".to_string()),
        };

        let json = serde_json::to_value(&result).unwrap();
        let obj = json.as_object().unwrap();

        // result should be absent
        assert!(!obj.contains_key("result"));
        // error should be present
        assert!(obj.contains_key("error"));
    }

    #[test]
    fn test_step_record_is_error_omitted_when_none() {
        let step = sample_step();
        let json = serde_json::to_value(&step).unwrap();
        let obj = json.as_object().unwrap();

        assert!(!obj.contains_key("is_error"));
    }

    #[test]
    fn test_step_record_is_error_present_when_some() {
        let step = sample_step_with_error();
        let json = serde_json::to_value(&step).unwrap();

        assert_eq!(json["is_error"], true);
    }

    #[test]
    fn test_error_field_omitted_when_none() {
        let result = AgentResult::success("ok".to_string(), vec![], 100);
        let json = serde_json::to_value(&result).unwrap();
        let obj = json.as_object().unwrap();

        assert!(!obj.contains_key("error"));
    }

    #[test]
    fn test_deserialization_with_all_fields_present() {
        let input = json!({
            "status": "success",
            "result": "done",
            "steps": [
                {
                    "iteration": 1,
                    "tool": "navigate",
                    "input": { "url": "https://example.com" },
                    "output": { "success": true },
                    "duration_ms": 1200,
                    "is_error": false
                }
            ],
            "total_iterations": 1,
            "total_duration_ms": 1200,
            "error": null
        });

        let result: AgentResult = serde_json::from_value(input).unwrap();
        assert_eq!(result.status, AgentStatus::Success);
        assert_eq!(result.result, Some("done".to_string()));
        assert_eq!(result.steps.len(), 1);
        assert_eq!(result.steps[0].is_error, Some(false));
    }
}
