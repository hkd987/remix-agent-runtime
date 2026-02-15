use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::llm::types::Message;
use crate::output::result::StepRecord;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub String);

impl SessionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    InProgress,
    Completed,
    Error,
    Paused,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub id: SessionId,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub task: String,
    pub status: SessionStatus,
    pub total_input_tokens: Option<u32>,
    pub total_output_tokens: Option<u32>,
    pub parent_session_id: Option<SessionId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub metadata: SessionMetadata,
    pub messages: Vec<Message>,
    pub steps: Vec<StepRecord>,
    pub system_prompt: Option<String>,
    pub iteration: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::types::{ContentBlock, Role};
    use chrono::Utc;
    use serde_json::json;
    use std::collections::HashSet;

    fn sample_metadata() -> SessionMetadata {
        let now = Utc::now();
        SessionMetadata {
            id: SessionId("test-session-123".to_string()),
            created_at: now,
            updated_at: now,
            task: "Navigate to example.com".to_string(),
            status: SessionStatus::InProgress,
            total_input_tokens: None,
            total_output_tokens: None,
            parent_session_id: None,
        }
    }

    fn sample_snapshot() -> SessionSnapshot {
        SessionSnapshot {
            metadata: sample_metadata(),
            messages: vec![Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "Hello".to_string(),
                }],
            }],
            steps: vec![StepRecord {
                iteration: 1,
                tool: "navigate".to_string(),
                input: json!({"url": "https://example.com"}),
                output: json!({"success": true}),
                duration_ms: 500,
                is_error: None,
            }],
            system_prompt: Some("You are a helpful agent.".to_string()),
            iteration: 1,
        }
    }

    #[test]
    fn test_session_id_new_generates_unique_ids() {
        let mut ids = HashSet::new();
        for _ in 0..100 {
            let id = SessionId::new();
            assert!(!id.0.is_empty());
            ids.insert(id.0);
        }
        assert_eq!(ids.len(), 100, "All generated IDs should be unique");
    }

    #[test]
    fn test_session_id_new_is_valid_uuid() {
        let id = SessionId::new();
        let parsed = Uuid::parse_str(&id.0);
        assert!(parsed.is_ok(), "SessionId should contain a valid UUID");
    }

    #[test]
    fn test_session_id_default() {
        let id = SessionId::default();
        assert!(!id.0.is_empty());
        let parsed = Uuid::parse_str(&id.0);
        assert!(parsed.is_ok());
    }

    #[test]
    fn test_session_id_display() {
        let id = SessionId("my-session-id".to_string());
        assert_eq!(format!("{}", id), "my-session-id");
    }

    #[test]
    fn test_session_id_equality() {
        let a = SessionId("abc".to_string());
        let b = SessionId("abc".to_string());
        let c = SessionId("xyz".to_string());
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn test_session_id_hash() {
        let mut set = HashSet::new();
        set.insert(SessionId("a".to_string()));
        set.insert(SessionId("a".to_string()));
        set.insert(SessionId("b".to_string()));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn test_session_id_serde_roundtrip() {
        let id = SessionId("session-42".to_string());
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"session-42\"");
        let deserialized: SessionId = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, id);
    }

    #[test]
    fn test_session_status_serde_in_progress() {
        let json = serde_json::to_string(&SessionStatus::InProgress).unwrap();
        assert_eq!(json, "\"in_progress\"");
        let deserialized: SessionStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, SessionStatus::InProgress);
    }

    #[test]
    fn test_session_status_serde_completed() {
        let json = serde_json::to_string(&SessionStatus::Completed).unwrap();
        assert_eq!(json, "\"completed\"");
        let deserialized: SessionStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, SessionStatus::Completed);
    }

    #[test]
    fn test_session_status_serde_error() {
        let json = serde_json::to_string(&SessionStatus::Error).unwrap();
        assert_eq!(json, "\"error\"");
        let deserialized: SessionStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, SessionStatus::Error);
    }

    #[test]
    fn test_session_status_serde_paused() {
        let json = serde_json::to_string(&SessionStatus::Paused).unwrap();
        assert_eq!(json, "\"paused\"");
        let deserialized: SessionStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, SessionStatus::Paused);
    }

    #[test]
    fn test_session_metadata_serde_roundtrip() {
        let metadata = sample_metadata();
        let json_str = serde_json::to_string(&metadata).unwrap();
        let deserialized: SessionMetadata = serde_json::from_str(&json_str).unwrap();
        assert_eq!(deserialized.id, metadata.id);
        assert_eq!(deserialized.task, metadata.task);
        assert_eq!(deserialized.status, metadata.status);
        assert_eq!(deserialized.total_input_tokens, None);
        assert_eq!(deserialized.total_output_tokens, None);
        assert_eq!(deserialized.parent_session_id, None);
    }

    #[test]
    fn test_session_metadata_with_tokens() {
        let mut metadata = sample_metadata();
        metadata.total_input_tokens = Some(1500);
        metadata.total_output_tokens = Some(800);
        let json_str = serde_json::to_string(&metadata).unwrap();
        let deserialized: SessionMetadata = serde_json::from_str(&json_str).unwrap();
        assert_eq!(deserialized.total_input_tokens, Some(1500));
        assert_eq!(deserialized.total_output_tokens, Some(800));
    }

    #[test]
    fn test_session_metadata_with_parent() {
        let mut metadata = sample_metadata();
        metadata.parent_session_id = Some(SessionId("parent-session".to_string()));
        let json_str = serde_json::to_string(&metadata).unwrap();
        let deserialized: SessionMetadata = serde_json::from_str(&json_str).unwrap();
        assert_eq!(
            deserialized.parent_session_id,
            Some(SessionId("parent-session".to_string()))
        );
    }

    #[test]
    fn test_session_metadata_json_structure() {
        let metadata = sample_metadata();
        let json = serde_json::to_value(&metadata).unwrap();
        assert_eq!(json["id"], "test-session-123");
        assert_eq!(json["task"], "Navigate to example.com");
        assert_eq!(json["status"], "in_progress");
        assert!(json["created_at"].is_string());
        assert!(json["updated_at"].is_string());
    }

    #[test]
    fn test_session_snapshot_serde_roundtrip() {
        let snapshot = sample_snapshot();
        let json_str = serde_json::to_string(&snapshot).unwrap();
        let deserialized: SessionSnapshot = serde_json::from_str(&json_str).unwrap();
        assert_eq!(deserialized.metadata.id, snapshot.metadata.id);
        assert_eq!(deserialized.messages.len(), 1);
        assert_eq!(deserialized.steps.len(), 1);
        assert_eq!(
            deserialized.system_prompt,
            Some("You are a helpful agent.".to_string())
        );
        assert_eq!(deserialized.iteration, 1);
    }

    #[test]
    fn test_session_snapshot_empty_messages_and_steps() {
        let mut snapshot = sample_snapshot();
        snapshot.messages = vec![];
        snapshot.steps = vec![];
        snapshot.system_prompt = None;
        snapshot.iteration = 0;
        let json_str = serde_json::to_string(&snapshot).unwrap();
        let deserialized: SessionSnapshot = serde_json::from_str(&json_str).unwrap();
        assert!(deserialized.messages.is_empty());
        assert!(deserialized.steps.is_empty());
        assert!(deserialized.system_prompt.is_none());
        assert_eq!(deserialized.iteration, 0);
    }

    #[test]
    fn test_session_snapshot_json_structure() {
        let snapshot = sample_snapshot();
        let json = serde_json::to_value(&snapshot).unwrap();
        assert!(json["metadata"].is_object());
        assert!(json["messages"].is_array());
        assert!(json["steps"].is_array());
        assert_eq!(json["system_prompt"], "You are a helpful agent.");
        assert_eq!(json["iteration"], 1);
    }

    #[test]
    fn test_session_metadata_timestamps_preserved() {
        let metadata = sample_metadata();
        let json_str = serde_json::to_string(&metadata).unwrap();
        let deserialized: SessionMetadata = serde_json::from_str(&json_str).unwrap();
        // Chrono serializes with sub-second precision; compare at millisecond level
        assert_eq!(
            metadata.created_at.timestamp_millis(),
            deserialized.created_at.timestamp_millis()
        );
        assert_eq!(
            metadata.updated_at.timestamp_millis(),
            deserialized.updated_at.timestamp_millis()
        );
    }
}
