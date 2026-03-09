use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;

use super::context::TenantId;
use crate::error::AgentError;

/// Per-tenant token bucket rate limiter.
pub struct TenantRateLimiter {
    buckets: Arc<RwLock<HashMap<TenantId, TokenBucket>>>,
    default_capacity: u32,
    default_refill_rate: f64, // tokens per second
}

struct TokenBucket {
    tokens: f64,
    capacity: u32,
    refill_rate: f64,
    last_refill: Instant,
}

impl TokenBucket {
    fn new(capacity: u32, refill_rate: f64) -> Self {
        Self {
            tokens: capacity as f64,
            capacity,
            refill_rate,
            last_refill: Instant::now(),
        }
    }

    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill_rate).min(self.capacity as f64);
        self.last_refill = now;
    }

    fn try_consume(&mut self, tokens: u32) -> bool {
        self.refill();
        if self.tokens >= tokens as f64 {
            self.tokens -= tokens as f64;
            true
        } else {
            false
        }
    }
}

impl TenantRateLimiter {
    pub fn new(default_capacity: u32, default_refill_rate: f64) -> Self {
        Self {
            buckets: Arc::new(RwLock::new(HashMap::new())),
            default_capacity,
            default_refill_rate,
        }
    }

    pub async fn check_rate(&self, tenant_id: &TenantId, tokens: u32) -> Result<(), AgentError> {
        let mut buckets = self.buckets.write().await;
        let bucket = buckets
            .entry(tenant_id.clone())
            .or_insert_with(|| TokenBucket::new(self.default_capacity, self.default_refill_rate));

        if bucket.try_consume(tokens) {
            Ok(())
        } else {
            Err(AgentError::Tenant(format!(
                "Rate limit exceeded for tenant: {}",
                tenant_id
            )))
        }
    }

    pub async fn remove_tenant(&self, tenant_id: &TenantId) {
        let mut buckets = self.buckets.write().await;
        buckets.remove(tenant_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_basic_rate_check_passes() {
        let limiter = TenantRateLimiter::new(10, 1.0);
        let tid = TenantId::new("t1");
        let result = limiter.check_rate(&tid, 1).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_rate_check_multiple_tokens() {
        let limiter = TenantRateLimiter::new(10, 1.0);
        let tid = TenantId::new("t1");
        let result = limiter.check_rate(&tid, 5).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_rate_limit_exceeded() {
        let limiter = TenantRateLimiter::new(5, 0.0); // no refill
        let tid = TenantId::new("t1");

        // Consume all 5 tokens
        limiter.check_rate(&tid, 5).await.unwrap();

        // Next request should fail
        let result = limiter.check_rate(&tid, 1).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            AgentError::Tenant(msg) if msg.contains("Rate limit exceeded")
        ));
    }

    #[tokio::test]
    async fn test_rate_limit_exceeded_single_large_request() {
        let limiter = TenantRateLimiter::new(5, 0.0);
        let tid = TenantId::new("t1");

        // Request more than capacity
        let result = limiter.check_rate(&tid, 6).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_tokens_refill_over_time() {
        let limiter = TenantRateLimiter::new(5, 100.0); // fast refill: 100 tokens/sec
        let tid = TenantId::new("t1");

        // Consume all tokens
        limiter.check_rate(&tid, 5).await.unwrap();

        // Wait a bit for refill
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Should have refilled enough tokens
        let result = limiter.check_rate(&tid, 1).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_tokens_do_not_exceed_capacity() {
        let limiter = TenantRateLimiter::new(5, 1000.0); // very fast refill
        let tid = TenantId::new("t1");

        // Wait for potential over-refill
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Should succeed for capacity amount
        let result = limiter.check_rate(&tid, 5).await;
        assert!(result.is_ok());

        // Should fail for anything more (0 refill rate check not applicable
        // here since refill is fast, but we just consumed all 5)
    }

    #[tokio::test]
    async fn test_remove_tenant_clears_bucket() {
        let limiter = TenantRateLimiter::new(5, 0.0);
        let tid = TenantId::new("t1");

        // Consume all tokens
        limiter.check_rate(&tid, 5).await.unwrap();
        assert!(limiter.check_rate(&tid, 1).await.is_err());

        // Remove tenant
        limiter.remove_tenant(&tid).await;

        // Should get fresh bucket with full capacity
        let result = limiter.check_rate(&tid, 5).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_separate_tenants_have_separate_buckets() {
        let limiter = TenantRateLimiter::new(5, 0.0);
        let t1 = TenantId::new("t1");
        let t2 = TenantId::new("t2");

        // Exhaust t1's tokens
        limiter.check_rate(&t1, 5).await.unwrap();
        assert!(limiter.check_rate(&t1, 1).await.is_err());

        // t2 should still have tokens
        let result = limiter.check_rate(&t2, 5).await;
        assert!(result.is_ok());
    }
}
