use super::context::TenantContext;
use crate::config::schema::{AgentConfig, PermissionModeConfig, PermissionsConfig};
use crate::llm::client::AnthropicClient;

/// Create an AnthropicClient configured for a specific tenant.
pub fn create_tenant_llm(context: &TenantContext) -> AnthropicClient {
    AnthropicClient::new(
        crate::config::schema::default_base_url(),
        context.api_key.clone(),
        context.model.clone(),
        context.max_tokens,
        context.custom_headers.clone(),
        None, // thinking config - can be extended per tenant later
        true, // enable prompt caching
    )
}

/// Create an AgentConfig from tenant context.
pub fn create_tenant_config(context: &TenantContext) -> AgentConfig {
    AgentConfig {
        max_iterations: context.max_iterations,
        system_prompt: context.system_prompt.clone(),
        timeout_secs: context.timeout_secs,
        coordination_config: None,
        tool_result_max_bytes: 32768,
        max_budget_usd: context.max_budget_usd,
    }
}

/// Create permissions config from tenant context.
pub fn create_tenant_permissions(context: &TenantContext) -> PermissionsConfig {
    PermissionsConfig {
        mode: PermissionModeConfig::BypassPermissions,
        allowed_tools: context.allowed_tools.clone(),
        denied_tools: context.denied_tools.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_context() -> TenantContext {
        let mut headers = HashMap::new();
        headers.insert("X-Custom".to_string(), "val".to_string());

        TenantContext {
            id: super::super::context::TenantId::new("iso-test"),
            api_key: "sk-test-isolation".to_string(),
            model: "claude-sonnet-4-20250514".to_string(),
            max_tokens: 4096,
            system_prompt: Some("Be helpful".to_string()),
            allowed_tools: vec!["navigate".to_string()],
            denied_tools: vec!["bash".to_string()],
            max_iterations: 30,
            timeout_secs: 180,
            max_budget_usd: Some(5.0),
            max_concurrent_agents: 2,
            custom_headers: headers,
        }
    }

    #[test]
    fn test_create_tenant_llm() {
        let ctx = make_context();
        // Just verify it doesn't panic - AnthropicClient fields are private
        let _client = create_tenant_llm(&ctx);
    }

    #[test]
    fn test_create_tenant_config_preserves_fields() {
        let ctx = make_context();
        let config = create_tenant_config(&ctx);

        assert_eq!(config.max_iterations, 30);
        assert_eq!(config.system_prompt, Some("Be helpful".to_string()));
        assert_eq!(config.timeout_secs, 180);
        assert!(config.coordination_config.is_none());
        assert_eq!(config.tool_result_max_bytes, 32768);
        assert_eq!(config.max_budget_usd, Some(5.0));
    }

    #[test]
    fn test_create_tenant_config_default_context() {
        let ctx = TenantContext::new("default-test", "sk-key");
        let config = create_tenant_config(&ctx);

        assert_eq!(config.max_iterations, 50);
        assert!(config.system_prompt.is_none());
        assert_eq!(config.timeout_secs, 300);
        assert!(config.max_budget_usd.is_none());
    }

    #[test]
    fn test_create_tenant_permissions_maps_correctly() {
        let ctx = make_context();
        let perms = create_tenant_permissions(&ctx);

        assert_eq!(perms.mode, PermissionModeConfig::BypassPermissions);
        assert_eq!(perms.allowed_tools, vec!["navigate".to_string()]);
        assert_eq!(perms.denied_tools, vec!["bash".to_string()]);
    }

    #[test]
    fn test_create_tenant_permissions_empty_tools() {
        let ctx = TenantContext::new("empty-tools", "sk-key");
        let perms = create_tenant_permissions(&ctx);

        assert_eq!(perms.mode, PermissionModeConfig::BypassPermissions);
        assert!(perms.allowed_tools.is_empty());
        assert!(perms.denied_tools.is_empty());
    }
}
