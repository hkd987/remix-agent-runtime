use async_trait::async_trait;
use chrono::Utc;
use sqlx::PgPool;

use crate::error::AgentError;
use crate::llm::types::Message;
use crate::output::result::StepRecord;

use super::traits::SessionStorage;
use super::types::{SessionId, SessionMetadata, SessionSnapshot, SessionStatus};

/// PostgreSQL-backed session storage.
///
/// Stores sessions, messages, and steps in three tables:
/// - `sessions` -- one row per session (metadata).
/// - `session_messages` -- append-only message log per session.
/// - `session_steps` -- JSONB array of step records per session.
pub struct PostgresSessionStore {
    pool: PgPool,
    max_sessions: usize,
}

impl PostgresSessionStore {
    pub fn new(pool: PgPool, max_sessions: usize) -> Self {
        Self { pool, max_sessions }
    }

    /// Run database migrations to create the required tables.
    pub async fn migrate(&self) -> Result<(), AgentError> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                created_at TIMESTAMPTZ NOT NULL,
                updated_at TIMESTAMPTZ NOT NULL,
                task TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'in_progress',
                total_input_tokens INTEGER,
                total_output_tokens INTEGER,
                parent_session_id TEXT REFERENCES sessions(id)
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| AgentError::Session(e.to_string()))?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS session_messages (
                id SERIAL PRIMARY KEY,
                session_id TEXT NOT NULL REFERENCES sessions(id),
                message JSONB NOT NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| AgentError::Session(e.to_string()))?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS session_steps (
                session_id TEXT PRIMARY KEY REFERENCES sessions(id),
                steps JSONB NOT NULL DEFAULT '[]'::jsonb
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| AgentError::Session(e.to_string()))?;

        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_session_messages_session_id
                ON session_messages(session_id)
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| AgentError::Session(e.to_string()))?;

        Ok(())
    }

    /// Serialize a `SessionStatus` to the snake_case string stored in the DB.
    fn status_to_str(status: &SessionStatus) -> &'static str {
        match status {
            SessionStatus::InProgress => "in_progress",
            SessionStatus::Completed => "completed",
            SessionStatus::Error => "error",
            SessionStatus::Paused => "paused",
        }
    }

    /// Deserialize a status string from the DB back to `SessionStatus`.
    fn str_to_status(s: &str) -> Result<SessionStatus, AgentError> {
        match s {
            "in_progress" => Ok(SessionStatus::InProgress),
            "completed" => Ok(SessionStatus::Completed),
            "error" => Ok(SessionStatus::Error),
            "paused" => Ok(SessionStatus::Paused),
            other => Err(AgentError::Session(format!(
                "Unknown session status: {}",
                other
            ))),
        }
    }

    /// Build a `SessionMetadata` from a raw database row.
    fn row_to_metadata(row: &sqlx::postgres::PgRow) -> Result<SessionMetadata, AgentError> {
        use sqlx::Row;

        let id: String = row
            .try_get("id")
            .map_err(|e| AgentError::Session(e.to_string()))?;
        let created_at = row
            .try_get("created_at")
            .map_err(|e| AgentError::Session(e.to_string()))?;
        let updated_at = row
            .try_get("updated_at")
            .map_err(|e| AgentError::Session(e.to_string()))?;
        let task: String = row
            .try_get("task")
            .map_err(|e| AgentError::Session(e.to_string()))?;
        let status_str: String = row
            .try_get("status")
            .map_err(|e| AgentError::Session(e.to_string()))?;
        let total_input_tokens: Option<i32> = row
            .try_get("total_input_tokens")
            .map_err(|e| AgentError::Session(e.to_string()))?;
        let total_output_tokens: Option<i32> = row
            .try_get("total_output_tokens")
            .map_err(|e| AgentError::Session(e.to_string()))?;
        let parent_session_id: Option<String> = row
            .try_get("parent_session_id")
            .map_err(|e| AgentError::Session(e.to_string()))?;

        Ok(SessionMetadata {
            id: SessionId(id),
            created_at,
            updated_at,
            task,
            status: Self::str_to_status(&status_str)?,
            total_input_tokens: total_input_tokens.map(|v| v as u32),
            total_output_tokens: total_output_tokens.map(|v| v as u32),
            parent_session_id: parent_session_id.map(SessionId),
        })
    }
}

#[async_trait]
impl SessionStorage for PostgresSessionStore {
    async fn create(&self, task: &str) -> Result<SessionMetadata, AgentError> {
        // Check session count
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AgentError::Session(e.to_string()))?;

        if count as usize >= self.max_sessions {
            return Err(AgentError::Session(format!(
                "Session limit reached: {} of {} maximum",
                count, self.max_sessions
            )));
        }

        let id = SessionId::new();
        let now = Utc::now();
        let status_str = Self::status_to_str(&SessionStatus::InProgress);

        sqlx::query(
            r#"
            INSERT INTO sessions (id, created_at, updated_at, task, status)
            VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(&id.0)
        .bind(now)
        .bind(now)
        .bind(task)
        .bind(status_str)
        .execute(&self.pool)
        .await
        .map_err(|e| AgentError::Session(e.to_string()))?;

        // Initialize empty steps row
        sqlx::query(
            r#"
            INSERT INTO session_steps (session_id, steps)
            VALUES ($1, '[]'::jsonb)
            "#,
        )
        .bind(&id.0)
        .execute(&self.pool)
        .await
        .map_err(|e| AgentError::Session(e.to_string()))?;

        Ok(SessionMetadata {
            id,
            created_at: now,
            updated_at: now,
            task: task.to_string(),
            status: SessionStatus::InProgress,
            total_input_tokens: None,
            total_output_tokens: None,
            parent_session_id: None,
        })
    }

    async fn save_metadata(&self, metadata: &SessionMetadata) -> Result<(), AgentError> {
        let status_str = Self::status_to_str(&metadata.status);

        let result = sqlx::query(
            r#"
            UPDATE sessions
            SET updated_at = $1,
                task = $2,
                status = $3,
                total_input_tokens = $4,
                total_output_tokens = $5,
                parent_session_id = $6
            WHERE id = $7
            "#,
        )
        .bind(metadata.updated_at)
        .bind(&metadata.task)
        .bind(status_str)
        .bind(metadata.total_input_tokens.map(|v| v as i32))
        .bind(metadata.total_output_tokens.map(|v| v as i32))
        .bind(metadata.parent_session_id.as_ref().map(|id| &id.0))
        .bind(&metadata.id.0)
        .execute(&self.pool)
        .await
        .map_err(|e| AgentError::Session(e.to_string()))?;

        if result.rows_affected() == 0 {
            return Err(AgentError::Session(format!(
                "Session not found: {}",
                metadata.id
            )));
        }

        Ok(())
    }

    async fn append_messages(
        &self,
        session_id: &SessionId,
        messages: &[Message],
    ) -> Result<(), AgentError> {
        for msg in messages {
            let json_value =
                serde_json::to_value(msg).map_err(|e| AgentError::Session(e.to_string()))?;

            sqlx::query(
                r#"
                INSERT INTO session_messages (session_id, message)
                VALUES ($1, $2)
                "#,
            )
            .bind(&session_id.0)
            .bind(json_value)
            .execute(&self.pool)
            .await
            .map_err(|e| AgentError::Session(e.to_string()))?;
        }

        Ok(())
    }

    async fn save_steps(
        &self,
        session_id: &SessionId,
        steps: &[StepRecord],
    ) -> Result<(), AgentError> {
        let json_value =
            serde_json::to_value(steps).map_err(|e| AgentError::Session(e.to_string()))?;

        sqlx::query(
            r#"
            INSERT INTO session_steps (session_id, steps)
            VALUES ($1, $2)
            ON CONFLICT (session_id)
            DO UPDATE SET steps = $2
            "#,
        )
        .bind(&session_id.0)
        .bind(json_value)
        .execute(&self.pool)
        .await
        .map_err(|e| AgentError::Session(e.to_string()))?;

        Ok(())
    }

    async fn load(&self, session_id: &SessionId) -> Result<SessionSnapshot, AgentError> {
        let metadata = self.load_metadata(session_id).await?;
        let messages = self.load_messages(session_id).await?;

        // Load steps
        let steps_row: Option<sqlx::postgres::PgRow> =
            sqlx::query("SELECT steps FROM session_steps WHERE session_id = $1")
                .bind(&session_id.0)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| AgentError::Session(e.to_string()))?;

        let steps: Vec<StepRecord> = match steps_row {
            Some(row) => {
                use sqlx::Row;
                let json_value: serde_json::Value = row
                    .try_get("steps")
                    .map_err(|e| AgentError::Session(e.to_string()))?;
                serde_json::from_value(json_value)
                    .map_err(|e| AgentError::Session(e.to_string()))?
            }
            None => Vec::new(),
        };

        let iteration = steps.iter().map(|s| s.iteration).max().unwrap_or(0);

        Ok(SessionSnapshot {
            metadata,
            messages,
            steps,
            system_prompt: None,
            iteration,
        })
    }

    async fn load_metadata(&self, session_id: &SessionId) -> Result<SessionMetadata, AgentError> {
        let row: sqlx::postgres::PgRow = sqlx::query("SELECT * FROM sessions WHERE id = $1")
            .bind(&session_id.0)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| AgentError::Session(e.to_string()))?
            .ok_or_else(|| AgentError::Session(format!("Session not found: {}", session_id)))?;

        Self::row_to_metadata(&row)
    }

    async fn load_messages(&self, session_id: &SessionId) -> Result<Vec<Message>, AgentError> {
        let rows: Vec<sqlx::postgres::PgRow> = sqlx::query(
            "SELECT message FROM session_messages WHERE session_id = $1 ORDER BY id",
        )
        .bind(&session_id.0)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AgentError::Session(e.to_string()))?;

        let mut messages = Vec::with_capacity(rows.len());
        for row in &rows {
            use sqlx::Row;
            let json_value: serde_json::Value = row
                .try_get("message")
                .map_err(|e| AgentError::Session(e.to_string()))?;
            let msg: Message = serde_json::from_value(json_value)
                .map_err(|e| AgentError::Session(e.to_string()))?;
            messages.push(msg);
        }

        Ok(messages)
    }

    async fn list(&self) -> Result<Vec<SessionMetadata>, AgentError> {
        let rows: Vec<sqlx::postgres::PgRow> =
            sqlx::query("SELECT * FROM sessions ORDER BY created_at DESC LIMIT $1")
                .bind(self.max_sessions as i64)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| AgentError::Session(e.to_string()))?;

        let mut sessions = Vec::with_capacity(rows.len());
        for row in &rows {
            sessions.push(Self::row_to_metadata(row)?);
        }

        Ok(sessions)
    }

    async fn fork(&self, source_id: &SessionId) -> Result<SessionMetadata, AgentError> {
        // Load source metadata
        let source = self.load_metadata(source_id).await?;

        // Check session limit
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AgentError::Session(e.to_string()))?;

        if count as usize >= self.max_sessions {
            return Err(AgentError::Session(format!(
                "Session limit reached: {} of {} maximum",
                count, self.max_sessions
            )));
        }

        let new_id = SessionId::new();
        let now = Utc::now();
        let status_str = Self::status_to_str(&SessionStatus::InProgress);

        // Insert new session row
        sqlx::query(
            r#"
            INSERT INTO sessions (id, created_at, updated_at, task, status,
                                  total_input_tokens, total_output_tokens, parent_session_id)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
        )
        .bind(&new_id.0)
        .bind(now)
        .bind(now)
        .bind(&source.task)
        .bind(status_str)
        .bind(source.total_input_tokens.map(|v| v as i32))
        .bind(source.total_output_tokens.map(|v| v as i32))
        .bind(&source_id.0)
        .execute(&self.pool)
        .await
        .map_err(|e| AgentError::Session(e.to_string()))?;

        // Copy messages
        sqlx::query(
            r#"
            INSERT INTO session_messages (session_id, message, created_at)
            SELECT $1, message, created_at
            FROM session_messages
            WHERE session_id = $2
            ORDER BY id
            "#,
        )
        .bind(&new_id.0)
        .bind(&source_id.0)
        .execute(&self.pool)
        .await
        .map_err(|e| AgentError::Session(e.to_string()))?;

        // Copy steps
        sqlx::query(
            r#"
            INSERT INTO session_steps (session_id, steps)
            SELECT $1, steps
            FROM session_steps
            WHERE session_id = $2
            ON CONFLICT (session_id) DO UPDATE SET steps = EXCLUDED.steps
            "#,
        )
        .bind(&new_id.0)
        .bind(&source_id.0)
        .execute(&self.pool)
        .await
        .map_err(|e| AgentError::Session(e.to_string()))?;

        Ok(SessionMetadata {
            id: new_id,
            created_at: now,
            updated_at: now,
            task: source.task,
            status: SessionStatus::InProgress,
            total_input_tokens: source.total_input_tokens,
            total_output_tokens: source.total_output_tokens,
            parent_session_id: Some(source_id.clone()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Requires running PostgreSQL
    async fn test_create_and_load_session() {
        let pool = PgPool::connect("postgres://localhost/remix_test")
            .await
            .unwrap();
        let store = PostgresSessionStore::new(pool, 100);
        store.migrate().await.unwrap();

        let metadata = store.create("Test task").await.unwrap();
        assert_eq!(metadata.task, "Test task");
        assert_eq!(metadata.status, SessionStatus::InProgress);
        assert!(metadata.total_input_tokens.is_none());
        assert!(metadata.parent_session_id.is_none());

        let loaded = store.load_metadata(&metadata.id).await.unwrap();
        assert_eq!(loaded.task, "Test task");
        assert_eq!(loaded.id, metadata.id);
    }

    #[tokio::test]
    #[ignore] // Requires running PostgreSQL
    async fn test_save_metadata_updates_row() {
        let pool = PgPool::connect("postgres://localhost/remix_test")
            .await
            .unwrap();
        let store = PostgresSessionStore::new(pool, 100);
        store.migrate().await.unwrap();

        let mut metadata = store.create("Update test").await.unwrap();
        metadata.status = SessionStatus::Completed;
        metadata.total_input_tokens = Some(500);
        metadata.total_output_tokens = Some(200);
        store.save_metadata(&metadata).await.unwrap();

        let loaded = store.load_metadata(&metadata.id).await.unwrap();
        assert_eq!(loaded.status, SessionStatus::Completed);
        assert_eq!(loaded.total_input_tokens, Some(500));
        assert_eq!(loaded.total_output_tokens, Some(200));
    }

    #[tokio::test]
    #[ignore] // Requires running PostgreSQL
    async fn test_append_and_load_messages() {
        use crate::llm::types::{ContentBlock, Role};

        let pool = PgPool::connect("postgres://localhost/remix_test")
            .await
            .unwrap();
        let store = PostgresSessionStore::new(pool, 100);
        store.migrate().await.unwrap();

        let metadata = store.create("Message test").await.unwrap();

        let messages = vec![
            Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "Hello".to_string(),
                }],
            },
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::Text {
                    text: "Hi there".to_string(),
                }],
            },
        ];

        store
            .append_messages(&metadata.id, &messages)
            .await
            .unwrap();

        let loaded = store.load_messages(&metadata.id).await.unwrap();
        assert_eq!(loaded.len(), 2);
    }

    #[tokio::test]
    #[ignore] // Requires running PostgreSQL
    async fn test_save_and_load_steps() {
        use serde_json::json;

        let pool = PgPool::connect("postgres://localhost/remix_test")
            .await
            .unwrap();
        let store = PostgresSessionStore::new(pool, 100);
        store.migrate().await.unwrap();

        let metadata = store.create("Steps test").await.unwrap();

        let steps = vec![
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
                duration_ms: 100,
                is_error: None,
            },
        ];

        store.save_steps(&metadata.id, &steps).await.unwrap();

        let snapshot = store.load(&metadata.id).await.unwrap();
        assert_eq!(snapshot.steps.len(), 2);
        assert_eq!(snapshot.steps[0].iteration, 1);
        assert_eq!(snapshot.steps[1].iteration, 2);
        assert_eq!(snapshot.iteration, 2);
    }

    #[tokio::test]
    #[ignore] // Requires running PostgreSQL
    async fn test_load_full_snapshot() {
        use crate::llm::types::{ContentBlock, Role};
        use serde_json::json;

        let pool = PgPool::connect("postgres://localhost/remix_test")
            .await
            .unwrap();
        let store = PostgresSessionStore::new(pool, 100);
        store.migrate().await.unwrap();

        let metadata = store.create("Full snapshot test").await.unwrap();

        store
            .append_messages(
                &metadata.id,
                &[Message {
                    role: Role::User,
                    content: vec![ContentBlock::Text {
                        text: "Go to example.com".to_string(),
                    }],
                }],
            )
            .await
            .unwrap();

        store
            .save_steps(
                &metadata.id,
                &[StepRecord {
                    iteration: 1,
                    tool: "navigate".to_string(),
                    input: json!({"url": "https://example.com"}),
                    output: json!({"success": true}),
                    duration_ms: 500,
                    is_error: None,
                }],
            )
            .await
            .unwrap();

        let snapshot = store.load(&metadata.id).await.unwrap();
        assert_eq!(snapshot.metadata.task, "Full snapshot test");
        assert_eq!(snapshot.messages.len(), 1);
        assert_eq!(snapshot.steps.len(), 1);
        assert_eq!(snapshot.iteration, 1);
        assert!(snapshot.system_prompt.is_none());
    }

    #[tokio::test]
    #[ignore] // Requires running PostgreSQL
    async fn test_list_sessions() {
        let pool = PgPool::connect("postgres://localhost/remix_test")
            .await
            .unwrap();
        let store = PostgresSessionStore::new(pool, 100);
        store.migrate().await.unwrap();

        store.create("List task 1").await.unwrap();
        store.create("List task 2").await.unwrap();

        let sessions = store.list().await.unwrap();
        assert!(sessions.len() >= 2);
    }

    #[tokio::test]
    #[ignore] // Requires running PostgreSQL
    async fn test_fork_session() {
        use crate::llm::types::{ContentBlock, Role};
        use serde_json::json;

        let pool = PgPool::connect("postgres://localhost/remix_test")
            .await
            .unwrap();
        let store = PostgresSessionStore::new(pool, 100);
        store.migrate().await.unwrap();

        let original = store.create("Fork source").await.unwrap();

        store
            .append_messages(
                &original.id,
                &[Message {
                    role: Role::User,
                    content: vec![ContentBlock::Text {
                        text: "Hello".to_string(),
                    }],
                }],
            )
            .await
            .unwrap();

        store
            .save_steps(
                &original.id,
                &[StepRecord {
                    iteration: 1,
                    tool: "navigate".to_string(),
                    input: json!({"url": "https://example.com"}),
                    output: json!({"success": true}),
                    duration_ms: 500,
                    is_error: None,
                }],
            )
            .await
            .unwrap();

        let forked = store.fork(&original.id).await.unwrap();
        assert_ne!(forked.id, original.id);
        assert_eq!(forked.task, "Fork source");
        assert_eq!(forked.parent_session_id, Some(original.id.clone()));
        assert_eq!(forked.status, SessionStatus::InProgress);

        // Verify forked session has copied data
        let forked_snapshot = store.load(&forked.id).await.unwrap();
        assert_eq!(forked_snapshot.messages.len(), 1);
        assert_eq!(forked_snapshot.steps.len(), 1);
    }

    #[tokio::test]
    #[ignore] // Requires running PostgreSQL
    async fn test_load_nonexistent_session() {
        let pool = PgPool::connect("postgres://localhost/remix_test")
            .await
            .unwrap();
        let store = PostgresSessionStore::new(pool, 100);
        store.migrate().await.unwrap();

        let fake_id = SessionId("nonexistent-id".to_string());
        let result = store.load_metadata(&fake_id).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    #[ignore] // Requires running PostgreSQL
    async fn test_save_metadata_nonexistent_session() {
        let pool = PgPool::connect("postgres://localhost/remix_test")
            .await
            .unwrap();
        let store = PostgresSessionStore::new(pool, 100);
        store.migrate().await.unwrap();

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
    #[ignore] // Requires running PostgreSQL
    async fn test_session_limit_enforced() {
        let pool = PgPool::connect("postgres://localhost/remix_test")
            .await
            .unwrap();
        let store = PostgresSessionStore::new(pool, 2);
        store.migrate().await.unwrap();

        store.create("Task 1").await.unwrap();
        store.create("Task 2").await.unwrap();
        let result = store.create("Task 3").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Session limit"));
    }

    #[test]
    fn test_status_to_str() {
        assert_eq!(
            PostgresSessionStore::status_to_str(&SessionStatus::InProgress),
            "in_progress"
        );
        assert_eq!(
            PostgresSessionStore::status_to_str(&SessionStatus::Completed),
            "completed"
        );
        assert_eq!(
            PostgresSessionStore::status_to_str(&SessionStatus::Error),
            "error"
        );
        assert_eq!(
            PostgresSessionStore::status_to_str(&SessionStatus::Paused),
            "paused"
        );
    }

    #[test]
    fn test_str_to_status() {
        assert_eq!(
            PostgresSessionStore::str_to_status("in_progress").unwrap(),
            SessionStatus::InProgress
        );
        assert_eq!(
            PostgresSessionStore::str_to_status("completed").unwrap(),
            SessionStatus::Completed
        );
        assert_eq!(
            PostgresSessionStore::str_to_status("error").unwrap(),
            SessionStatus::Error
        );
        assert_eq!(
            PostgresSessionStore::str_to_status("paused").unwrap(),
            SessionStatus::Paused
        );
        assert!(PostgresSessionStore::str_to_status("unknown").is_err());
    }
}
