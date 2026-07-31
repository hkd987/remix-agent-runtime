//! Plugin system integration tests.
//!
//! These tests exercise the real multi-layer decorator chain and the full
//! discovery-to-execution pipeline. No test requires an API key, browser, or
//! network access -- all use `tempfile::TempDir` fixtures and mock backends.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;
use tempfile::TempDir;

use remix_agent_runtime::browser::mcp::{ToolExecutionResult, ToolExecutor};
use remix_agent_runtime::config::schema::{LocalToolsConfig, PluginSourceConfig, PluginsConfig};
use remix_agent_runtime::error::AgentError;
use remix_agent_runtime::llm::types::ToolDefinition;
use remix_agent_runtime::local_tools::executor::LocalToolsExecutor;
use remix_agent_runtime::plugins::components::agents::{
    discover_agents_in_files, inject_agents_into_system_prompt,
};
use remix_agent_runtime::plugins::components::hooks::HookRegistry;
use remix_agent_runtime::plugins::components::skills::merge_plugin_skills;
use remix_agent_runtime::plugins::composite_executor::CompositeToolExecutor;
use remix_agent_runtime::plugins::discovery::{discover_all_plugins, resolve_local_dir};
use remix_agent_runtime::plugins::hook_executor::HookAwareExecutor;
use remix_agent_runtime::plugins::types::{PluginSet, PluginSource};
use remix_agent_runtime::skills::executor::SkillAwareExecutor;
use remix_agent_runtime::skills::types::{SkillEntry, SkillMetadata, SkillSet};

// ---------------------------------------------------------------------------
// Shared infrastructure
// ---------------------------------------------------------------------------

/// Mock backend that returns "{prefix}:{tool_name}" for routing verification.
struct MockBackend {
    tools: Vec<ToolDefinition>,
    prefix: String,
}

impl MockBackend {
    fn new(prefix: &str, tool_names: &[&str]) -> Self {
        let tools = tool_names
            .iter()
            .map(|name| ToolDefinition {
                name: name.to_string(),
                description: format!("{prefix} tool: {name}"),
                input_schema: json!({"type": "object"}),
                cache_control: None,
                read_only: false,
            })
            .collect();
        Self {
            tools,
            prefix: prefix.to_string(),
        }
    }
}

#[async_trait]
impl ToolExecutor for MockBackend {
    fn tool_definitions(&self) -> &[ToolDefinition] {
        &self.tools
    }

    async fn execute_tool(
        &self,
        name: &str,
        _arguments: serde_json::Value,
    ) -> Result<ToolExecutionResult, AgentError> {
        Ok(ToolExecutionResult {
            content: format!("{}:{name}", self.prefix),
            is_error: false,
        })
    }
}

/// Backend that sets an `Arc<AtomicBool>` on `shutdown()` for verification.
struct ShutdownTrackingBackend {
    tools: Vec<ToolDefinition>,
    shutdown_flag: Arc<AtomicBool>,
}

impl ShutdownTrackingBackend {
    fn new(name: &str, flag: Arc<AtomicBool>) -> Self {
        Self {
            tools: vec![ToolDefinition {
                name: name.to_string(),
                description: format!("tracked: {name}"),
                input_schema: json!({"type": "object"}),
                cache_control: None,
                read_only: false,
            }],
            shutdown_flag: flag,
        }
    }
}

#[async_trait]
impl ToolExecutor for ShutdownTrackingBackend {
    fn tool_definitions(&self) -> &[ToolDefinition] {
        &self.tools
    }

    async fn execute_tool(
        &self,
        name: &str,
        _arguments: serde_json::Value,
    ) -> Result<ToolExecutionResult, AgentError> {
        Ok(ToolExecutionResult {
            content: format!("tracked:{name}"),
            is_error: false,
        })
    }

    fn shutdown(self: Box<Self>) {
        self.shutdown_flag.store(true, Ordering::SeqCst);
    }
}

/// Build the full 4-layer chain:
/// HookAwareExecutor<LocalToolsExecutor<SkillAwareExecutor<CompositeToolExecutor>>>
fn build_full_chain(
    composite: CompositeToolExecutor,
    skill_set: SkillSet,
    local_tools_config: LocalToolsConfig,
    hook_registry: HookRegistry,
) -> HookAwareExecutor<LocalToolsExecutor<SkillAwareExecutor<CompositeToolExecutor>>> {
    let skill_layer = SkillAwareExecutor::new(composite, skill_set, 60);
    let local_layer = LocalToolsExecutor::new(skill_layer, local_tools_config, 32_768)
        .expect("LocalToolsExecutor::new should not fail in tests");
    HookAwareExecutor::new(local_layer, hook_registry, 30)
}

/// Create a SKILL.md file and return a SkillSet containing it.
fn make_skill(dir: &TempDir, name: &str, description: &str, body: &str) -> SkillSet {
    let skill_dir = dir.path().join("skills").join(name);
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n{body}\n"),
    )
    .unwrap();

    let canonical = std::fs::canonicalize(&skill_dir).unwrap();
    let mut set = SkillSet::new();
    set.insert(SkillEntry {
        metadata: SkillMetadata {
            name: name.to_string(),
            description: description.to_string(),
            license: None,
            compatibility: vec![],
            metadata: HashMap::new(),
            allowed_tools: vec![],
        },
        body: body.to_string(),
        dir_path: canonical.clone(),
        skill_md_path: canonical.join("SKILL.md"),
    })
    .unwrap();
    set
}

/// Create a hooks.json file and return a HookRegistry loaded from it.
fn make_hooks(
    dir: &std::path::Path,
    hooks: &[(&str, &str, &str)], // (matcher, timing "pre"/"post", command)
) -> HookRegistry {
    let hooks_dir = dir.join("hooks");
    std::fs::create_dir_all(&hooks_dir).unwrap();

    let pre: Vec<String> = hooks
        .iter()
        .filter(|(_, t, _)| *t == "pre")
        .map(|(m, _, c)| {
            format!(
                r#"{{ "matcher": "{m}", "hooks": [{{ "type": "command", "command": "{c}" }}] }}"#
            )
        })
        .collect();

    let post: Vec<String> = hooks
        .iter()
        .filter(|(_, t, _)| *t == "post")
        .map(|(m, _, c)| {
            format!(
                r#"{{ "matcher": "{m}", "hooks": [{{ "type": "command", "command": "{c}" }}] }}"#
            )
        })
        .collect();

    let json = format!(
        r#"{{ "hooks": {{ "PreToolUse": [{}], "PostToolUse": [{}] }} }}"#,
        pre.join(","),
        post.join(","),
    );

    let path = hooks_dir.join("hooks.json");
    std::fs::write(&path, json).unwrap();

    let mut registry = HookRegistry::new();
    registry.load_from_file(&path, dir).unwrap();
    registry
}

/// Create an agent .md file in `agents/` under `dir`.
fn make_agent_md(dir: &std::path::Path, name: &str, description: &str) {
    let agents_dir = dir.join("agents");
    std::fs::create_dir_all(&agents_dir).unwrap();
    let content = format!(
        "---\nname: {name}\ndescription: {description}\n---\n# {name}\n\nYou are {name}.\n"
    );
    std::fs::write(agents_dir.join(format!("{name}.md")), content).unwrap();
}

// ---------------------------------------------------------------------------
// Group 1: Full Decorator Chain Routing (Tests 1-4)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_full_chain_tool_definitions_merge() {
    let sandbox = TempDir::new().unwrap();
    let skill_dir = TempDir::new().unwrap();
    let skill_set = make_skill(&skill_dir, "my-skill", "A skill", "Instructions here");

    let mut composite = CompositeToolExecutor::new();
    composite.add_backend(Box::new(MockBackend::new(
        "browser",
        &["navigate", "click", "screenshot"],
    )));

    let config = LocalToolsConfig {
        enabled: true,
        sandbox_dir: Some(sandbox.path().to_path_buf()),
        ..Default::default()
    };

    let chain = build_full_chain(composite, skill_set, config, HookRegistry::new());

    let defs = chain.tool_definitions();
    let names: Vec<&str> = defs.iter().map(|t| t.name.as_str()).collect();

    // 3 backend + 3 skill virtual + 8 local = 14
    assert_eq!(
        defs.len(),
        14,
        "Expected 14 tools (3 backend + 3 skill + 8 local), got {}: {:?}",
        defs.len(),
        names
    );

    // No duplicates
    let mut unique = names.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        names.len(),
        "Tool names must be unique, found duplicates"
    );

    // Spot-check key tools
    assert!(names.contains(&"navigate"));
    assert!(names.contains(&"load_skill"));
    assert!(names.contains(&"read_file"));
    assert!(names.contains(&"bash"));
}

#[tokio::test]
async fn test_full_chain_routes_backend_tool() {
    let sandbox = TempDir::new().unwrap();

    let mut composite = CompositeToolExecutor::new();
    composite.add_backend(Box::new(MockBackend::new("browser", &["navigate"])));

    let config = LocalToolsConfig {
        enabled: true,
        sandbox_dir: Some(sandbox.path().to_path_buf()),
        ..Default::default()
    };

    let chain = build_full_chain(composite, SkillSet::new(), config, HookRegistry::new());

    let result = chain
        .execute_tool("navigate", json!({"url": "https://example.com"}))
        .await
        .unwrap();

    assert_eq!(result.content, "browser:navigate");
    assert!(!result.is_error);
}

#[tokio::test]
async fn test_full_chain_routes_local_tool() {
    let sandbox = TempDir::new().unwrap();
    std::fs::write(sandbox.path().join("test.txt"), "Hello from local tool").unwrap();

    let mut composite = CompositeToolExecutor::new();
    composite.add_backend(Box::new(MockBackend::new("browser", &["navigate"])));

    let config = LocalToolsConfig {
        enabled: true,
        sandbox_dir: Some(sandbox.path().to_path_buf()),
        ..Default::default()
    };

    let chain = build_full_chain(composite, SkillSet::new(), config, HookRegistry::new());

    let result = chain
        .execute_tool("read_file", json!({"path": "test.txt"}))
        .await
        .unwrap();

    // Should contain actual file content, NOT "browser:read_file"
    assert!(
        result.content.contains("Hello from local tool"),
        "Expected file content, got: {}",
        result.content
    );
    assert!(!result.is_error);
}

#[tokio::test]
async fn test_full_chain_routes_skill_tool() {
    let sandbox = TempDir::new().unwrap();
    let skill_dir = TempDir::new().unwrap();
    let skill_set = make_skill(
        &skill_dir,
        "my-skill",
        "A test skill",
        "Do the skill things.",
    );

    let mut composite = CompositeToolExecutor::new();
    composite.add_backend(Box::new(MockBackend::new("browser", &["navigate"])));

    let config = LocalToolsConfig {
        enabled: true,
        sandbox_dir: Some(sandbox.path().to_path_buf()),
        ..Default::default()
    };

    let chain = build_full_chain(composite, skill_set, config, HookRegistry::new());

    let result = chain
        .execute_tool("load_skill", json!({"name": "my-skill"}))
        .await
        .unwrap();

    assert!(
        result.content.contains("Do the skill things."),
        "Expected SKILL.md body, got: {}",
        result.content
    );
    assert!(!result.is_error);
}

// ---------------------------------------------------------------------------
// Group 2: Hooks in Real Chain (Tests 5-6)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_hooks_fire_around_backend_tool() {
    let sandbox = TempDir::new().unwrap();
    let hook_dir = TempDir::new().unwrap();

    let pre_marker = hook_dir.path().join("pre_ran");
    let post_marker = hook_dir.path().join("post_ran");

    let registry = make_hooks(
        hook_dir.path(),
        &[
            (
                "navigate",
                "pre",
                &format!("touch {}", pre_marker.display()),
            ),
            (
                "navigate",
                "post",
                &format!("touch {}", post_marker.display()),
            ),
        ],
    );

    let mut composite = CompositeToolExecutor::new();
    composite.add_backend(Box::new(MockBackend::new("browser", &["navigate"])));

    let config = LocalToolsConfig {
        enabled: true,
        sandbox_dir: Some(sandbox.path().to_path_buf()),
        ..Default::default()
    };

    let chain = build_full_chain(composite, SkillSet::new(), config, registry);

    let result = chain
        .execute_tool("navigate", json!({"url": "test"}))
        .await
        .unwrap();

    // Result should still come from mock backend
    assert_eq!(result.content, "browser:navigate");

    // Give hooks time to finish
    tokio::time::sleep(Duration::from_millis(200)).await;

    assert!(
        pre_marker.exists(),
        "Pre-hook should have created marker file"
    );
    assert!(
        post_marker.exists(),
        "Post-hook should have created marker file"
    );
}

#[tokio::test]
async fn test_hooks_fire_around_local_tool() {
    let sandbox = TempDir::new().unwrap();
    std::fs::write(sandbox.path().join("data.txt"), "local content").unwrap();

    let hook_dir = TempDir::new().unwrap();
    let marker = hook_dir.path().join("hook_ran");

    let registry = make_hooks(
        hook_dir.path(),
        &[("read_file", "pre", &format!("touch {}", marker.display()))],
    );

    let mut composite = CompositeToolExecutor::new();
    composite.add_backend(Box::new(MockBackend::new("browser", &["navigate"])));

    let config = LocalToolsConfig {
        enabled: true,
        sandbox_dir: Some(sandbox.path().to_path_buf()),
        ..Default::default()
    };

    let chain = build_full_chain(composite, SkillSet::new(), config, registry);

    let result = chain
        .execute_tool("read_file", json!({"path": "data.txt"}))
        .await
        .unwrap();

    // Result should be actual file content (intercepted by LocalToolsExecutor)
    assert!(
        result.content.contains("local content"),
        "Expected file content, got: {}",
        result.content
    );

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(marker.exists(), "Hook should have fired for local tool");
}

// ---------------------------------------------------------------------------
// Group 3: Discovery -> Execution Pipeline (Tests 7-9)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_discovery_to_skill_merge_to_load_skill() {
    let plugin_dir = TempDir::new().unwrap();
    let sandbox = TempDir::new().unwrap();

    // Create plugin with a skill
    let skill_dir = plugin_dir.path().join("skills").join("test-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: test-skill\ndescription: A discovered skill\n---\nDiscovered skill body.\n",
    )
    .unwrap();

    // Resolve plugin via discovery
    let plugin = resolve_local_dir(plugin_dir.path()).unwrap().unwrap();
    let mut plugin_set = PluginSet::new();
    plugin_set.push(plugin);

    // Merge skills
    let mut skill_set = SkillSet::new();
    let count = merge_plugin_skills(&plugin_set, &mut skill_set).unwrap();
    assert_eq!(count, 1);
    assert!(skill_set.get("test-skill").is_some());

    // Build executor chain with merged skills
    let composite = CompositeToolExecutor::new();
    let config = LocalToolsConfig {
        enabled: false,
        sandbox_dir: Some(sandbox.path().to_path_buf()),
        ..Default::default()
    };
    let chain = build_full_chain(composite, skill_set, config, HookRegistry::new());

    // Execute load_skill
    let result = chain
        .execute_tool("load_skill", json!({"name": "test-skill"}))
        .await
        .unwrap();

    assert!(
        result.content.contains("Discovered skill body."),
        "Expected skill body, got: {}",
        result.content
    );
}

#[test]
fn test_discovery_to_agent_injection() {
    let plugin_dir = TempDir::new().unwrap();

    // Create two agent files
    make_agent_md(plugin_dir.path(), "researcher", "Searches the web");
    make_agent_md(plugin_dir.path(), "coder", "Writes code");

    // Resolve plugin and discover agents
    let plugin = resolve_local_dir(plugin_dir.path()).unwrap().unwrap();
    assert!(plugin.components.has_agents);
    assert_eq!(plugin.components.agent_files.len(), 2);

    let file_refs: Vec<&PathBuf> = plugin.components.agent_files.iter().collect();
    let agents = discover_agents_in_files(&file_refs);
    assert_eq!(agents.len(), 2);

    // Inject into system prompt
    let prompt = inject_agents_into_system_prompt(&agents).unwrap();
    assert!(prompt.contains("<available_agents>"));
    assert!(prompt.contains("</available_agents>"));

    // Both agents should appear
    let agent_names: Vec<&str> = agents.iter().map(|a| a.definition.name.as_str()).collect();
    assert!(agent_names.contains(&"researcher"));
    assert!(agent_names.contains(&"coder"));
}

#[tokio::test]
async fn test_discovery_to_hook_registry_to_execution() {
    let plugin_dir = TempDir::new().unwrap();
    let sandbox = TempDir::new().unwrap();

    let marker = plugin_dir.path().join("hook_marker");
    let command = format!("touch {}", marker.display());

    // Create hooks/ directory manually (resolve_plugin_at_path detects hooks/hooks.json)
    let hooks_dir = plugin_dir.path().join("hooks");
    std::fs::create_dir_all(&hooks_dir).unwrap();
    let hooks_json = format!(
        r#"{{ "hooks": {{ "PreToolUse": [{{ "matcher": "navigate", "hooks": [{{ "type": "command", "command": "{command}" }}] }}], "PostToolUse": [] }} }}"#,
    );
    std::fs::write(hooks_dir.join("hooks.json"), hooks_json).unwrap();

    // Discover plugin
    let plugin = resolve_local_dir(plugin_dir.path()).unwrap().unwrap();
    assert!(plugin.components.has_hooks);

    // Load hooks into registry
    let mut registry = HookRegistry::new();
    registry
        .load_from_file(
            plugin.components.hooks_config_path.as_ref().unwrap(),
            &plugin.root_path,
        )
        .unwrap();
    assert_eq!(registry.len(), 1);

    // Build chain with hook registry
    let mut composite = CompositeToolExecutor::new();
    composite.add_backend(Box::new(MockBackend::new("browser", &["navigate"])));

    let config = LocalToolsConfig {
        enabled: false,
        sandbox_dir: Some(sandbox.path().to_path_buf()),
        ..Default::default()
    };

    let chain = build_full_chain(composite, SkillSet::new(), config, registry);

    chain
        .execute_tool("navigate", json!({"url": "test"}))
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        marker.exists(),
        "Hook from discovered plugin should have created marker file"
    );
}

// ---------------------------------------------------------------------------
// Group 4: Shutdown Contract (Tests 10-11)
// ---------------------------------------------------------------------------

#[test]
fn test_composite_shutdown_all_calls_backends() {
    let flag_a = Arc::new(AtomicBool::new(false));
    let flag_b = Arc::new(AtomicBool::new(false));
    let flag_c = Arc::new(AtomicBool::new(false));

    let mut composite = CompositeToolExecutor::new();
    composite.add_backend(Box::new(ShutdownTrackingBackend::new(
        "tool_a",
        flag_a.clone(),
    )));
    composite.add_backend(Box::new(ShutdownTrackingBackend::new(
        "tool_b",
        flag_b.clone(),
    )));
    composite.add_backend(Box::new(ShutdownTrackingBackend::new(
        "tool_c",
        flag_c.clone(),
    )));

    composite.shutdown_all();

    assert!(
        flag_a.load(Ordering::SeqCst),
        "Backend A should have been shut down"
    );
    assert!(
        flag_b.load(Ordering::SeqCst),
        "Backend B should have been shut down"
    );
    assert!(
        flag_c.load(Ordering::SeqCst),
        "Backend C should have been shut down"
    );
}

#[test]
fn test_full_chain_shutdown_unwrap() {
    let sandbox = TempDir::new().unwrap();
    let flag = Arc::new(AtomicBool::new(false));

    let mut composite = CompositeToolExecutor::new();
    composite.add_backend(Box::new(ShutdownTrackingBackend::new(
        "tool_x",
        flag.clone(),
    )));

    let config = LocalToolsConfig {
        enabled: true,
        sandbox_dir: Some(sandbox.path().to_path_buf()),
        ..Default::default()
    };

    let chain = build_full_chain(composite, SkillSet::new(), config, HookRegistry::new());

    // Unwrap through all layers: Hook -> Local -> Skill -> Composite
    let local_layer = chain.into_inner();
    let skill_layer = local_layer.into_inner();
    let composite = skill_layer.into_inner();
    composite.shutdown_all();

    assert!(
        flag.load(Ordering::SeqCst),
        "Backend should have been shut down through full chain unwrap"
    );
}

// ---------------------------------------------------------------------------
// Group 5: Edge Cases (Tests 12-14)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_disabled_local_tools_delegates_all() {
    let sandbox = TempDir::new().unwrap();
    std::fs::write(sandbox.path().join("test.txt"), "real content").unwrap();

    let mut composite = CompositeToolExecutor::new();
    composite.add_backend(Box::new(MockBackend::new("mock", &["read_file"])));

    let config = LocalToolsConfig {
        enabled: false,
        sandbox_dir: Some(sandbox.path().to_path_buf()),
        ..Default::default()
    };

    let chain = build_full_chain(composite, SkillSet::new(), config, HookRegistry::new());

    let result = chain
        .execute_tool("read_file", json!({"path": "test.txt"}))
        .await
        .unwrap();

    // With local tools disabled, should delegate to mock backend
    assert_eq!(
        result.content, "mock:read_file",
        "Disabled local tools should delegate to inner executor"
    );
}

#[test]
fn test_empty_plugin_set_no_overhead() {
    let plugin_set = PluginSet::new();

    // Skill merge: 0 skills
    let mut skill_set = SkillSet::new();
    let count = merge_plugin_skills(&plugin_set, &mut skill_set).unwrap();
    assert_eq!(count, 0);
    assert!(skill_set.is_empty());

    // Hook configs: empty
    let hook_configs = plugin_set.all_hook_configs();
    assert!(hook_configs.is_empty());

    // Agent files: empty
    let agent_files = plugin_set.all_agent_files();
    assert!(agent_files.is_empty());

    // Agent prompt: None
    let prompt = inject_agents_into_system_prompt(&[]);
    assert!(prompt.is_none());
}

#[tokio::test]
async fn test_multiple_plugins_merge_all_components() {
    let sandbox = TempDir::new().unwrap();

    // Plugin 1: has skill + hook + agent
    let plugin1_dir = TempDir::new().unwrap();
    let skill1 = plugin1_dir.path().join("skills").join("alpha-skill");
    std::fs::create_dir_all(&skill1).unwrap();
    std::fs::write(
        skill1.join("SKILL.md"),
        "---\nname: alpha-skill\ndescription: Alpha\n---\nAlpha instructions.\n",
    )
    .unwrap();
    make_agent_md(plugin1_dir.path(), "agent-a", "Agent A description");

    let hooks1_dir = plugin1_dir.path().join("hooks");
    std::fs::create_dir_all(&hooks1_dir).unwrap();
    std::fs::write(
        hooks1_dir.join("hooks.json"),
        r#"{ "hooks": { "PreToolUse": [{ "matcher": "navigate", "hooks": [{ "type": "command", "command": "echo hook1" }] }], "PostToolUse": [] } }"#,
    )
    .unwrap();

    // Plugin 2: has skill + hook + agent
    let plugin2_dir = TempDir::new().unwrap();
    let skill2 = plugin2_dir.path().join("skills").join("beta-skill");
    std::fs::create_dir_all(&skill2).unwrap();
    std::fs::write(
        skill2.join("SKILL.md"),
        "---\nname: beta-skill\ndescription: Beta\n---\nBeta instructions.\n",
    )
    .unwrap();
    make_agent_md(plugin2_dir.path(), "agent-b", "Agent B description");

    let hooks2_dir = plugin2_dir.path().join("hooks");
    std::fs::create_dir_all(&hooks2_dir).unwrap();
    std::fs::write(
        hooks2_dir.join("hooks.json"),
        r#"{ "hooks": { "PreToolUse": [], "PostToolUse": [{ "matcher": "click", "hooks": [{ "type": "command", "command": "echo hook2" }] }] } }"#,
    )
    .unwrap();

    // Resolve both plugins
    let p1 = resolve_local_dir(plugin1_dir.path()).unwrap().unwrap();
    let p2 = resolve_local_dir(plugin2_dir.path()).unwrap().unwrap();

    let mut plugin_set = PluginSet::new();
    plugin_set.push(p1);
    plugin_set.push(p2);

    // Merge skills: should get 2
    let mut skill_set = SkillSet::new();
    let count = merge_plugin_skills(&plugin_set, &mut skill_set).unwrap();
    assert_eq!(count, 2, "Should merge 2 unique skills");
    assert!(skill_set.get("alpha-skill").is_some());
    assert!(skill_set.get("beta-skill").is_some());

    // Merge hooks: should get 2
    let mut registry = HookRegistry::new();
    for (plugin, config_path) in plugin_set.all_hook_configs() {
        registry
            .load_from_file(config_path, &plugin.root_path)
            .unwrap();
    }
    assert_eq!(registry.len(), 2, "Should have 2 hooks total");

    // Discover agents: should get 2
    let agent_file_refs: Vec<&PathBuf> = plugin_set
        .all_agent_files()
        .into_iter()
        .map(|(_, f)| f)
        .collect();
    let agents = discover_agents_in_files(&agent_file_refs);
    assert_eq!(agents.len(), 2, "Should discover 2 agents");

    // Build chain and verify skill tools work
    let mut composite = CompositeToolExecutor::new();
    composite.add_backend(Box::new(MockBackend::new("browser", &["navigate"])));

    let config = LocalToolsConfig {
        enabled: true,
        sandbox_dir: Some(sandbox.path().to_path_buf()),
        ..Default::default()
    };

    let chain = build_full_chain(composite, skill_set, config, registry);

    // Verify both skills are loadable
    let result = chain
        .execute_tool("load_skill", json!({"name": "alpha-skill"}))
        .await
        .unwrap();
    assert!(result.content.contains("Alpha instructions."));

    let result = chain
        .execute_tool("load_skill", json!({"name": "beta-skill"}))
        .await
        .unwrap();
    assert!(result.content.contains("Beta instructions."));
}

// ---------------------------------------------------------------------------
// Group 6: Config/Discovery (Tests 15-16)
// ---------------------------------------------------------------------------

#[test]
fn test_disabled_plugins_returns_empty() {
    let config = PluginsConfig {
        enabled: false,
        ..Default::default()
    };

    let result = discover_all_plugins(&config).unwrap();
    assert!(result.is_empty());
}

#[test]
fn test_local_source_discovery() {
    let tmp = TempDir::new().unwrap();
    let plugin_dir = tmp.path().join("my-local-plugin");
    std::fs::create_dir_all(&plugin_dir).unwrap();

    // Add components so it looks like a real plugin
    let skill_dir = plugin_dir.join("skills").join("local-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "# Skill").unwrap();

    let config = PluginsConfig {
        enabled: true,
        claude_code_cache: false,
        sources: vec![PluginSourceConfig {
            path: Some(plugin_dir),
            github: None,
            git_ref: None,
        }],
        ..Default::default()
    };

    let result = discover_all_plugins(&config).unwrap();
    assert_eq!(result.len(), 1);

    let plugin = result.iter().next().unwrap();
    assert_eq!(plugin.name, "my-local-plugin");
    assert!(matches!(plugin.source, PluginSource::LocalDir { .. }));
    assert!(plugin.components.has_skills);
}
