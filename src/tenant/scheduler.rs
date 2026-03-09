use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, Semaphore};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use chrono::{DateTime, Utc};

use crate::error::AgentError;
use super::context::{TenantId, TenantContext};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RunId(pub String);

impl RunId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

impl Default for RunId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for RunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone)]
pub struct RunStatus {
    pub run_id: RunId,
    pub tenant_id: TenantId,
    pub task: String,
    pub started_at: DateTime<Utc>,
    pub is_cancelled: bool,
}

pub struct RunHandle {
    pub run_id: RunId,
    pub tenant_id: TenantId,
    pub task: String,
    pub started_at: DateTime<Utc>,
    pub cancel_token: CancellationToken,
}

struct ActiveRun {
    status: RunStatus,
    cancel_token: CancellationToken,
    #[allow(dead_code)]
    join_handle: JoinHandle<Result<(), AgentError>>,
}

pub struct TenantScheduler {
    tenants: Arc<RwLock<HashMap<TenantId, TenantContext>>>,
    semaphores: Arc<RwLock<HashMap<TenantId, Arc<Semaphore>>>>,
    global_semaphore: Arc<Semaphore>,
    active_runs: Arc<RwLock<HashMap<RunId, ActiveRun>>>,
}

impl TenantScheduler {
    pub fn new(global_max_concurrency: u32) -> Self {
        Self {
            tenants: Arc::new(RwLock::new(HashMap::new())),
            semaphores: Arc::new(RwLock::new(HashMap::new())),
            global_semaphore: Arc::new(Semaphore::new(global_max_concurrency as usize)),
            active_runs: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn register_tenant(&self, context: TenantContext) -> Result<(), AgentError> {
        let tenant_id = context.id.clone();
        let max_concurrent = context.max_concurrent_agents as usize;

        let mut tenants = self.tenants.write().await;
        if tenants.contains_key(&tenant_id) {
            return Err(AgentError::Tenant(format!(
                "Tenant already registered: {}",
                tenant_id
            )));
        }
        tenants.insert(tenant_id.clone(), context);

        let mut semaphores = self.semaphores.write().await;
        semaphores.insert(tenant_id, Arc::new(Semaphore::new(max_concurrent)));

        Ok(())
    }

    pub async fn remove_tenant(&self, tenant_id: &TenantId) -> Result<(), AgentError> {
        let mut tenants = self.tenants.write().await;
        if tenants.remove(tenant_id).is_none() {
            return Err(AgentError::Tenant(format!(
                "Tenant not found: {}",
                tenant_id
            )));
        }

        let mut semaphores = self.semaphores.write().await;
        semaphores.remove(tenant_id);

        Ok(())
    }

    pub async fn get_tenant(&self, tenant_id: &TenantId) -> Result<TenantContext, AgentError> {
        let tenants = self.tenants.read().await;
        tenants.get(tenant_id).cloned().ok_or_else(|| {
            AgentError::Tenant(format!("Tenant not found: {}", tenant_id))
        })
    }

    /// Schedule a run for a tenant. The provided closure receives the TenantContext and CancellationToken.
    /// Returns a RunHandle with the run_id and cancel_token.
    pub async fn schedule_run<F, Fut>(
        &self,
        tenant_id: &TenantId,
        task: String,
        run_fn: F,
    ) -> Result<RunHandle, AgentError>
    where
        F: FnOnce(TenantContext, CancellationToken) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<(), AgentError>> + Send + 'static,
    {
        let context = self.get_tenant(tenant_id).await?;

        let tenant_semaphore = {
            let semaphores = self.semaphores.read().await;
            semaphores.get(tenant_id).cloned().ok_or_else(|| {
                AgentError::Tenant(format!("Tenant semaphore not found: {}", tenant_id))
            })?
        };

        let run_id = RunId::new();
        let cancel_token = CancellationToken::new();
        let started_at = Utc::now();

        let status = RunStatus {
            run_id: run_id.clone(),
            tenant_id: tenant_id.clone(),
            task: task.clone(),
            started_at,
            is_cancelled: false,
        };

        let handle = RunHandle {
            run_id: run_id.clone(),
            tenant_id: tenant_id.clone(),
            task: task.clone(),
            started_at,
            cancel_token: cancel_token.clone(),
        };

        let global_sem = self.global_semaphore.clone();
        let cancel = cancel_token.clone();
        let active_runs = self.active_runs.clone();
        let rid = run_id.clone();

        let join_handle = tokio::spawn(async move {
            // Acquire both semaphore permits
            let _global_permit = global_sem
                .acquire()
                .await
                .map_err(|_| AgentError::Tenant("Global semaphore closed".to_string()))?;
            let _tenant_permit = tenant_semaphore
                .acquire()
                .await
                .map_err(|_| AgentError::Tenant("Tenant semaphore closed".to_string()))?;

            let result = tokio::select! {
                result = run_fn(context, cancel.clone()) => result,
                _ = cancel.cancelled() => {
                    Err(AgentError::Tenant("Run cancelled".to_string()))
                }
            };

            // Clean up active run
            let mut runs = active_runs.write().await;
            runs.remove(&rid);

            result
        });

        let active_run = ActiveRun {
            status,
            cancel_token: cancel_token.clone(),
            join_handle,
        };

        let mut runs = self.active_runs.write().await;
        runs.insert(run_id, active_run);

        Ok(handle)
    }

    pub async fn list_active_runs(&self, tenant_id: &TenantId) -> Vec<RunStatus> {
        let runs = self.active_runs.read().await;
        runs.values()
            .filter(|r| &r.status.tenant_id == tenant_id)
            .map(|r| RunStatus {
                is_cancelled: r.cancel_token.is_cancelled(),
                ..r.status.clone()
            })
            .collect()
    }

    pub async fn cancel_run(&self, run_id: &RunId) -> Result<(), AgentError> {
        let runs = self.active_runs.read().await;
        match runs.get(run_id) {
            Some(run) => {
                run.cancel_token.cancel();
                Ok(())
            }
            None => Err(AgentError::Tenant(format!("Run not found: {}", run_id))),
        }
    }

    pub async fn active_run_count(&self) -> usize {
        let runs = self.active_runs.read().await;
        runs.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn make_tenant_context(id: &str, max_concurrent: u32) -> TenantContext {
        let mut ctx = TenantContext::new(id, "sk-test-key");
        ctx.max_concurrent_agents = max_concurrent;
        ctx
    }

    #[tokio::test]
    async fn test_register_tenant() {
        let scheduler = TenantScheduler::new(10);
        let ctx = make_tenant_context("t1", 5);
        let result = scheduler.register_tenant(ctx).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_double_register_fails() {
        let scheduler = TenantScheduler::new(10);
        let ctx1 = make_tenant_context("t1", 5);
        let ctx2 = make_tenant_context("t1", 3);
        scheduler.register_tenant(ctx1).await.unwrap();
        let result = scheduler.register_tenant(ctx2).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), AgentError::Tenant(msg) if msg.contains("already registered")));
    }

    #[tokio::test]
    async fn test_remove_tenant() {
        let scheduler = TenantScheduler::new(10);
        let ctx = make_tenant_context("t1", 5);
        scheduler.register_tenant(ctx).await.unwrap();
        let result = scheduler.remove_tenant(&TenantId::new("t1")).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_remove_nonexistent_fails() {
        let scheduler = TenantScheduler::new(10);
        let result = scheduler.remove_tenant(&TenantId::new("nonexistent")).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), AgentError::Tenant(msg) if msg.contains("not found")));
    }

    #[tokio::test]
    async fn test_get_tenant() {
        let scheduler = TenantScheduler::new(10);
        let ctx = make_tenant_context("t1", 5);
        scheduler.register_tenant(ctx).await.unwrap();
        let retrieved = scheduler.get_tenant(&TenantId::new("t1")).await.unwrap();
        assert_eq!(retrieved.id, TenantId::new("t1"));
    }

    #[tokio::test]
    async fn test_get_nonexistent_tenant_fails() {
        let scheduler = TenantScheduler::new(10);
        let result = scheduler.get_tenant(&TenantId::new("missing")).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_schedule_run() {
        let scheduler = TenantScheduler::new(10);
        let ctx = make_tenant_context("t1", 5);
        scheduler.register_tenant(ctx).await.unwrap();

        let tid = TenantId::new("t1");
        let handle = scheduler
            .schedule_run(&tid, "test task".to_string(), |_ctx, _cancel| async {
                Ok(())
            })
            .await
            .unwrap();

        assert_eq!(handle.tenant_id, tid);
        assert_eq!(handle.task, "test task");

        // Give the spawned task time to complete
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    #[tokio::test]
    async fn test_schedule_run_for_unregistered_tenant_fails() {
        let scheduler = TenantScheduler::new(10);
        let tid = TenantId::new("unknown");
        let result = scheduler
            .schedule_run(&tid, "task".to_string(), |_ctx, _cancel| async { Ok(()) })
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_cancel_run() {
        let scheduler = TenantScheduler::new(10);
        let ctx = make_tenant_context("t1", 5);
        scheduler.register_tenant(ctx).await.unwrap();

        let tid = TenantId::new("t1");
        let handle = scheduler
            .schedule_run(&tid, "long task".to_string(), |_ctx, cancel| async move {
                // Wait until cancelled
                cancel.cancelled().await;
                Ok(())
            })
            .await
            .unwrap();

        // Cancel the run
        let result = scheduler.cancel_run(&handle.run_id).await;
        assert!(result.is_ok());

        // Give it time to process
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    #[tokio::test]
    async fn test_cancel_nonexistent_run_fails() {
        let scheduler = TenantScheduler::new(10);
        let result = scheduler.cancel_run(&RunId::new()).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), AgentError::Tenant(msg) if msg.contains("Run not found")));
    }

    #[tokio::test]
    async fn test_list_active_runs() {
        let scheduler = TenantScheduler::new(10);
        let ctx = make_tenant_context("t1", 5);
        scheduler.register_tenant(ctx).await.unwrap();

        let tid = TenantId::new("t1");
        let notify = Arc::new(tokio::sync::Notify::new());
        let notify_clone = notify.clone();

        let _handle = scheduler
            .schedule_run(&tid, "active task".to_string(), move |_ctx, _cancel| {
                let n = notify_clone;
                async move {
                    n.notified().await;
                    Ok(())
                }
            })
            .await
            .unwrap();

        // Give spawned task time to start
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let runs = scheduler.list_active_runs(&tid).await;
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].task, "active task");
        assert!(!runs[0].is_cancelled);

        // Release the task
        notify.notify_one();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    #[tokio::test]
    async fn test_active_run_count() {
        let scheduler = TenantScheduler::new(10);
        assert_eq!(scheduler.active_run_count().await, 0);
    }

    #[tokio::test]
    async fn test_concurrency_limiting() {
        let scheduler = TenantScheduler::new(10);
        // Register tenant with max_concurrent_agents = 1
        let ctx = make_tenant_context("t1", 1);
        scheduler.register_tenant(ctx).await.unwrap();

        let tid = TenantId::new("t1");
        let concurrent_count = Arc::new(AtomicU32::new(0));
        let max_concurrent = Arc::new(AtomicU32::new(0));
        let barrier = Arc::new(tokio::sync::Notify::new());

        // Schedule 2 runs, only 1 should execute at a time
        for _ in 0..2 {
            let cc = concurrent_count.clone();
            let mc = max_concurrent.clone();
            let b = barrier.clone();

            scheduler
                .schedule_run(&tid, "concurrent task".to_string(), move |_ctx, _cancel| async move {
                    let current = cc.fetch_add(1, Ordering::SeqCst) + 1;
                    // Update max if current is higher
                    mc.fetch_max(current, Ordering::SeqCst);

                    // Hold the slot for a bit
                    b.notified().await;

                    cc.fetch_sub(1, Ordering::SeqCst);
                    Ok(())
                })
                .await
                .unwrap();
        }

        // Give time for first task to start but not second (since max_concurrent=1)
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Release first task
        barrier.notify_one();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Release second task
        barrier.notify_one();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Max concurrent should be 1 since tenant allows only 1 concurrent agent
        assert_eq!(max_concurrent.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_run_id_display() {
        let id = RunId(String::from("test-run-id"));
        assert_eq!(format!("{}", id), "test-run-id");
    }

    #[test]
    fn test_run_id_default() {
        let id = RunId::default();
        assert!(!id.0.is_empty());
    }

    #[test]
    fn test_run_id_uniqueness() {
        let id1 = RunId::new();
        let id2 = RunId::new();
        assert_ne!(id1, id2);
    }
}
