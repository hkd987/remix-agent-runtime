use std::path::PathBuf;
use std::sync::Arc;

use crate::config::schema::CoordinationConfig;

use super::inbox_store::InboxStore;
use super::task_store::TaskStore;
use super::team_store::TeamStore;

/// Shared coordination state passed to parent and child agents via Arc.
#[derive(Debug)]
pub struct CoordinationContext {
    pub task_store: Arc<TaskStore>,
    pub team_store: Arc<TeamStore>,
    pub inbox_store: Arc<InboxStore>,
}

impl CoordinationContext {
    /// Create a new coordination context with stores backed by the given directory.
    pub fn new(storage_dir: PathBuf) -> Self {
        let tasks_dir = storage_dir.clone();
        let team_dir = storage_dir.clone();
        let inbox_dir = storage_dir;

        Self {
            task_store: Arc::new(TaskStore::new(tasks_dir)),
            team_store: Arc::new(TeamStore::new(team_dir)),
            inbox_store: Arc::new(InboxStore::new(inbox_dir)),
        }
    }

    /// Create from a CoordinationConfig.
    pub fn from_config(config: &CoordinationConfig) -> Self {
        Self::new(config.storage_dir.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_creation() {
        let dir = tempfile::TempDir::new().unwrap();
        let ctx = CoordinationContext::new(dir.path().to_path_buf());
        // Just verify we can access all stores
        let _ = &ctx.task_store;
        let _ = &ctx.team_store;
        let _ = &ctx.inbox_store;
    }

    #[test]
    fn test_context_from_config() {
        let dir = tempfile::TempDir::new().unwrap();
        let config = CoordinationConfig {
            enabled: true,
            max_worker_iterations: 10,
            worker_timeout_secs: 120,
            max_workers: 5,
            storage_dir: dir.path().to_path_buf(),
        };
        let ctx = CoordinationContext::from_config(&config);
        let _ = &ctx.task_store;
        let _ = &ctx.team_store;
        let _ = &ctx.inbox_store;
    }

    #[test]
    fn test_context_is_shareable() {
        let dir = tempfile::TempDir::new().unwrap();
        let ctx = Arc::new(CoordinationContext::new(dir.path().to_path_buf()));
        let ctx_clone = ctx.clone();
        // Both should reference same stores
        assert!(Arc::ptr_eq(&ctx.task_store, &ctx_clone.task_store));
        assert!(Arc::ptr_eq(&ctx.team_store, &ctx_clone.team_store));
        assert!(Arc::ptr_eq(&ctx.inbox_store, &ctx_clone.inbox_store));
    }
}
