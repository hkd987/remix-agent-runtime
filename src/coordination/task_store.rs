use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::Utc;
use serde_json::Value;
use tokio::sync::RwLock;

use crate::error::AgentError;

use super::task_types::{
    Task, TaskCreateParams, TaskId, TaskStatus, TaskSummary, TaskUpdateParams,
};

pub struct TaskStore {
    tasks: RwLock<HashMap<String, Task>>,
    next_id: AtomicU64,
    storage_dir: PathBuf,
}

impl std::fmt::Debug for TaskStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskStore")
            .field("next_id", &self.next_id.load(Ordering::Relaxed))
            .field("storage_dir", &self.storage_dir)
            .finish()
    }
}

impl TaskStore {
    /// Create a new TaskStore, loading existing tasks from disk if present.
    pub fn new(storage_dir: PathBuf) -> Self {
        std::fs::create_dir_all(&storage_dir).ok();

        let (tasks, next_id) = Self::load_from_disk(&storage_dir).unwrap_or_default();

        Self {
            tasks: RwLock::new(tasks),
            next_id: AtomicU64::new(next_id),
            storage_dir,
        }
    }

    /// Create a new task with a sequential ID.
    pub async fn create(&self, params: TaskCreateParams) -> Result<Task, AgentError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let now = Utc::now();

        let task = Task {
            id: TaskId(id.to_string()),
            subject: params.subject,
            description: params.description,
            status: TaskStatus::Pending,
            owner: None,
            blocked_by: vec![],
            blocks: vec![],
            active_form: params.active_form,
            metadata: params.metadata.unwrap_or_default(),
            created_at: now,
            updated_at: now,
        };

        {
            let mut tasks = self.tasks.write().await;
            tasks.insert(task.id.0.clone(), task.clone());
        }

        self.persist().await?;
        Ok(task)
    }

    /// List all non-deleted tasks as summaries sorted by numeric ID.
    /// Filters blocked_by to only include IDs of tasks that are NOT Completed/Deleted.
    pub async fn list(&self) -> Vec<TaskSummary> {
        let tasks = self.tasks.read().await;

        let mut summaries: Vec<TaskSummary> = tasks
            .values()
            .filter(|t| t.status != TaskStatus::Deleted)
            .map(|task| {
                let filtered_blocked_by: Vec<TaskId> = task
                    .blocked_by
                    .iter()
                    .filter(|dep_id| {
                        tasks
                            .get(&dep_id.0)
                            .map(|dep_task| {
                                dep_task.status != TaskStatus::Completed
                                    && dep_task.status != TaskStatus::Deleted
                            })
                            .unwrap_or(false)
                    })
                    .cloned()
                    .collect();

                TaskSummary {
                    id: task.id.clone(),
                    subject: task.subject.clone(),
                    status: task.status.clone(),
                    owner: task.owner.clone(),
                    blocked_by: filtered_blocked_by,
                }
            })
            .collect();

        summaries.sort_by(|a, b| {
            let a_num: u64 = a.id.0.parse().unwrap_or(0);
            let b_num: u64 = b.id.0.parse().unwrap_or(0);
            a_num.cmp(&b_num)
        });

        summaries
    }

    /// Get a task by ID.
    pub async fn get(&self, task_id: &str) -> Result<Task, AgentError> {
        let tasks = self.tasks.read().await;
        tasks
            .get(task_id)
            .cloned()
            .ok_or_else(|| AgentError::ToolExecution(format!("Task not found: {}", task_id)))
    }

    /// Apply partial updates to a task.
    pub async fn update(&self, params: TaskUpdateParams) -> Result<Task, AgentError> {
        let mut tasks = self.tasks.write().await;

        let task = tasks.get_mut(&params.task_id).ok_or_else(|| {
            AgentError::ToolExecution(format!("Task not found: {}", params.task_id))
        })?;

        if let Some(status) = params.status {
            task.status = status;
        }
        if let Some(subject) = params.subject {
            task.subject = subject;
        }
        if let Some(description) = params.description {
            task.description = description;
        }
        if let Some(active_form) = params.active_form {
            task.active_form = Some(active_form);
        }
        if let Some(owner) = params.owner {
            task.owner = Some(owner);
        }

        // Merge metadata: add new keys, delete keys with null values
        if let Some(new_metadata) = params.metadata {
            for (key, value) in new_metadata {
                if value == Value::Null {
                    task.metadata.remove(&key);
                } else {
                    task.metadata.insert(key, value);
                }
            }
        }

        // Append to blocks
        if let Some(add_blocks) = params.add_blocks {
            for block_id in add_blocks {
                let tid = TaskId(block_id);
                if !task.blocks.contains(&tid) {
                    task.blocks.push(tid);
                }
            }
        }

        // Append to blocked_by
        if let Some(add_blocked_by) = params.add_blocked_by {
            for blocked_id in add_blocked_by {
                let tid = TaskId(blocked_id);
                if !task.blocked_by.contains(&tid) {
                    task.blocked_by.push(tid);
                }
            }
        }

        task.updated_at = Utc::now();

        let updated = task.clone();
        drop(tasks);

        self.persist().await?;
        Ok(updated)
    }

    /// Persist all tasks to disk atomically (write to tmp, then rename).
    pub async fn persist(&self) -> Result<(), AgentError> {
        let tasks = self.tasks.read().await;
        let task_list: Vec<&Task> = tasks.values().collect();
        let json = serde_json::to_string_pretty(&task_list)
            .map_err(|e| AgentError::ToolExecution(format!("Failed to serialize tasks: {}", e)))?;
        drop(tasks);

        let tmp_path = self.storage_dir.join("tasks.json.tmp");
        let final_path = self.storage_dir.join("tasks.json");

        std::fs::write(&tmp_path, &json)
            .map_err(|e| AgentError::ToolExecution(format!("Failed to write tmp file: {}", e)))?;

        std::fs::rename(&tmp_path, &final_path)
            .map_err(|e| AgentError::ToolExecution(format!("Failed to rename tmp file: {}", e)))?;

        Ok(())
    }

    /// Load tasks from disk, returning the task map and the next available ID.
    pub fn load_from_disk(storage_dir: &Path) -> Result<(HashMap<String, Task>, u64), AgentError> {
        let path = storage_dir.join("tasks.json");
        if !path.exists() {
            return Ok((HashMap::new(), 1));
        }

        let content = std::fs::read_to_string(&path)
            .map_err(|e| AgentError::ToolExecution(format!("Failed to read tasks.json: {}", e)))?;

        let task_list: Vec<Task> = serde_json::from_str(&content)
            .map_err(|e| AgentError::ToolExecution(format!("Failed to parse tasks.json: {}", e)))?;

        let mut max_id: u64 = 0;
        let mut tasks = HashMap::new();
        for task in task_list {
            if let Ok(num_id) = task.id.0.parse::<u64>() {
                if num_id > max_id {
                    max_id = num_id;
                }
            }
            tasks.insert(task.id.0.clone(), task);
        }

        Ok((tasks, max_id + 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    fn make_params(subject: &str) -> TaskCreateParams {
        TaskCreateParams {
            subject: subject.to_string(),
            description: format!("Description for {}", subject),
            active_form: None,
            metadata: None,
        }
    }

    #[tokio::test]
    async fn test_create_sequential_ids() {
        let tmp = TempDir::new().unwrap();
        let store = TaskStore::new(tmp.path().to_path_buf());

        let t1 = store.create(make_params("Task 1")).await.unwrap();
        let t2 = store.create(make_params("Task 2")).await.unwrap();
        let t3 = store.create(make_params("Task 3")).await.unwrap();

        let id1: u64 = t1.id.0.parse().unwrap();
        let id2: u64 = t2.id.0.parse().unwrap();
        let id3: u64 = t3.id.0.parse().unwrap();

        assert_eq!(id2, id1 + 1);
        assert_eq!(id3, id2 + 1);
    }

    #[tokio::test]
    async fn test_create_sets_defaults() {
        let tmp = TempDir::new().unwrap();
        let store = TaskStore::new(tmp.path().to_path_buf());

        let task = store.create(make_params("Test")).await.unwrap();
        assert_eq!(task.status, TaskStatus::Pending);
        assert!(task.owner.is_none());
        assert!(task.blocked_by.is_empty());
        assert!(task.blocks.is_empty());
        assert!(task.metadata.is_empty());
    }

    #[tokio::test]
    async fn test_create_with_metadata_and_active_form() {
        let tmp = TempDir::new().unwrap();
        let store = TaskStore::new(tmp.path().to_path_buf());

        let mut meta = HashMap::new();
        meta.insert("priority".to_string(), json!("high"));

        let params = TaskCreateParams {
            subject: "Important task".to_string(),
            description: "Very important".to_string(),
            active_form: Some("Working on important task".to_string()),
            metadata: Some(meta),
        };

        let task = store.create(params).await.unwrap();
        assert_eq!(
            task.active_form,
            Some("Working on important task".to_string())
        );
        assert_eq!(task.metadata.get("priority"), Some(&json!("high")));
    }

    #[tokio::test]
    async fn test_list_returns_sorted_summaries() {
        let tmp = TempDir::new().unwrap();
        let store = TaskStore::new(tmp.path().to_path_buf());

        store.create(make_params("Task A")).await.unwrap();
        store.create(make_params("Task B")).await.unwrap();
        store.create(make_params("Task C")).await.unwrap();

        let summaries = store.list().await;
        assert_eq!(summaries.len(), 3);

        let ids: Vec<u64> = summaries.iter().map(|s| s.id.0.parse().unwrap()).collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]));
    }

    #[tokio::test]
    async fn test_list_filters_blocked_by_to_open_tasks() {
        let tmp = TempDir::new().unwrap();
        let store = TaskStore::new(tmp.path().to_path_buf());

        let blocker = store.create(make_params("Blocker")).await.unwrap();
        let dependent = store.create(make_params("Dependent")).await.unwrap();

        // Add blocker as blocked_by for dependent
        store
            .update(TaskUpdateParams {
                task_id: dependent.id.0.clone(),
                add_blocked_by: Some(vec![blocker.id.0.clone()]),
                status: None,
                subject: None,
                description: None,
                active_form: None,
                owner: None,
                metadata: None,
                add_blocks: None,
            })
            .await
            .unwrap();

        // Verify blocked_by is present when blocker is pending
        let summaries = store.list().await;
        let dep_summary = summaries.iter().find(|s| s.id == dependent.id).unwrap();
        assert_eq!(dep_summary.blocked_by.len(), 1);

        // Complete the blocker
        store
            .update(TaskUpdateParams {
                task_id: blocker.id.0.clone(),
                status: Some(TaskStatus::Completed),
                subject: None,
                description: None,
                active_form: None,
                owner: None,
                metadata: None,
                add_blocks: None,
                add_blocked_by: None,
            })
            .await
            .unwrap();

        // blocked_by should now be empty since blocker is completed
        let summaries = store.list().await;
        let dep_summary = summaries.iter().find(|s| s.id == dependent.id).unwrap();
        assert!(dep_summary.blocked_by.is_empty());
    }

    #[tokio::test]
    async fn test_get_existing_task() {
        let tmp = TempDir::new().unwrap();
        let store = TaskStore::new(tmp.path().to_path_buf());

        let created = store.create(make_params("Find me")).await.unwrap();
        let found = store.get(&created.id.0).await.unwrap();

        assert_eq!(found.id, created.id);
        assert_eq!(found.subject, "Find me");
    }

    #[tokio::test]
    async fn test_get_nonexistent_returns_error() {
        let tmp = TempDir::new().unwrap();
        let store = TaskStore::new(tmp.path().to_path_buf());

        let result = store.get("999").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Task not found"));
    }

    #[tokio::test]
    async fn test_update_status() {
        let tmp = TempDir::new().unwrap();
        let store = TaskStore::new(tmp.path().to_path_buf());

        let task = store.create(make_params("Status test")).await.unwrap();
        assert_eq!(task.status, TaskStatus::Pending);

        let updated = store
            .update(TaskUpdateParams {
                task_id: task.id.0.clone(),
                status: Some(TaskStatus::InProgress),
                subject: None,
                description: None,
                active_form: None,
                owner: None,
                metadata: None,
                add_blocks: None,
                add_blocked_by: None,
            })
            .await
            .unwrap();

        assert_eq!(updated.status, TaskStatus::InProgress);
    }

    #[tokio::test]
    async fn test_update_subject_and_description() {
        let tmp = TempDir::new().unwrap();
        let store = TaskStore::new(tmp.path().to_path_buf());

        let task = store.create(make_params("Original")).await.unwrap();

        let updated = store
            .update(TaskUpdateParams {
                task_id: task.id.0.clone(),
                status: None,
                subject: Some("Updated subject".to_string()),
                description: Some("Updated description".to_string()),
                active_form: None,
                owner: None,
                metadata: None,
                add_blocks: None,
                add_blocked_by: None,
            })
            .await
            .unwrap();

        assert_eq!(updated.subject, "Updated subject");
        assert_eq!(updated.description, "Updated description");
    }

    #[tokio::test]
    async fn test_update_owner() {
        let tmp = TempDir::new().unwrap();
        let store = TaskStore::new(tmp.path().to_path_buf());

        let task = store.create(make_params("Owner test")).await.unwrap();
        assert!(task.owner.is_none());

        let updated = store
            .update(TaskUpdateParams {
                task_id: task.id.0.clone(),
                status: None,
                subject: None,
                description: None,
                active_form: None,
                owner: Some("alice".to_string()),
                metadata: None,
                add_blocks: None,
                add_blocked_by: None,
            })
            .await
            .unwrap();

        assert_eq!(updated.owner, Some("alice".to_string()));
    }

    #[tokio::test]
    async fn test_update_add_blocks_and_blocked_by() {
        let tmp = TempDir::new().unwrap();
        let store = TaskStore::new(tmp.path().to_path_buf());

        let task = store.create(make_params("Deps test")).await.unwrap();

        let updated = store
            .update(TaskUpdateParams {
                task_id: task.id.0.clone(),
                status: None,
                subject: None,
                description: None,
                active_form: None,
                owner: None,
                metadata: None,
                add_blocks: Some(vec!["10".to_string(), "11".to_string()]),
                add_blocked_by: Some(vec!["20".to_string()]),
            })
            .await
            .unwrap();

        assert_eq!(updated.blocks.len(), 2);
        assert_eq!(updated.blocked_by.len(), 1);
        assert!(updated.blocks.contains(&TaskId("10".to_string())));
        assert!(updated.blocks.contains(&TaskId("11".to_string())));
        assert!(updated.blocked_by.contains(&TaskId("20".to_string())));
    }

    #[tokio::test]
    async fn test_update_add_blocks_deduplicates() {
        let tmp = TempDir::new().unwrap();
        let store = TaskStore::new(tmp.path().to_path_buf());

        let task = store.create(make_params("Dedup test")).await.unwrap();

        store
            .update(TaskUpdateParams {
                task_id: task.id.0.clone(),
                status: None,
                subject: None,
                description: None,
                active_form: None,
                owner: None,
                metadata: None,
                add_blocks: Some(vec!["10".to_string()]),
                add_blocked_by: None,
            })
            .await
            .unwrap();

        let updated = store
            .update(TaskUpdateParams {
                task_id: task.id.0.clone(),
                status: None,
                subject: None,
                description: None,
                active_form: None,
                owner: None,
                metadata: None,
                add_blocks: Some(vec!["10".to_string()]),
                add_blocked_by: None,
            })
            .await
            .unwrap();

        assert_eq!(updated.blocks.len(), 1);
    }

    #[tokio::test]
    async fn test_update_metadata_merge_add_and_delete() {
        let tmp = TempDir::new().unwrap();
        let store = TaskStore::new(tmp.path().to_path_buf());

        let mut initial_meta = HashMap::new();
        initial_meta.insert("key1".to_string(), json!("value1"));
        initial_meta.insert("key2".to_string(), json!("value2"));

        let task = store
            .create(TaskCreateParams {
                subject: "Meta test".to_string(),
                description: "Metadata test".to_string(),
                active_form: None,
                metadata: Some(initial_meta),
            })
            .await
            .unwrap();

        // Update: add key3, delete key1, modify key2
        let mut update_meta = HashMap::new();
        update_meta.insert("key1".to_string(), Value::Null); // delete
        update_meta.insert("key2".to_string(), json!("updated")); // modify
        update_meta.insert("key3".to_string(), json!("new")); // add

        let updated = store
            .update(TaskUpdateParams {
                task_id: task.id.0.clone(),
                status: None,
                subject: None,
                description: None,
                active_form: None,
                owner: None,
                metadata: Some(update_meta),
                add_blocks: None,
                add_blocked_by: None,
            })
            .await
            .unwrap();

        assert!(!updated.metadata.contains_key("key1"));
        assert_eq!(updated.metadata.get("key2"), Some(&json!("updated")));
        assert_eq!(updated.metadata.get("key3"), Some(&json!("new")));
    }

    #[tokio::test]
    async fn test_update_nonexistent_returns_error() {
        let tmp = TempDir::new().unwrap();
        let store = TaskStore::new(tmp.path().to_path_buf());

        let result = store
            .update(TaskUpdateParams {
                task_id: "999".to_string(),
                status: Some(TaskStatus::Completed),
                subject: None,
                description: None,
                active_form: None,
                owner: None,
                metadata: None,
                add_blocks: None,
                add_blocked_by: None,
            })
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Task not found"));
    }

    #[tokio::test]
    async fn test_update_updates_timestamp() {
        let tmp = TempDir::new().unwrap();
        let store = TaskStore::new(tmp.path().to_path_buf());

        let task = store.create(make_params("Timestamp test")).await.unwrap();
        let original_updated_at = task.updated_at;

        // Small delay to ensure timestamp differs
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        let updated = store
            .update(TaskUpdateParams {
                task_id: task.id.0.clone(),
                status: Some(TaskStatus::InProgress),
                subject: None,
                description: None,
                active_form: None,
                owner: None,
                metadata: None,
                add_blocks: None,
                add_blocked_by: None,
            })
            .await
            .unwrap();

        assert!(updated.updated_at >= original_updated_at);
    }

    #[tokio::test]
    async fn test_concurrent_creates() {
        let tmp = TempDir::new().unwrap();
        let store = std::sync::Arc::new(TaskStore::new(tmp.path().to_path_buf()));

        let mut handles = vec![];
        for i in 0..10 {
            let store = store.clone();
            handles.push(tokio::spawn(async move {
                store
                    .create(make_params(&format!("Concurrent {}", i)))
                    .await
                    .unwrap()
            }));
        }

        let mut ids = vec![];
        for handle in handles {
            let task = handle.await.unwrap();
            ids.push(task.id.0.parse::<u64>().unwrap());
        }

        // All IDs should be unique
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 10);

        // All tasks should be listable
        let summaries = store.list().await;
        assert_eq!(summaries.len(), 10);
    }

    #[tokio::test]
    async fn test_persist_and_reload() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().to_path_buf();

        // Create tasks in first store
        {
            let store = TaskStore::new(dir.clone());
            store.create(make_params("Task A")).await.unwrap();
            store.create(make_params("Task B")).await.unwrap();
            store.create(make_params("Task C")).await.unwrap();
        }

        // Load from disk in new store
        let store2 = TaskStore::new(dir);
        let summaries = store2.list().await;
        assert_eq!(summaries.len(), 3);

        // IDs should continue sequencing
        let t4 = store2.create(make_params("Task D")).await.unwrap();
        let t4_id: u64 = t4.id.0.parse().unwrap();
        // Should be at least 4 (IDs 1,2,3 used, next is 4)
        assert!(t4_id >= 4);
    }

    #[tokio::test]
    async fn test_delete_via_status_hides_from_list() {
        let tmp = TempDir::new().unwrap();
        let store = TaskStore::new(tmp.path().to_path_buf());

        let task = store.create(make_params("To delete")).await.unwrap();

        store
            .update(TaskUpdateParams {
                task_id: task.id.0.clone(),
                status: Some(TaskStatus::Deleted),
                subject: None,
                description: None,
                active_form: None,
                owner: None,
                metadata: None,
                add_blocks: None,
                add_blocked_by: None,
            })
            .await
            .unwrap();

        let summaries = store.list().await;
        assert!(summaries.is_empty());

        // But get should still find it
        let found = store.get(&task.id.0).await.unwrap();
        assert_eq!(found.status, TaskStatus::Deleted);
    }

    #[tokio::test]
    async fn test_deleted_task_removed_from_blocked_by_in_list() {
        let tmp = TempDir::new().unwrap();
        let store = TaskStore::new(tmp.path().to_path_buf());

        let blocker = store.create(make_params("Blocker")).await.unwrap();
        let dependent = store.create(make_params("Dependent")).await.unwrap();

        store
            .update(TaskUpdateParams {
                task_id: dependent.id.0.clone(),
                add_blocked_by: Some(vec![blocker.id.0.clone()]),
                status: None,
                subject: None,
                description: None,
                active_form: None,
                owner: None,
                metadata: None,
                add_blocks: None,
            })
            .await
            .unwrap();

        // Delete the blocker
        store
            .update(TaskUpdateParams {
                task_id: blocker.id.0.clone(),
                status: Some(TaskStatus::Deleted),
                subject: None,
                description: None,
                active_form: None,
                owner: None,
                metadata: None,
                add_blocks: None,
                add_blocked_by: None,
            })
            .await
            .unwrap();

        let summaries = store.list().await;
        let dep_summary = summaries.iter().find(|s| s.id == dependent.id).unwrap();
        assert!(dep_summary.blocked_by.is_empty());
    }

    #[tokio::test]
    async fn test_list_empty_store() {
        let tmp = TempDir::new().unwrap();
        let store = TaskStore::new(tmp.path().to_path_buf());

        let summaries = store.list().await;
        assert!(summaries.is_empty());
    }

    #[tokio::test]
    async fn test_load_from_disk_no_file() {
        let tmp = TempDir::new().unwrap();
        let (tasks, next_id) = TaskStore::load_from_disk(tmp.path()).unwrap();
        assert!(tasks.is_empty());
        assert_eq!(next_id, 1);
    }
}
