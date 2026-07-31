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

/// Create a dedicated AnthropicClient for compaction, using a cheaper model.
/// Returns None if no compaction_model is configured (falls back to primary LLM).
pub fn create_compaction_llm(context: &TenantContext) -> Option<AnthropicClient> {
    context.compaction_model.as_ref().map(|model| {
        AnthropicClient::new(
            crate::config::schema::default_base_url(),
            context.api_key.clone(),
            model.clone(),
            4096, // compaction responses are short summaries
            context.custom_headers.clone(),
            None,  // no thinking for compaction
            false, // no prompt caching for compaction
        )
    })
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
        lazy_tool_discovery: false,
        plan_mode: false,
        reminders: Vec::new(),
        self_critique: None,
        nudge_on_text_only: false,
        nudge_max_count: crate::config::schema::default_nudge_max_count(),
        goal_check_on_complete: false,
        action_reminder_interval: None,
        loop_detection: None,
        reasoning_stages: None,
        iteration_budget_warning_threshold: None,
    }
}

/// Create permissions config from tenant context.
///
/// Uses `Default` mode, not `BypassPermissions`. A tenant profile exists to *constrain*
/// a run — it carries `allowed_tools` and `denied_tools` — so granting bypass would
/// make those two fields inert and hand every tenant unrestricted access.
pub fn create_tenant_permissions(context: &TenantContext) -> PermissionsConfig {
    PermissionsConfig {
        mode: PermissionModeConfig::Default,
        allowed_tools: context.allowed_tools.clone(),
        denied_tools: context.denied_tools.clone(),
    }
}

/// Apply a tenant profile on top of a loaded config.
///
/// The tenant's limits win, because the point of the profile is to bound the run.
/// Fields the profile does not speak to are left as configured.
pub fn apply_tenant_profile(config: &mut crate::config::schema::AppConfig) {
    let Some(context) = config.tenant.clone() else {
        return;
    };

    tracing::info!(tenant = %context.id, "Applying tenant profile");

    config.llm.model = context.model.clone();
    config.llm.max_tokens = context.max_tokens;
    if !context.api_key.is_empty() {
        config.llm.api_key = context.api_key.clone();
    }
    for (k, v) in &context.custom_headers {
        config.llm.custom_headers.insert(k.clone(), v.clone());
    }

    // Take the tighter of the two bounds so a profile can only ever narrow a run.
    config.agent.max_iterations = config.agent.max_iterations.min(context.max_iterations);
    config.agent.timeout_secs = config.agent.timeout_secs.min(context.timeout_secs);
    config.agent.max_budget_usd = match (config.agent.max_budget_usd, context.max_budget_usd) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, b) => b,
    };
    if context.system_prompt.is_some() {
        config.agent.system_prompt = context.system_prompt.clone();
    }

    if context.compaction_model.is_some() {
        config.compaction.compaction_model = context.compaction_model.clone();
    }

    // Deny lists are additive: a profile may add restrictions but never remove them.
    let tenant_perms = create_tenant_permissions(&context);
    for denied in tenant_perms.denied_tools {
        if !config.permissions.denied_tools.contains(&denied) {
            config.permissions.denied_tools.push(denied);
        }
    }
    if !tenant_perms.allowed_tools.is_empty() {
        config.permissions.allowed_tools = tenant_perms.allowed_tools;
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
            compaction_model: None,
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

        // A tenant profile constrains a run; it must not grant bypass.
        assert_eq!(perms.mode, PermissionModeConfig::Default);
        assert_eq!(perms.allowed_tools, vec!["navigate".to_string()]);
        assert_eq!(perms.denied_tools, vec!["bash".to_string()]);
    }

    #[test]
    fn test_create_tenant_permissions_empty_tools() {
        let ctx = TenantContext::new("empty-tools", "sk-key");
        let perms = create_tenant_permissions(&ctx);

        // A tenant profile constrains a run; it must not grant bypass.
        assert_eq!(perms.mode, PermissionModeConfig::Default);
        assert!(perms.allowed_tools.is_empty());
        assert!(perms.denied_tools.is_empty());
    }

    // --- Tenant profile application ---
    //
    // These cover the wiring that makes this module reachable at all: before it, no
    // binary path constructed a TenantContext.

    fn config_with_tenant(context: TenantContext) -> crate::config::schema::AppConfig {
        crate::config::schema::AppConfig {
            tenant: Some(context),
            ..Default::default()
        }
    }

    #[test]
    fn test_apply_tenant_profile_sets_model_and_tokens() {
        let mut config = config_with_tenant(make_context());
        apply_tenant_profile(&mut config);
        assert_eq!(config.llm.model, "claude-sonnet-4-20250514");
        assert_eq!(config.llm.max_tokens, 4096);
        assert_eq!(config.llm.custom_headers.get("X-Custom").unwrap(), "val");
    }

    #[test]
    fn test_apply_tenant_profile_only_narrows_limits() {
        let mut config = config_with_tenant(make_context());
        // Start looser than the tenant allows.
        config.agent.max_iterations = 500;
        config.agent.timeout_secs = 9_999;
        config.agent.max_budget_usd = Some(100.0);

        apply_tenant_profile(&mut config);

        assert_eq!(config.agent.max_iterations, 30);
        assert_eq!(config.agent.timeout_secs, 180);
        assert_eq!(config.agent.max_budget_usd, Some(5.0));
    }

    #[test]
    fn test_apply_tenant_profile_does_not_loosen_limits() {
        let mut config = config_with_tenant(make_context());
        // Already tighter than the tenant profile; the profile must not raise them.
        config.agent.max_iterations = 5;
        config.agent.timeout_secs = 10;
        config.agent.max_budget_usd = Some(1.0);

        apply_tenant_profile(&mut config);

        assert_eq!(config.agent.max_iterations, 5);
        assert_eq!(config.agent.timeout_secs, 10);
        assert_eq!(config.agent.max_budget_usd, Some(1.0));
    }

    #[test]
    fn test_apply_tenant_profile_deny_list_is_additive() {
        let mut config = config_with_tenant(make_context());
        config.permissions.denied_tools = vec!["write_file".to_string()];

        apply_tenant_profile(&mut config);

        // The pre-existing denial survives and the tenant's is added.
        assert!(config
            .permissions
            .denied_tools
            .contains(&"write_file".to_string()));
        assert!(config
            .permissions
            .denied_tools
            .contains(&"bash".to_string()));
    }

    #[test]
    fn test_apply_tenant_profile_is_a_noop_without_a_tenant() {
        let mut config = crate::config::schema::AppConfig::default();
        let before = config.agent.max_iterations;
        apply_tenant_profile(&mut config);
        assert_eq!(config.agent.max_iterations, before);
    }
}
