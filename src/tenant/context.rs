use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TenantId(pub String);

impl TenantId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl std::fmt::Display for TenantId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantContext {
    pub id: TenantId,
    pub api_key: String,
    pub model: String,
    pub max_tokens: u32,
    pub system_prompt: Option<String>,
    pub allowed_tools: Vec<String>,
    pub denied_tools: Vec<String>,
    pub max_iterations: u32,
    pub timeout_secs: u64,
    pub max_budget_usd: Option<f64>,
    pub max_concurrent_agents: u32,
    pub custom_headers: HashMap<String, String>,
    /// Dedicated model for compaction summarization (e.g., "claude-haiku-4-5-20251001").
    /// When None, the primary model is used.
    #[serde(default)]
    pub compaction_model: Option<String>,
}

impl TenantContext {
    pub fn new(id: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            id: TenantId::new(id),
            api_key: api_key.into(),
            model: "claude-sonnet-4-20250514".to_string(),
            max_tokens: 8192,
            system_prompt: None,
            allowed_tools: Vec::new(),
            denied_tools: Vec::new(),
            max_iterations: 50,
            timeout_secs: 300,
            max_budget_usd: None,
            max_concurrent_agents: 5,
            custom_headers: HashMap::new(),
            compaction_model: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tenant_id_creation() {
        let id = TenantId::new("tenant-1");
        assert_eq!(id.0, "tenant-1");
    }

    #[test]
    fn test_tenant_id_from_string() {
        let id = TenantId::new(String::from("tenant-2"));
        assert_eq!(id.0, "tenant-2");
    }

    #[test]
    fn test_tenant_id_display() {
        let id = TenantId::new("my-tenant");
        assert_eq!(format!("{}", id), "my-tenant");
    }

    #[test]
    fn test_tenant_id_equality() {
        let id1 = TenantId::new("tenant-a");
        let id2 = TenantId::new("tenant-a");
        let id3 = TenantId::new("tenant-b");
        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_tenant_id_hash() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(TenantId::new("t1"));
        set.insert(TenantId::new("t1"));
        set.insert(TenantId::new("t2"));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn test_tenant_context_new_defaults() {
        let ctx = TenantContext::new("test-tenant", "sk-test-key");
        assert_eq!(ctx.id, TenantId::new("test-tenant"));
        assert_eq!(ctx.api_key, "sk-test-key");
        assert_eq!(ctx.model, "claude-sonnet-4-20250514");
        assert_eq!(ctx.max_tokens, 8192);
        assert!(ctx.system_prompt.is_none());
        assert!(ctx.allowed_tools.is_empty());
        assert!(ctx.denied_tools.is_empty());
        assert_eq!(ctx.max_iterations, 50);
        assert_eq!(ctx.timeout_secs, 300);
        assert!(ctx.max_budget_usd.is_none());
        assert_eq!(ctx.max_concurrent_agents, 5);
        assert!(ctx.custom_headers.is_empty());
    }

    #[test]
    fn test_tenant_context_serde_roundtrip() {
        let mut headers = HashMap::new();
        headers.insert("X-Custom".to_string(), "value".to_string());

        let ctx = TenantContext {
            id: TenantId::new("serde-test"),
            api_key: "sk-key-123".to_string(),
            model: "custom-model".to_string(),
            max_tokens: 4096,
            system_prompt: Some("You are helpful".to_string()),
            allowed_tools: vec!["navigate".to_string(), "click".to_string()],
            denied_tools: vec!["bash".to_string()],
            max_iterations: 25,
            timeout_secs: 120,
            max_budget_usd: Some(10.0),
            max_concurrent_agents: 3,
            custom_headers: headers,
            compaction_model: None,
        };

        let json = serde_json::to_string(&ctx).unwrap();
        let deserialized: TenantContext = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.id, ctx.id);
        assert_eq!(deserialized.api_key, ctx.api_key);
        assert_eq!(deserialized.model, ctx.model);
        assert_eq!(deserialized.max_tokens, ctx.max_tokens);
        assert_eq!(deserialized.system_prompt, ctx.system_prompt);
        assert_eq!(deserialized.allowed_tools, ctx.allowed_tools);
        assert_eq!(deserialized.denied_tools, ctx.denied_tools);
        assert_eq!(deserialized.max_iterations, ctx.max_iterations);
        assert_eq!(deserialized.timeout_secs, ctx.timeout_secs);
        assert_eq!(deserialized.max_budget_usd, ctx.max_budget_usd);
        assert_eq!(
            deserialized.max_concurrent_agents,
            ctx.max_concurrent_agents
        );
        assert_eq!(deserialized.custom_headers, ctx.custom_headers);
    }

    #[test]
    fn test_tenant_id_serde_roundtrip() {
        let id = TenantId::new("serde-id");
        let json = serde_json::to_string(&id).unwrap();
        let deserialized: TenantId = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, id);
    }
}
