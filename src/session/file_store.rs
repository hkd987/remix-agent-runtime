use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use chrono::Utc;

use crate::error::AgentError;
use crate::llm::types::Message;
use crate::output::result::StepRecord;

use super::traits::SessionStorage;
use super::types::{compute_iteration, SessionId, SessionMetadata, SessionSnapshot, SessionStatus};

/// Run a blocking closure on the Tokio blocking thread pool and flatten the join error.
async fn blocking<F, T>(f: F) -> Result<T, AgentError>
where
    F: FnOnce() -> Result<T, AgentError> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| AgentError::Session(format!("Task join error: {e}")))?
}

/// Read JSONL messages from the given path. Returns an empty vec if the file does not exist.
fn load_messages_sync(path: &Path) -> Result<Vec<Message>, AgentError> {
    if !path.exists() {
        return Ok(vec![]);
    }
    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);
    let mut messages = Vec::new();
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let msg: Message = serde_json::from_str(&line)?;
        messages.push(msg);
    }
    Ok(messages)
}

pub struct FileSessionStore {
    root_dir: PathBuf,
    max_sessions: usize,
}

impl FileSessionStore {
    pub fn new(root_dir: PathBuf, max_sessions: usize) -> Self {
        Self {
            root_dir,
            max_sessions,
        }
    }

    fn session_dir(&self, session_id: &SessionId) -> PathBuf {
        self.root_dir.join(&session_id.0)
    }

    fn count_sessions_sync(root_dir: &std::path::Path) -> Result<usize, AgentError> {
        if !root_dir.exists() {
            return Ok(0);
        }
        let count = fs::read_dir(root_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
            .count();
        Ok(count)
    }

    fn load_steps_sync(dir: &std::path::Path) -> Result<Vec<StepRecord>, AgentError> {
        let path = dir.join("steps.json");
        if !path.exists() {
            return Ok(vec![]);
        }
        let content = fs::read_to_string(path)?;
        let steps: Vec<StepRecord> = serde_json::from_str(&content)?;
        Ok(steps)
    }
}

#[async_trait]
impl SessionStorage for FileSessionStore {
    async fn create(&self, task: &str) -> Result<SessionMetadata, AgentError> {
        let root_dir = self.root_dir.clone();
        let max_sessions = self.max_sessions;
        let task = task.to_string();

        blocking(move || {
            let count = FileSessionStore::count_sessions_sync(&root_dir)?;
            if count >= max_sessions {
                return Err(AgentError::Config(format!(
                    "Session limit reached: {} of {} maximum",
                    count, max_sessions
                )));
            }

            let id = SessionId::new();
            let dir = root_dir.join(&id.0);
            fs::create_dir_all(&dir)?;

            let metadata = SessionMetadata::new(id, &task);

            // Save metadata
            let path = dir.join("metadata.json");
            let json = serde_json::to_string_pretty(&metadata)?;
            fs::write(path, json)?;

            // Initialize empty messages.jsonl
            let messages_path = dir.join("messages.jsonl");
            fs::File::create(&messages_path)?;

            // Initialize empty steps.json
            let steps_path = dir.join("steps.json");
            fs::write(&steps_path, "[]")?;

            Ok(metadata)
        })
        .await
    }

    async fn save_metadata(&self, metadata: &SessionMetadata) -> Result<(), AgentError> {
        let dir = self.session_dir(&metadata.id);
        let metadata = metadata.clone();

        blocking(move || {
            if !dir.exists() {
                return Err(AgentError::Config(format!(
                    "Session directory not found: {}",
                    metadata.id
                )));
            }
            let path = dir.join("metadata.json");
            let json = serde_json::to_string_pretty(&metadata)?;
            fs::write(path, json)?;
            Ok(())
        })
        .await
    }

    async fn save_messages(
        &self,
        session_id: &SessionId,
        messages: &[Message],
    ) -> Result<(), AgentError> {
        let dir = self.session_dir(session_id);
        let messages = messages.to_vec();

        blocking(move || {
            if !dir.exists() {
                return Err(AgentError::Config(format!(
                    "Session directory not found: {}",
                    dir.display()
                )));
            }
            let path = dir.join("messages.jsonl");
            let mut file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(path)?;
            for msg in &messages {
                let line = serde_json::to_string(msg)?;
                writeln!(file, "{}", line)?;
            }
            Ok(())
        })
        .await
    }

    async fn save_steps(
        &self,
        session_id: &SessionId,
        steps: &[StepRecord],
    ) -> Result<(), AgentError> {
        let dir = self.session_dir(session_id);
        let steps = steps.to_vec();

        blocking(move || {
            if !dir.exists() {
                return Err(AgentError::Config(format!(
                    "Session directory not found: {}",
                    dir.display()
                )));
            }
            let path = dir.join("steps.json");
            let json = serde_json::to_string_pretty(&steps)?;
            fs::write(path, json)?;
            Ok(())
        })
        .await
    }

    async fn load(&self, session_id: &SessionId) -> Result<SessionSnapshot, AgentError> {
        let root_dir = self.root_dir.clone();
        let session_id = session_id.clone();

        blocking(move || {
            let dir = root_dir.join(&session_id.0);
            if !dir.exists() {
                return Err(AgentError::Config(format!(
                    "Session not found: {}",
                    session_id
                )));
            }

            // Load metadata
            let metadata_path = dir.join("metadata.json");
            if !metadata_path.exists() {
                return Err(AgentError::Config(format!(
                    "Session metadata not found: {}",
                    session_id
                )));
            }
            let content = fs::read_to_string(metadata_path)?;
            let metadata: SessionMetadata = serde_json::from_str(&content)?;

            // Load messages
            let messages_path = dir.join("messages.jsonl");
            let messages = load_messages_sync(&messages_path)?;

            // Load steps
            let steps = FileSessionStore::load_steps_sync(&dir)?;

            let iteration = compute_iteration(&steps);

            Ok(SessionSnapshot {
                metadata,
                messages,
                steps,
                system_prompt: None,
                iteration,
            })
        })
        .await
    }

    async fn load_metadata(&self, session_id: &SessionId) -> Result<SessionMetadata, AgentError> {
        let path = self.session_dir(session_id).join("metadata.json");

        blocking(move || {
            if !path.exists() {
                return Err(AgentError::Config(format!(
                    "Session metadata not found: {}",
                    path.display()
                )));
            }
            let content = fs::read_to_string(path)?;
            let metadata: SessionMetadata = serde_json::from_str(&content)?;
            Ok(metadata)
        })
        .await
    }

    async fn load_messages(&self, session_id: &SessionId) -> Result<Vec<Message>, AgentError> {
        let path = self.session_dir(session_id).join("messages.jsonl");

        blocking(move || load_messages_sync(&path)).await
    }

    async fn list(&self) -> Result<Vec<SessionMetadata>, AgentError> {
        let root_dir = self.root_dir.clone();

        blocking(move || {
            if !root_dir.exists() {
                return Ok(vec![]);
            }

            let mut sessions = Vec::new();
            for entry in fs::read_dir(&root_dir)? {
                let entry = entry?;
                if entry.file_type()?.is_dir() {
                    let metadata_path = entry.path().join("metadata.json");
                    if metadata_path.exists() {
                        let content = fs::read_to_string(&metadata_path)?;
                        if let Ok(metadata) = serde_json::from_str::<SessionMetadata>(&content) {
                            sessions.push(metadata);
                        }
                    }
                }
            }

            sessions.sort_by(|a, b| b.created_at.cmp(&a.created_at));

            Ok(sessions)
        })
        .await
    }

    async fn fork(&self, source_id: &SessionId) -> Result<SessionMetadata, AgentError> {
        let root_dir = self.root_dir.clone();
        let max_sessions = self.max_sessions;
        let source_id = source_id.clone();

        blocking(move || {
            let source_dir = root_dir.join(&source_id.0);
            if !source_dir.exists() {
                return Err(AgentError::Config(format!(
                    "Source session not found: {}",
                    source_id
                )));
            }

            let count = FileSessionStore::count_sessions_sync(&root_dir)?;
            if count >= max_sessions {
                return Err(AgentError::Config(format!(
                    "Session limit reached: {} of {} maximum",
                    count, max_sessions
                )));
            }

            // Load source metadata
            let metadata_path = source_dir.join("metadata.json");
            let content = fs::read_to_string(metadata_path)?;
            let source_metadata: SessionMetadata = serde_json::from_str(&content)?;

            let new_id = SessionId::new();
            let new_dir = root_dir.join(&new_id.0);
            fs::create_dir_all(&new_dir)?;

            // Copy messages.jsonl
            let source_messages = source_dir.join("messages.jsonl");
            let dest_messages = new_dir.join("messages.jsonl");
            if source_messages.exists() {
                fs::copy(&source_messages, &dest_messages)?;
            } else {
                fs::File::create(&dest_messages)?;
            }

            // Copy steps.json
            let source_steps = source_dir.join("steps.json");
            let dest_steps = new_dir.join("steps.json");
            if source_steps.exists() {
                fs::copy(&source_steps, &dest_steps)?;
            } else {
                fs::write(&dest_steps, "[]")?;
            }

            // Create new metadata with parent reference
            let now = Utc::now();
            let new_metadata = SessionMetadata {
                id: new_id,
                created_at: now,
                updated_at: now,
                task: source_metadata.task,
                status: SessionStatus::InProgress,
                total_input_tokens: source_metadata.total_input_tokens,
                total_output_tokens: source_metadata.total_output_tokens,
                parent_session_id: Some(source_id),
            };

            // Save metadata
            let path = new_dir.join("metadata.json");
            let json = serde_json::to_string_pretty(&new_metadata)?;
            fs::write(path, json)?;

            Ok(new_metadata)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::types::{ContentBlock, Role};
    use serde_json::json;
    use tempfile::TempDir;

    fn make_store(tmp: &TempDir) -> FileSessionStore {
        FileSessionStore::new(tmp.path().to_path_buf(), 100)
    }

    fn sample_message(text: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
        }
    }

    fn sample_step(iteration: u32) -> StepRecord {
        StepRecord {
            iteration,
            tool: "navigate".to_string(),
            input: json!({"url": "https://example.com"}),
            output: json!({"success": true}),
            duration_ms: 500,
            is_error: None,
        }
    }

    #[tokio::test]
    async fn test_create_generates_metadata() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);
        let metadata = store.create("Test task").await.unwrap();

        assert!(!metadata.id.0.is_empty());
        assert_eq!(metadata.task, "Test task");
        assert_eq!(metadata.status, SessionStatus::InProgress);
        assert!(metadata.total_input_tokens.is_none());
        assert!(metadata.total_output_tokens.is_none());
        assert!(metadata.parent_session_id.is_none());
    }

    #[tokio::test]
    async fn test_create_initializes_files() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);
        let metadata = store.create("Test task").await.unwrap();

        let dir = tmp.path().join(&metadata.id.0);
        assert!(dir.join("metadata.json").exists());
        assert!(dir.join("messages.jsonl").exists());
        assert!(dir.join("steps.json").exists());
    }

    #[tokio::test]
    async fn test_create_respects_max_sessions() {
        let tmp = TempDir::new().unwrap();
        let store = FileSessionStore::new(tmp.path().to_path_buf(), 2);

        store.create("Task 1").await.unwrap();
        store.create("Task 2").await.unwrap();
        let result = store.create("Task 3").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Session limit"));
    }

    #[tokio::test]
    async fn test_save_and_load_metadata_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);
        let mut metadata = store.create("Test task").await.unwrap();

        metadata.status = SessionStatus::Completed;
        metadata.total_input_tokens = Some(500);
        metadata.total_output_tokens = Some(200);
        store.save_metadata(&metadata).await.unwrap();

        let loaded = store.load_metadata(&metadata.id).await.unwrap();
        assert_eq!(loaded.id, metadata.id);
        assert_eq!(loaded.status, SessionStatus::Completed);
        assert_eq!(loaded.total_input_tokens, Some(500));
        assert_eq!(loaded.total_output_tokens, Some(200));
    }

    #[tokio::test]
    async fn test_save_messages_creates_jsonl() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);
        let metadata = store.create("Test task").await.unwrap();

        let messages = vec![sample_message("Hello"), sample_message("How are you?")];
        store.save_messages(&metadata.id, &messages).await.unwrap();

        // Verify file contents
        let path = tmp.path().join(&metadata.id.0).join("messages.jsonl");
        let content = fs::read_to_string(path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);

        // Each line should be valid JSON
        let msg1: Message = serde_json::from_str(lines[0]).unwrap();
        assert!(matches!(
            &msg1.content[0],
            ContentBlock::Text { text } if text == "Hello"
        ));
    }

    #[tokio::test]
    async fn test_save_messages_truncates_on_full_write() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);
        let metadata = store.create("Test task").await.unwrap();

        store
            .save_messages(&metadata.id, &[sample_message("First")])
            .await
            .unwrap();
        // Second call with only one message should truncate the file, not append
        store
            .save_messages(&metadata.id, &[sample_message("Second")])
            .await
            .unwrap();

        let path = tmp.path().join(&metadata.id.0).join("messages.jsonl");
        let content = fs::read_to_string(path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        // Truncate mode means only the last write's messages remain
        assert_eq!(lines.len(), 1);

        let msg: Message = serde_json::from_str(lines[0]).unwrap();
        assert!(matches!(
            &msg.content[0],
            ContentBlock::Text { text } if text == "Second"
        ));
    }

    #[tokio::test]
    async fn test_save_and_load_steps() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);
        let metadata = store.create("Test task").await.unwrap();

        let steps = vec![sample_step(1), sample_step(2)];
        store.save_steps(&metadata.id, &steps).await.unwrap();

        let snapshot = store.load(&metadata.id).await.unwrap();
        assert_eq!(snapshot.steps.len(), 2);
        assert_eq!(snapshot.steps[0].iteration, 1);
        assert_eq!(snapshot.steps[1].iteration, 2);
    }

    #[tokio::test]
    async fn test_load_full_snapshot() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);
        let metadata = store.create("Navigate to example.com").await.unwrap();

        store
            .save_messages(&metadata.id, &[sample_message("Go to example.com")])
            .await
            .unwrap();
        store
            .save_steps(&metadata.id, &[sample_step(1)])
            .await
            .unwrap();

        let snapshot = store.load(&metadata.id).await.unwrap();
        assert_eq!(snapshot.metadata.task, "Navigate to example.com");
        assert_eq!(snapshot.messages.len(), 1);
        assert_eq!(snapshot.steps.len(), 1);
        assert_eq!(snapshot.iteration, 1);
        assert!(snapshot.system_prompt.is_none());
    }

    #[tokio::test]
    async fn test_load_snapshot_empty_session() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);
        let metadata = store.create("Empty task").await.unwrap();

        let snapshot = store.load(&metadata.id).await.unwrap();
        assert!(snapshot.messages.is_empty());
        assert!(snapshot.steps.is_empty());
        assert_eq!(snapshot.iteration, 0);
    }

    #[tokio::test]
    async fn test_list_sessions() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);

        store.create("Task 1").await.unwrap();
        store.create("Task 2").await.unwrap();
        store.create("Task 3").await.unwrap();

        let sessions = store.list().await.unwrap();
        assert_eq!(sessions.len(), 3);
    }

    #[tokio::test]
    async fn test_list_sessions_empty() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);

        let sessions = store.list().await.unwrap();
        assert!(sessions.is_empty());
    }

    #[tokio::test]
    async fn test_list_sessions_nonexistent_root() {
        let store = FileSessionStore::new(PathBuf::from("/nonexistent/path"), 100);
        let sessions = store.list().await.unwrap();
        assert!(sessions.is_empty());
    }

    #[tokio::test]
    async fn test_fork_creates_new_session() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);
        let original = store.create("Original task").await.unwrap();

        store
            .save_messages(&original.id, &[sample_message("Hello")])
            .await
            .unwrap();
        store
            .save_steps(&original.id, &[sample_step(1)])
            .await
            .unwrap();

        let forked = store.fork(&original.id).await.unwrap();
        assert_ne!(forked.id, original.id);
        assert_eq!(forked.task, "Original task");
        assert_eq!(forked.parent_session_id, Some(original.id.clone()));
        assert_eq!(forked.status, SessionStatus::InProgress);
    }

    #[tokio::test]
    async fn test_fork_copies_messages_and_steps() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);
        let original = store.create("Original task").await.unwrap();

        store
            .save_messages(
                &original.id,
                &[sample_message("Hello"), sample_message("World")],
            )
            .await
            .unwrap();
        store
            .save_steps(&original.id, &[sample_step(1), sample_step(2)])
            .await
            .unwrap();

        let forked = store.fork(&original.id).await.unwrap();
        let forked_snapshot = store.load(&forked.id).await.unwrap();
        assert_eq!(forked_snapshot.messages.len(), 2);
        assert_eq!(forked_snapshot.steps.len(), 2);
    }

    #[tokio::test]
    async fn test_fork_preserves_token_counts() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);
        let mut original = store.create("Original task").await.unwrap();
        original.total_input_tokens = Some(1000);
        original.total_output_tokens = Some(500);
        store.save_metadata(&original).await.unwrap();

        let forked = store.fork(&original.id).await.unwrap();
        assert_eq!(forked.total_input_tokens, Some(1000));
        assert_eq!(forked.total_output_tokens, Some(500));
    }

    #[tokio::test]
    async fn test_load_nonexistent_session() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);
        let fake_id = SessionId("nonexistent".to_string());

        let result = store.load(&fake_id).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_load_metadata_nonexistent() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);
        let fake_id = SessionId("nonexistent".to_string());

        let result = store.load_metadata(&fake_id).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_save_messages_nonexistent_session() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);
        let fake_id = SessionId("nonexistent".to_string());

        let result = store
            .save_messages(&fake_id, &[sample_message("Hello")])
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_save_steps_nonexistent_session() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);
        let fake_id = SessionId("nonexistent".to_string());

        let result = store.save_steps(&fake_id, &[sample_step(1)]).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_fork_nonexistent_source() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);
        let fake_id = SessionId("nonexistent".to_string());

        let result = store.fork(&fake_id).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_save_metadata_nonexistent_directory() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);
        let metadata = SessionMetadata {
            id: SessionId("nonexistent".to_string()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            task: "Test".to_string(),
            status: SessionStatus::InProgress,
            total_input_tokens: None,
            total_output_tokens: None,
            parent_session_id: None,
        };

        let result = store.save_metadata(&metadata).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_load_messages() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);
        let metadata = store.create("Test task").await.unwrap();

        let messages = vec![sample_message("Hello"), sample_message("World")];
        store.save_messages(&metadata.id, &messages).await.unwrap();

        let loaded = store.load_messages(&metadata.id).await.unwrap();
        assert_eq!(loaded.len(), 2);
    }

    #[tokio::test]
    async fn test_load_messages_empty() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);
        let metadata = store.create("Test task").await.unwrap();

        let loaded = store.load_messages(&metadata.id).await.unwrap();
        assert!(loaded.is_empty());
    }

    /// Verify that FileSessionStore can be used as a `dyn SessionStorage` trait object.
    #[tokio::test]
    async fn test_file_session_store_as_dyn_session_storage() {
        let tmp = TempDir::new().unwrap();
        let store = FileSessionStore::new(tmp.path().to_path_buf(), 100);
        let dyn_store: &dyn SessionStorage = &store;

        let metadata = dyn_store.create("Test via trait object").await.unwrap();
        assert_eq!(metadata.task, "Test via trait object");

        let loaded = dyn_store.load_metadata(&metadata.id).await.unwrap();
        assert_eq!(loaded.task, "Test via trait object");
    }

    // --- Shared backend contract ---
    //
    // These run the same assertions the Postgres backend must satisfy. The Postgres
    // store duplicated the entire conversation on every iteration for want of exactly
    // this check.

    #[tokio::test]
    async fn test_conformance_save_messages_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);
        crate::session::conformance::assert_save_messages_is_idempotent(&store).await;
    }

    #[tokio::test]
    async fn test_conformance_save_messages_replaces_history() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);
        crate::session::conformance::assert_save_messages_replaces_history(&store).await;
    }

    #[tokio::test]
    async fn test_conformance_save_steps_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);
        crate::session::conformance::assert_save_steps_is_idempotent(&store).await;
    }
}
