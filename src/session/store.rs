use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use chrono::Utc;

use crate::error::AgentError;
use crate::llm::types::Message;
use crate::output::result::StepRecord;

use super::types::{SessionId, SessionMetadata, SessionSnapshot, SessionStatus};

pub struct SessionStore {
    root_dir: PathBuf,
    max_sessions: usize,
}

impl SessionStore {
    pub fn new(root_dir: PathBuf, max_sessions: usize) -> Self {
        Self {
            root_dir,
            max_sessions,
        }
    }

    /// Create a new session directory and initialize it.
    pub fn create(&self, task: &str) -> Result<SessionMetadata, AgentError> {
        let session_count = self.count_sessions()?;
        if session_count >= self.max_sessions {
            return Err(AgentError::Config(format!(
                "Session limit reached: {} of {} maximum",
                session_count, self.max_sessions
            )));
        }

        let id = SessionId::new();
        let dir = self.session_dir(&id);
        fs::create_dir_all(&dir)?;

        let now = Utc::now();
        let metadata = SessionMetadata {
            id,
            created_at: now,
            updated_at: now,
            task: task.to_string(),
            status: SessionStatus::InProgress,
            total_input_tokens: None,
            total_output_tokens: None,
            parent_session_id: None,
        };

        self.save_metadata(&metadata)?;

        // Initialize empty messages.jsonl
        let messages_path = dir.join("messages.jsonl");
        fs::File::create(&messages_path)?;

        // Initialize empty steps.json
        let steps_path = dir.join("steps.json");
        fs::write(&steps_path, "[]")?;

        Ok(metadata)
    }

    /// Save/update session metadata.
    pub fn save_metadata(&self, metadata: &SessionMetadata) -> Result<(), AgentError> {
        let dir = self.session_dir(&metadata.id);
        if !dir.exists() {
            return Err(AgentError::Config(format!(
                "Session directory not found: {}",
                metadata.id
            )));
        }
        let path = dir.join("metadata.json");
        let json = serde_json::to_string_pretty(metadata)?;
        fs::write(path, json)?;
        Ok(())
    }

    /// Append messages to the session's messages.jsonl (append-only).
    pub fn append_messages(
        &self,
        session_id: &SessionId,
        messages: &[Message],
    ) -> Result<(), AgentError> {
        let dir = self.session_dir(session_id);
        if !dir.exists() {
            return Err(AgentError::Config(format!(
                "Session directory not found: {}",
                session_id
            )));
        }
        let path = dir.join("messages.jsonl");
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        for msg in messages {
            let line = serde_json::to_string(msg)?;
            writeln!(file, "{}", line)?;
        }
        Ok(())
    }

    /// Save steps to the session.
    pub fn save_steps(
        &self,
        session_id: &SessionId,
        steps: &[StepRecord],
    ) -> Result<(), AgentError> {
        let dir = self.session_dir(session_id);
        if !dir.exists() {
            return Err(AgentError::Config(format!(
                "Session directory not found: {}",
                session_id
            )));
        }
        let path = dir.join("steps.json");
        let json = serde_json::to_string_pretty(steps)?;
        fs::write(path, json)?;
        Ok(())
    }

    /// Load a complete session snapshot.
    pub fn load(&self, session_id: &SessionId) -> Result<SessionSnapshot, AgentError> {
        let dir = self.session_dir(session_id);
        if !dir.exists() {
            return Err(AgentError::Config(format!(
                "Session not found: {}",
                session_id
            )));
        }

        let metadata = self.load_metadata(session_id)?;
        let messages = self.load_messages(session_id)?;
        let steps = self.load_steps(session_id)?;

        // Determine iteration from steps
        let iteration = steps.iter().map(|s| s.iteration).max().unwrap_or(0);

        Ok(SessionSnapshot {
            metadata,
            messages,
            steps,
            system_prompt: None,
            iteration,
        })
    }

    /// Load just metadata.
    pub fn load_metadata(&self, session_id: &SessionId) -> Result<SessionMetadata, AgentError> {
        let path = self.session_dir(session_id).join("metadata.json");
        if !path.exists() {
            return Err(AgentError::Config(format!(
                "Session metadata not found: {}",
                session_id
            )));
        }
        let content = fs::read_to_string(path)?;
        let metadata: SessionMetadata = serde_json::from_str(&content)?;
        Ok(metadata)
    }

    /// List all sessions (metadata only).
    pub fn list(&self) -> Result<Vec<SessionMetadata>, AgentError> {
        if !self.root_dir.exists() {
            return Ok(vec![]);
        }

        let mut sessions = Vec::new();
        for entry in fs::read_dir(&self.root_dir)? {
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

        // Sort by created_at descending (most recent first)
        sessions.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        Ok(sessions)
    }

    /// Fork a session - creates a new session with copied state.
    pub fn fork(&self, source_id: &SessionId) -> Result<SessionMetadata, AgentError> {
        let source_dir = self.session_dir(source_id);
        if !source_dir.exists() {
            return Err(AgentError::Config(format!(
                "Source session not found: {}",
                source_id
            )));
        }

        let session_count = self.count_sessions()?;
        if session_count >= self.max_sessions {
            return Err(AgentError::Config(format!(
                "Session limit reached: {} of {} maximum",
                session_count, self.max_sessions
            )));
        }

        let source_metadata = self.load_metadata(source_id)?;

        let new_id = SessionId::new();
        let new_dir = self.session_dir(&new_id);
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
            parent_session_id: Some(source_id.clone()),
        };

        self.save_metadata(&new_metadata)?;

        Ok(new_metadata)
    }

    fn session_dir(&self, session_id: &SessionId) -> PathBuf {
        self.root_dir.join(&session_id.0)
    }

    fn load_messages(&self, session_id: &SessionId) -> Result<Vec<Message>, AgentError> {
        let path = self.session_dir(session_id).join("messages.jsonl");
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

    fn load_steps(&self, session_id: &SessionId) -> Result<Vec<StepRecord>, AgentError> {
        let path = self.session_dir(session_id).join("steps.json");
        if !path.exists() {
            return Ok(vec![]);
        }
        let content = fs::read_to_string(path)?;
        let steps: Vec<StepRecord> = serde_json::from_str(&content)?;
        Ok(steps)
    }

    fn count_sessions(&self) -> Result<usize, AgentError> {
        if !self.root_dir.exists() {
            return Ok(0);
        }
        let count = fs::read_dir(&self.root_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
            .count();
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::types::{ContentBlock, Role};
    use serde_json::json;
    use tempfile::TempDir;

    fn make_store(tmp: &TempDir) -> SessionStore {
        SessionStore::new(tmp.path().to_path_buf(), 100)
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

    #[test]
    fn test_create_generates_metadata() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);
        let metadata = store.create("Test task").unwrap();

        assert!(!metadata.id.0.is_empty());
        assert_eq!(metadata.task, "Test task");
        assert_eq!(metadata.status, SessionStatus::InProgress);
        assert!(metadata.total_input_tokens.is_none());
        assert!(metadata.total_output_tokens.is_none());
        assert!(metadata.parent_session_id.is_none());
    }

    #[test]
    fn test_create_initializes_files() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);
        let metadata = store.create("Test task").unwrap();

        let dir = tmp.path().join(&metadata.id.0);
        assert!(dir.join("metadata.json").exists());
        assert!(dir.join("messages.jsonl").exists());
        assert!(dir.join("steps.json").exists());
    }

    #[test]
    fn test_create_respects_max_sessions() {
        let tmp = TempDir::new().unwrap();
        let store = SessionStore::new(tmp.path().to_path_buf(), 2);

        store.create("Task 1").unwrap();
        store.create("Task 2").unwrap();
        let result = store.create("Task 3");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Session limit"));
    }

    #[test]
    fn test_save_and_load_metadata_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);
        let mut metadata = store.create("Test task").unwrap();

        metadata.status = SessionStatus::Completed;
        metadata.total_input_tokens = Some(500);
        metadata.total_output_tokens = Some(200);
        store.save_metadata(&metadata).unwrap();

        let loaded = store.load_metadata(&metadata.id).unwrap();
        assert_eq!(loaded.id, metadata.id);
        assert_eq!(loaded.status, SessionStatus::Completed);
        assert_eq!(loaded.total_input_tokens, Some(500));
        assert_eq!(loaded.total_output_tokens, Some(200));
    }

    #[test]
    fn test_append_messages_creates_jsonl() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);
        let metadata = store.create("Test task").unwrap();

        let messages = vec![sample_message("Hello"), sample_message("How are you?")];
        store.append_messages(&metadata.id, &messages).unwrap();

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

    #[test]
    fn test_append_messages_appends_to_existing() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);
        let metadata = store.create("Test task").unwrap();

        store
            .append_messages(&metadata.id, &[sample_message("First")])
            .unwrap();
        store
            .append_messages(&metadata.id, &[sample_message("Second")])
            .unwrap();

        let path = tmp.path().join(&metadata.id.0).join("messages.jsonl");
        let content = fs::read_to_string(path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn test_save_and_load_steps() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);
        let metadata = store.create("Test task").unwrap();

        let steps = vec![sample_step(1), sample_step(2)];
        store.save_steps(&metadata.id, &steps).unwrap();

        let snapshot = store.load(&metadata.id).unwrap();
        assert_eq!(snapshot.steps.len(), 2);
        assert_eq!(snapshot.steps[0].iteration, 1);
        assert_eq!(snapshot.steps[1].iteration, 2);
    }

    #[test]
    fn test_load_full_snapshot() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);
        let metadata = store.create("Navigate to example.com").unwrap();

        store
            .append_messages(&metadata.id, &[sample_message("Go to example.com")])
            .unwrap();
        store.save_steps(&metadata.id, &[sample_step(1)]).unwrap();

        let snapshot = store.load(&metadata.id).unwrap();
        assert_eq!(snapshot.metadata.task, "Navigate to example.com");
        assert_eq!(snapshot.messages.len(), 1);
        assert_eq!(snapshot.steps.len(), 1);
        assert_eq!(snapshot.iteration, 1);
        assert!(snapshot.system_prompt.is_none());
    }

    #[test]
    fn test_load_snapshot_empty_session() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);
        let metadata = store.create("Empty task").unwrap();

        let snapshot = store.load(&metadata.id).unwrap();
        assert!(snapshot.messages.is_empty());
        assert!(snapshot.steps.is_empty());
        assert_eq!(snapshot.iteration, 0);
    }

    #[test]
    fn test_list_sessions() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);

        store.create("Task 1").unwrap();
        store.create("Task 2").unwrap();
        store.create("Task 3").unwrap();

        let sessions = store.list().unwrap();
        assert_eq!(sessions.len(), 3);
    }

    #[test]
    fn test_list_sessions_empty() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);

        let sessions = store.list().unwrap();
        assert!(sessions.is_empty());
    }

    #[test]
    fn test_list_sessions_nonexistent_root() {
        let store = SessionStore::new(PathBuf::from("/nonexistent/path"), 100);
        let sessions = store.list().unwrap();
        assert!(sessions.is_empty());
    }

    #[test]
    fn test_fork_creates_new_session() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);
        let original = store.create("Original task").unwrap();

        store
            .append_messages(&original.id, &[sample_message("Hello")])
            .unwrap();
        store.save_steps(&original.id, &[sample_step(1)]).unwrap();

        let forked = store.fork(&original.id).unwrap();
        assert_ne!(forked.id, original.id);
        assert_eq!(forked.task, "Original task");
        assert_eq!(forked.parent_session_id, Some(original.id.clone()));
        assert_eq!(forked.status, SessionStatus::InProgress);
    }

    #[test]
    fn test_fork_copies_messages_and_steps() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);
        let original = store.create("Original task").unwrap();

        store
            .append_messages(
                &original.id,
                &[sample_message("Hello"), sample_message("World")],
            )
            .unwrap();
        store
            .save_steps(&original.id, &[sample_step(1), sample_step(2)])
            .unwrap();

        let forked = store.fork(&original.id).unwrap();
        let forked_snapshot = store.load(&forked.id).unwrap();
        assert_eq!(forked_snapshot.messages.len(), 2);
        assert_eq!(forked_snapshot.steps.len(), 2);
    }

    #[test]
    fn test_fork_preserves_token_counts() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);
        let mut original = store.create("Original task").unwrap();
        original.total_input_tokens = Some(1000);
        original.total_output_tokens = Some(500);
        store.save_metadata(&original).unwrap();

        let forked = store.fork(&original.id).unwrap();
        assert_eq!(forked.total_input_tokens, Some(1000));
        assert_eq!(forked.total_output_tokens, Some(500));
    }

    #[test]
    fn test_load_nonexistent_session() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);
        let fake_id = SessionId("nonexistent".to_string());

        let result = store.load(&fake_id);
        assert!(result.is_err());
    }

    #[test]
    fn test_load_metadata_nonexistent() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);
        let fake_id = SessionId("nonexistent".to_string());

        let result = store.load_metadata(&fake_id);
        assert!(result.is_err());
    }

    #[test]
    fn test_append_messages_nonexistent_session() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);
        let fake_id = SessionId("nonexistent".to_string());

        let result = store.append_messages(&fake_id, &[sample_message("Hello")]);
        assert!(result.is_err());
    }

    #[test]
    fn test_save_steps_nonexistent_session() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);
        let fake_id = SessionId("nonexistent".to_string());

        let result = store.save_steps(&fake_id, &[sample_step(1)]);
        assert!(result.is_err());
    }

    #[test]
    fn test_fork_nonexistent_source() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp);
        let fake_id = SessionId("nonexistent".to_string());

        let result = store.fork(&fake_id);
        assert!(result.is_err());
    }

    #[test]
    fn test_save_metadata_nonexistent_directory() {
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

        let result = store.save_metadata(&metadata);
        assert!(result.is_err());
    }
}
