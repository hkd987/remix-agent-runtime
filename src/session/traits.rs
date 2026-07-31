use async_trait::async_trait;

use crate::error::AgentError;
use crate::llm::types::Message;
use crate::output::result::StepRecord;

use super::types::{SessionId, SessionMetadata, SessionSnapshot};

/// Async trait for session storage backends.
///
/// Implementations must be `Send + Sync` so they can be shared across async tasks
/// and used as trait objects (`dyn SessionStorage`).
#[async_trait]
pub trait SessionStorage: Send + Sync {
    /// Create a new session with the given task description.
    async fn create(&self, task: &str) -> Result<SessionMetadata, AgentError>;

    /// Save/update session metadata.
    async fn save_metadata(&self, metadata: &SessionMetadata) -> Result<(), AgentError>;

    /// Replace the session's message log with `messages`.
    ///
    /// This is a full replace, not an append. The caller passes the entire in-memory
    /// conversation each iteration, and compaction rewrites that history in place —
    /// summarizing, merging, and pruning earlier turns — so an appending backend would
    /// both duplicate every message and keep messages the agent has already discarded.
    /// The persisted log must mirror the current conversation exactly.
    async fn save_messages(
        &self,
        session_id: &SessionId,
        messages: &[Message],
    ) -> Result<(), AgentError>;

    /// Save steps to the session, replacing previous steps.
    async fn save_steps(
        &self,
        session_id: &SessionId,
        steps: &[StepRecord],
    ) -> Result<(), AgentError>;

    /// Load a complete session snapshot.
    async fn load(&self, session_id: &SessionId) -> Result<SessionSnapshot, AgentError>;

    /// Load just session metadata.
    async fn load_metadata(&self, session_id: &SessionId) -> Result<SessionMetadata, AgentError>;

    /// Load just session messages.
    async fn load_messages(&self, session_id: &SessionId) -> Result<Vec<Message>, AgentError>;

    /// List all sessions (metadata only), sorted by most recent first.
    async fn list(&self) -> Result<Vec<SessionMetadata>, AgentError>;

    /// Fork a session - creates a new session with copied state.
    async fn fork(&self, source_id: &SessionId) -> Result<SessionMetadata, AgentError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that `SessionStorage` is object-safe by accepting a trait object reference.
    fn _assert_object_safe(_storage: &dyn SessionStorage) {}

    #[test]
    fn test_session_storage_is_object_safe() {
        // This test verifies at compile time that SessionStorage can be used as a trait object.
        // If SessionStorage were not object-safe, this file would fail to compile.
        fn _takes_dyn(_: &dyn SessionStorage) {}
    }
}
