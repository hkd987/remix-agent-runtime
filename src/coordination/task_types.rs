use std::collections::HashMap;
use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Sequential task identifier.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct TaskId(pub String);

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Task lifecycle status.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    InProgress,
    Completed,
    Deleted,
}

/// A coordination task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub subject: String,
    pub description: String,
    pub status: TaskStatus,
    pub owner: Option<String>,
    pub blocked_by: Vec<TaskId>,
    pub blocks: Vec<TaskId>,
    pub active_form: Option<String>,
    pub metadata: HashMap<String, Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Lightweight summary for listing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSummary {
    pub id: TaskId,
    pub subject: String,
    pub status: TaskStatus,
    pub owner: Option<String>,
    pub blocked_by: Vec<TaskId>,
}

impl From<&Task> for TaskSummary {
    fn from(task: &Task) -> Self {
        Self {
            id: task.id.clone(),
            subject: task.subject.clone(),
            status: task.status.clone(),
            owner: task.owner.clone(),
            blocked_by: task.blocked_by.clone(),
        }
    }
}

/// Parameters for creating a new task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskCreateParams {
    pub subject: String,
    pub description: String,
    pub active_form: Option<String>,
    pub metadata: Option<HashMap<String, Value>>,
}

/// Parameters for updating an existing task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskUpdateParams {
    pub task_id: String,
    pub status: Option<TaskStatus>,
    pub subject: Option<String>,
    pub description: Option<String>,
    pub active_form: Option<String>,
    pub owner: Option<String>,
    pub metadata: Option<HashMap<String, Value>>,
    pub add_blocks: Option<Vec<String>>,
    pub add_blocked_by: Option<Vec<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_task_id_display() {
        let id = TaskId("42".to_string());
        assert_eq!(format!("{}", id), "42");
    }

    #[test]
    fn test_task_id_equality() {
        let a = TaskId("1".to_string());
        let b = TaskId("1".to_string());
        let c = TaskId("2".to_string());
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn test_task_id_hash() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(TaskId("1".to_string()));
        set.insert(TaskId("1".to_string()));
        set.insert(TaskId("2".to_string()));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn test_task_id_serde_roundtrip() {
        let id = TaskId("99".to_string());
        let json = serde_json::to_string(&id).unwrap();
        let deserialized: TaskId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, deserialized);
    }

    #[test]
    fn test_task_status_serde_pending() {
        let status = TaskStatus::Pending;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"pending\"");
        let deserialized: TaskStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, TaskStatus::Pending);
    }

    #[test]
    fn test_task_status_serde_in_progress() {
        let status = TaskStatus::InProgress;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"in_progress\"");
        let deserialized: TaskStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, TaskStatus::InProgress);
    }

    #[test]
    fn test_task_status_serde_completed() {
        let status = TaskStatus::Completed;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"completed\"");
        let deserialized: TaskStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, TaskStatus::Completed);
    }

    #[test]
    fn test_task_status_serde_deleted() {
        let status = TaskStatus::Deleted;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"deleted\"");
        let deserialized: TaskStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, TaskStatus::Deleted);
    }

    fn make_task() -> Task {
        Task {
            id: TaskId("1".to_string()),
            subject: "Fix bug".to_string(),
            description: "Fix the login bug".to_string(),
            status: TaskStatus::Pending,
            owner: Some("alice".to_string()),
            blocked_by: vec![TaskId("2".to_string())],
            blocks: vec![TaskId("3".to_string())],
            active_form: Some("Fixing bug".to_string()),
            metadata: {
                let mut m = HashMap::new();
                m.insert("priority".to_string(), json!("high"));
                m
            },
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn test_task_serde_roundtrip() {
        let task = make_task();
        let json = serde_json::to_string(&task).unwrap();
        let deserialized: Task = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, task.id);
        assert_eq!(deserialized.subject, task.subject);
        assert_eq!(deserialized.description, task.description);
        assert_eq!(deserialized.status, task.status);
        assert_eq!(deserialized.owner, task.owner);
        assert_eq!(deserialized.blocked_by, task.blocked_by);
        assert_eq!(deserialized.blocks, task.blocks);
        assert_eq!(deserialized.active_form, task.active_form);
        assert_eq!(deserialized.metadata, task.metadata);
    }

    #[test]
    fn test_task_with_no_optional_fields() {
        let task = Task {
            id: TaskId("5".to_string()),
            subject: "Simple task".to_string(),
            description: "A simple task".to_string(),
            status: TaskStatus::Pending,
            owner: None,
            blocked_by: vec![],
            blocks: vec![],
            active_form: None,
            metadata: HashMap::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let json = serde_json::to_string(&task).unwrap();
        let deserialized: Task = serde_json::from_str(&json).unwrap();
        assert!(deserialized.owner.is_none());
        assert!(deserialized.blocked_by.is_empty());
        assert!(deserialized.blocks.is_empty());
        assert!(deserialized.active_form.is_none());
        assert!(deserialized.metadata.is_empty());
    }

    #[test]
    fn test_task_summary_from_task() {
        let task = make_task();
        let summary = TaskSummary::from(&task);
        assert_eq!(summary.id, task.id);
        assert_eq!(summary.subject, task.subject);
        assert_eq!(summary.status, task.status);
        assert_eq!(summary.owner, task.owner);
        assert_eq!(summary.blocked_by, task.blocked_by);
    }

    #[test]
    fn test_task_summary_from_task_no_owner() {
        let mut task = make_task();
        task.owner = None;
        let summary = TaskSummary::from(&task);
        assert!(summary.owner.is_none());
    }

    #[test]
    fn test_task_summary_serde_roundtrip() {
        let task = make_task();
        let summary = TaskSummary::from(&task);
        let json = serde_json::to_string(&summary).unwrap();
        let deserialized: TaskSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, summary.id);
        assert_eq!(deserialized.subject, summary.subject);
        assert_eq!(deserialized.status, summary.status);
    }

    #[test]
    fn test_task_create_params_serde() {
        let params = TaskCreateParams {
            subject: "New feature".to_string(),
            description: "Add the new feature".to_string(),
            active_form: Some("Adding feature".to_string()),
            metadata: Some({
                let mut m = HashMap::new();
                m.insert("tag".to_string(), json!("feature"));
                m
            }),
        };
        let json = serde_json::to_string(&params).unwrap();
        let deserialized: TaskCreateParams = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.subject, params.subject);
        assert_eq!(deserialized.description, params.description);
        assert_eq!(deserialized.active_form, params.active_form);
        assert!(deserialized.metadata.is_some());
    }

    #[test]
    fn test_task_create_params_no_optional_fields() {
        let params = TaskCreateParams {
            subject: "Minimal task".to_string(),
            description: "Minimal".to_string(),
            active_form: None,
            metadata: None,
        };
        let json = serde_json::to_string(&params).unwrap();
        let deserialized: TaskCreateParams = serde_json::from_str(&json).unwrap();
        assert!(deserialized.active_form.is_none());
        assert!(deserialized.metadata.is_none());
    }

    #[test]
    fn test_task_update_params_full() {
        let params = TaskUpdateParams {
            task_id: "1".to_string(),
            status: Some(TaskStatus::InProgress),
            subject: Some("Updated subject".to_string()),
            description: Some("Updated description".to_string()),
            active_form: Some("Updating".to_string()),
            owner: Some("bob".to_string()),
            metadata: Some({
                let mut m = HashMap::new();
                m.insert("key".to_string(), json!("value"));
                m
            }),
            add_blocks: Some(vec!["2".to_string()]),
            add_blocked_by: Some(vec!["3".to_string()]),
        };
        let json = serde_json::to_string(&params).unwrap();
        let deserialized: TaskUpdateParams = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.task_id, "1");
        assert_eq!(deserialized.status, Some(TaskStatus::InProgress));
        assert_eq!(deserialized.subject, Some("Updated subject".to_string()));
        assert!(deserialized.add_blocks.is_some());
        assert!(deserialized.add_blocked_by.is_some());
    }

    #[test]
    fn test_task_update_params_minimal() {
        let params = TaskUpdateParams {
            task_id: "7".to_string(),
            status: None,
            subject: None,
            description: None,
            active_form: None,
            owner: None,
            metadata: None,
            add_blocks: None,
            add_blocked_by: None,
        };
        let json = serde_json::to_string(&params).unwrap();
        let deserialized: TaskUpdateParams = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.task_id, "7");
        assert!(deserialized.status.is_none());
        assert!(deserialized.subject.is_none());
    }

    #[test]
    fn test_task_status_all_variants_equality() {
        assert_ne!(TaskStatus::Pending, TaskStatus::InProgress);
        assert_ne!(TaskStatus::InProgress, TaskStatus::Completed);
        assert_ne!(TaskStatus::Completed, TaskStatus::Deleted);
        assert_ne!(TaskStatus::Deleted, TaskStatus::Pending);
    }
}
