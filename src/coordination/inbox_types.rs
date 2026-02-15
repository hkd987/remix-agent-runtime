use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct MessageId(pub String);

impl fmt::Display for MessageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl MessageId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
}

impl Default for MessageId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageType {
    Message,
    Broadcast,
    ShutdownRequest,
    ShutdownResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxMessage {
    pub id: MessageId,
    pub message_type: MessageType,
    pub sender: String,
    pub recipient: Option<String>,
    pub content: String,
    pub summary: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub request_id: Option<MessageId>,
    pub approve: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMessageParams {
    pub message_type: MessageType,
    pub recipient: Option<String>,
    pub content: String,
    pub summary: Option<String>,
    pub request_id: Option<String>,
    pub approve: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_message_id_display() {
        let id = MessageId("abc-123".to_string());
        assert_eq!(format!("{}", id), "abc-123");
    }

    #[test]
    fn test_message_id_uniqueness() {
        let ids: Vec<MessageId> = (0..100).map(|_| MessageId::new()).collect();
        let unique: HashSet<&MessageId> = ids.iter().collect();
        assert_eq!(unique.len(), 100);
    }

    #[test]
    fn test_message_id_default() {
        let id = MessageId::default();
        assert!(!id.0.is_empty());
        // UUID v4 format: 8-4-4-4-12
        assert_eq!(id.0.len(), 36);
    }

    #[test]
    fn test_message_id_equality() {
        let id1 = MessageId("same".to_string());
        let id2 = MessageId("same".to_string());
        let id3 = MessageId("diff".to_string());
        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_message_id_serde_roundtrip() {
        let id = MessageId::new();
        let json = serde_json::to_string(&id).unwrap();
        let deserialized: MessageId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, deserialized);
    }

    #[test]
    fn test_message_type_serde_roundtrip() {
        let types = vec![
            MessageType::Message,
            MessageType::Broadcast,
            MessageType::ShutdownRequest,
            MessageType::ShutdownResponse,
        ];
        for mt in types {
            let json = serde_json::to_string(&mt).unwrap();
            let deserialized: MessageType = serde_json::from_str(&json).unwrap();
            assert_eq!(mt, deserialized);
        }
    }

    #[test]
    fn test_message_type_serde_snake_case() {
        assert_eq!(
            serde_json::to_string(&MessageType::Message).unwrap(),
            "\"message\""
        );
        assert_eq!(
            serde_json::to_string(&MessageType::Broadcast).unwrap(),
            "\"broadcast\""
        );
        assert_eq!(
            serde_json::to_string(&MessageType::ShutdownRequest).unwrap(),
            "\"shutdown_request\""
        );
        assert_eq!(
            serde_json::to_string(&MessageType::ShutdownResponse).unwrap(),
            "\"shutdown_response\""
        );
    }

    #[test]
    fn test_inbox_message_serde_roundtrip() {
        let msg = InboxMessage {
            id: MessageId::new(),
            message_type: MessageType::Message,
            sender: "agent-a".to_string(),
            recipient: Some("agent-b".to_string()),
            content: "Hello, agent-b!".to_string(),
            summary: Some("Greeting".to_string()),
            timestamp: Utc::now(),
            request_id: None,
            approve: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: InboxMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, msg.id);
        assert_eq!(deserialized.message_type, MessageType::Message);
        assert_eq!(deserialized.sender, "agent-a");
        assert_eq!(deserialized.recipient, Some("agent-b".to_string()));
        assert_eq!(deserialized.content, "Hello, agent-b!");
        assert_eq!(deserialized.summary, Some("Greeting".to_string()));
        assert_eq!(deserialized.request_id, None);
        assert_eq!(deserialized.approve, None);
    }

    #[test]
    fn test_inbox_message_serde_shutdown_request() {
        let request_id = MessageId::new();
        let msg = InboxMessage {
            id: MessageId::new(),
            message_type: MessageType::ShutdownRequest,
            sender: "lead".to_string(),
            recipient: Some("worker".to_string()),
            content: "Please shut down".to_string(),
            summary: None,
            timestamp: Utc::now(),
            request_id: Some(request_id.clone()),
            approve: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: InboxMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.message_type, MessageType::ShutdownRequest);
        assert_eq!(deserialized.request_id, Some(request_id));
    }

    #[test]
    fn test_inbox_message_serde_shutdown_response() {
        let request_id = MessageId::new();
        let msg = InboxMessage {
            id: MessageId::new(),
            message_type: MessageType::ShutdownResponse,
            sender: "worker".to_string(),
            recipient: Some("lead".to_string()),
            content: "Shutting down".to_string(),
            summary: None,
            timestamp: Utc::now(),
            request_id: Some(request_id.clone()),
            approve: Some(true),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: InboxMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.message_type, MessageType::ShutdownResponse);
        assert_eq!(deserialized.approve, Some(true));
        assert_eq!(deserialized.request_id, Some(request_id));
    }

    #[test]
    fn test_send_message_params_serde() {
        let params = SendMessageParams {
            message_type: MessageType::Message,
            recipient: Some("agent-b".to_string()),
            content: "Hello".to_string(),
            summary: Some("Greeting".to_string()),
            request_id: None,
            approve: None,
        };
        let json = serde_json::to_string(&params).unwrap();
        let deserialized: SendMessageParams = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.message_type, MessageType::Message);
        assert_eq!(deserialized.recipient, Some("agent-b".to_string()));
        assert_eq!(deserialized.content, "Hello");
        assert_eq!(deserialized.summary, Some("Greeting".to_string()));
    }

    #[test]
    fn test_send_message_params_serde_broadcast() {
        let params = SendMessageParams {
            message_type: MessageType::Broadcast,
            recipient: None,
            content: "Announcement".to_string(),
            summary: Some("Team update".to_string()),
            request_id: None,
            approve: None,
        };
        let json = serde_json::to_string(&params).unwrap();
        let deserialized: SendMessageParams = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.message_type, MessageType::Broadcast);
        assert_eq!(deserialized.recipient, None);
    }

    #[test]
    fn test_send_message_params_serde_shutdown_response() {
        let params = SendMessageParams {
            message_type: MessageType::ShutdownResponse,
            recipient: Some("lead".to_string()),
            content: "Acknowledged".to_string(),
            summary: None,
            request_id: Some("req-123".to_string()),
            approve: Some(true),
        };
        let json = serde_json::to_string(&params).unwrap();
        let deserialized: SendMessageParams = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.message_type, MessageType::ShutdownResponse);
        assert_eq!(deserialized.request_id, Some("req-123".to_string()));
        assert_eq!(deserialized.approve, Some(true));
    }
}
