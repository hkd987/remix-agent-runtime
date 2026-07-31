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

/// Pick the session `--continue` should resume: the most recently updated one.
///
/// Extracted from `main.rs` so it can be tested. `main.rs` is 886 lines with no tests
/// of its own, so logic that lives only there is logic nothing checks.
///
/// Ties are broken by session id, so the choice is deterministic rather than dependent
/// on whatever order the store happened to return.
pub fn most_recent_session(sessions: &[SessionMetadata]) -> Option<&SessionMetadata> {
    sessions.iter().max_by(|a, b| {
        a.updated_at
            .cmp(&b.updated_at)
            .then_with(|| a.id.0.cmp(&b.id.0))
    })
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

    use chrono::{Duration, Utc};

    fn meta(id: &str, age_secs: i64) -> SessionMetadata {
        let mut m = SessionMetadata::new(SessionId(id.to_string()), "task");
        m.updated_at = Utc::now() - Duration::seconds(age_secs);
        m
    }

    #[test]
    fn test_most_recent_session_picks_newest() {
        // `--continue` was dead code until recently; this is the logic it now depends on.
        let sessions = vec![meta("old", 300), meta("newest", 1), meta("middle", 60)];
        let picked = most_recent_session(&sessions).unwrap();
        assert_eq!(picked.id.0, "newest");
    }

    #[test]
    fn test_most_recent_session_ignores_input_order() {
        let a = vec![meta("old", 300), meta("newest", 1)];
        let b = vec![meta("newest", 1), meta("old", 300)];
        assert_eq!(
            most_recent_session(&a).unwrap().id.0,
            most_recent_session(&b).unwrap().id.0
        );
    }

    #[test]
    fn test_most_recent_session_breaks_ties_deterministically() {
        // Equal timestamps must not leave the choice up to store iteration order.
        let mut x = meta("aaa", 10);
        let mut y = meta("zzz", 10);
        let ts = Utc::now();
        x.updated_at = ts;
        y.updated_at = ts;
        assert_eq!(
            most_recent_session(&[x.clone(), y.clone()]).unwrap().id.0,
            "zzz"
        );
        assert_eq!(most_recent_session(&[y, x]).unwrap().id.0, "zzz");
    }

    #[test]
    fn test_most_recent_session_empty_is_none() {
        assert!(most_recent_session(&[]).is_none());
    }
}
