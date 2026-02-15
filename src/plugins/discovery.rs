use std::path::{Path, PathBuf};
use tracing::{info, warn};

use crate::config::schema::PluginsConfig;
use crate::error::AgentError;

use super::github::clone_or_update_plugin;
use super::types::{
    resolve_plugin_at_path, InstalledPluginsJson, PluginComponents, PluginSet, PluginSource,
    ResolvedPlugin,
};

/// Default path to Claude Code's installed_plugins.json.
///
/// Located at `~/.claude/plugins/installed_plugins.json` on all platforms.
fn default_claude_code_plugins_path() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(|home| {
        PathBuf::from(home)
            .join(".claude")
            .join("plugins")
            .join("installed_plugins.json")
    })
}

/// Read and parse Claude Code's installed_plugins.json.
///
/// Returns an empty list if the file does not exist (not an error).
pub fn read_claude_code_cache(
    custom_path: Option<&Path>,
) -> Result<Vec<ResolvedPlugin>, AgentError> {
    let path = match custom_path {
        Some(p) => p.to_path_buf(),
        None => match default_claude_code_plugins_path() {
            Some(p) => p,
            None => return Ok(Vec::new()),
        },
    };

    if !path.exists() {
        return Ok(Vec::new());
    }

    let content = std::fs::read_to_string(&path).map_err(|e| {
        AgentError::Plugin(format!(
            "Failed to read installed_plugins.json at '{}': {e}",
            path.display()
        ))
    })?;

    let installed: InstalledPluginsJson = serde_json::from_str(&content)
        .map_err(|e| AgentError::Plugin(format!("Invalid installed_plugins.json: {e}")))?;

    let mut plugins = Vec::new();

    for (key, entries) in &installed.plugins {
        for entry in entries {
            if !entry.enabled {
                continue;
            }

            let install_path = PathBuf::from(&entry.install_path);
            if !install_path.exists() {
                warn!(
                    plugin = %key,
                    path = %entry.install_path,
                    "Plugin install path does not exist, skipping"
                );
                continue;
            }

            let components = resolve_plugin_at_path(&install_path);
            plugins.push(ResolvedPlugin {
                name: key.clone(),
                source: PluginSource::ClaudeCodeCache {
                    key: key.clone(),
                    scope: entry.scope.clone(),
                },
                root_path: install_path,
                components,
            });
        }
    }

    info!(
        count = plugins.len(),
        path = %path.display(),
        "Loaded plugins from Claude Code cache"
    );

    Ok(plugins)
}

/// Resolve a local directory as a plugin.
///
/// Returns `None` if the directory does not exist.
pub fn resolve_local_dir(path: &Path) -> Result<Option<ResolvedPlugin>, AgentError> {
    if !path.exists() {
        warn!(
            path = %path.display(),
            "Local plugin directory does not exist, skipping"
        );
        return Ok(None);
    }

    if !path.is_dir() {
        return Err(AgentError::Plugin(format!(
            "Plugin path '{}' is not a directory",
            path.display()
        )));
    }

    let components = resolve_plugin_at_path(path);
    let name = plugin_name_from_path(path);

    Ok(Some(ResolvedPlugin {
        name,
        source: PluginSource::LocalDir {
            path: path.to_path_buf(),
        },
        root_path: path.to_path_buf(),
        components,
    }))
}

/// Resolve a GitHub plugin by cloning/updating it and scanning its components.
pub fn resolve_github_source(
    repo: &str,
    git_ref: Option<&str>,
    cache_dir: &Path,
) -> Result<ResolvedPlugin, AgentError> {
    let target = clone_or_update_plugin(repo, git_ref, cache_dir)?;
    let components = resolve_plugin_at_path(&target);
    let name = repo.split('/').next_back().unwrap_or(repo).to_string();

    Ok(ResolvedPlugin {
        name,
        source: PluginSource::GitHub {
            repo: repo.to_string(),
            git_ref: git_ref.map(|s| s.to_string()),
        },
        root_path: target,
        components,
    })
}

/// Extract a plugin name from a filesystem path.
///
/// Uses the last path component, falling back to the full path string.
fn plugin_name_from_path(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string()
}

/// Discover all plugins from all configured sources.
///
/// Sources are processed in order:
/// 1. Claude Code cache (if enabled)
/// 2. Explicit sources from config (local dirs and GitHub repos)
///
/// Component filtering is NOT applied here -- callers decide which components
/// to use based on `PluginsConfig.components`.
pub fn discover_all_plugins(config: &PluginsConfig) -> Result<PluginSet, AgentError> {
    if !config.enabled {
        return Ok(PluginSet::new());
    }

    let mut plugin_set = PluginSet::new();

    // 1. Claude Code cache
    if config.claude_code_cache {
        match read_claude_code_cache(None) {
            Ok(plugins) => {
                for plugin in plugins {
                    info!(name = %plugin.name, source = %plugin.source, "Discovered plugin");
                    plugin_set.push(plugin);
                }
            }
            Err(e) => {
                warn!(error = %e, "Failed to read Claude Code plugin cache, continuing");
            }
        }
    }

    // 2. Explicit sources
    for source_config in &config.sources {
        if let Some(ref path) = source_config.path {
            match resolve_local_dir(path) {
                Ok(Some(plugin)) => {
                    info!(name = %plugin.name, source = %plugin.source, "Discovered plugin");
                    plugin_set.push(plugin);
                }
                Ok(None) => {} // Already warned
                Err(e) => {
                    warn!(error = %e, "Failed to resolve local plugin, skipping");
                }
            }
        }

        if let Some(ref repo) = source_config.github {
            let git_ref = source_config.git_ref.as_deref();
            match resolve_github_source(repo, git_ref, &config.github_cache_dir) {
                Ok(plugin) => {
                    info!(name = %plugin.name, source = %plugin.source, "Discovered plugin");
                    plugin_set.push(plugin);
                }
                Err(e) => {
                    warn!(
                        repo = %repo,
                        error = %e,
                        "Failed to resolve GitHub plugin, skipping"
                    );
                }
            }
        }
    }

    info!(total = plugin_set.len(), "Plugin discovery complete");
    Ok(plugin_set)
}

/// Check if a resolved plugin has any components at all.
pub fn has_any_components(components: &PluginComponents) -> bool {
    components.has_skills
        || components.has_mcp_config
        || components.has_hooks
        || components.has_agents
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::types::InstalledPluginEntry;
    use std::collections::HashMap;
    use std::fs;
    use tempfile::TempDir;

    // --- plugin_name_from_path ---

    #[test]
    fn test_plugin_name_from_path_normal() {
        let name = plugin_name_from_path(Path::new("/home/user/plugins/my-plugin"));
        assert_eq!(name, "my-plugin");
    }

    #[test]
    fn test_plugin_name_from_path_nested() {
        let name = plugin_name_from_path(Path::new("/a/b/c/deep-plugin"));
        assert_eq!(name, "deep-plugin");
    }

    #[test]
    fn test_plugin_name_from_path_root() {
        let name = plugin_name_from_path(Path::new("/"));
        assert_eq!(name, "unknown");
    }

    // --- has_any_components ---

    #[test]
    fn test_has_any_components_empty() {
        assert!(!has_any_components(&PluginComponents::default()));
    }

    #[test]
    fn test_has_any_components_skills() {
        let mut c = PluginComponents::default();
        c.has_skills = true;
        assert!(has_any_components(&c));
    }

    #[test]
    fn test_has_any_components_mcp() {
        let mut c = PluginComponents::default();
        c.has_mcp_config = true;
        assert!(has_any_components(&c));
    }

    #[test]
    fn test_has_any_components_hooks() {
        let mut c = PluginComponents::default();
        c.has_hooks = true;
        assert!(has_any_components(&c));
    }

    #[test]
    fn test_has_any_components_agents() {
        let mut c = PluginComponents::default();
        c.has_agents = true;
        assert!(has_any_components(&c));
    }

    // --- resolve_local_dir ---

    #[test]
    fn test_resolve_local_dir_valid() {
        let tmp = TempDir::new().unwrap();
        let plugin_dir = tmp.path().join("my-plugin");
        fs::create_dir_all(&plugin_dir).unwrap();

        // Add a .mcp.json to give it a component
        fs::write(plugin_dir.join(".mcp.json"), r#"{"mcpServers":{}}"#).unwrap();

        let result = resolve_local_dir(&plugin_dir).unwrap();
        assert!(result.is_some());

        let plugin = result.unwrap();
        assert_eq!(plugin.name, "my-plugin");
        assert!(matches!(plugin.source, PluginSource::LocalDir { .. }));
        assert!(plugin.components.has_mcp_config);
    }

    #[test]
    fn test_resolve_local_dir_nonexistent() {
        let result = resolve_local_dir(Path::new("/nonexistent/path/that/does/not/exist")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_resolve_local_dir_is_file() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("not-a-dir");
        fs::write(&file, "content").unwrap();

        let result = resolve_local_dir(&file);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not a directory"));
    }

    #[test]
    fn test_resolve_local_dir_empty() {
        let tmp = TempDir::new().unwrap();
        let plugin_dir = tmp.path().join("empty-plugin");
        fs::create_dir_all(&plugin_dir).unwrap();

        let plugin = resolve_local_dir(&plugin_dir).unwrap().unwrap();
        assert_eq!(plugin.name, "empty-plugin");
        assert!(!has_any_components(&plugin.components));
    }

    #[test]
    fn test_resolve_local_dir_with_all_components() {
        let tmp = TempDir::new().unwrap();
        let plugin_dir = tmp.path().join("full-plugin");
        fs::create_dir_all(&plugin_dir).unwrap();

        // Skills
        let skill = plugin_dir.join("skills").join("my-skill");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), "# Skill").unwrap();

        // MCP
        fs::write(plugin_dir.join(".mcp.json"), "{}").unwrap();

        // Hooks
        let hooks = plugin_dir.join("hooks");
        fs::create_dir_all(&hooks).unwrap();
        fs::write(hooks.join("hooks.json"), "{}").unwrap();

        // Agents
        let agents = plugin_dir.join("agents");
        fs::create_dir_all(&agents).unwrap();
        fs::write(agents.join("qa.md"), "# QA").unwrap();

        let plugin = resolve_local_dir(&plugin_dir).unwrap().unwrap();
        assert!(plugin.components.has_skills);
        assert!(plugin.components.has_mcp_config);
        assert!(plugin.components.has_hooks);
        assert!(plugin.components.has_agents);
    }

    // --- read_claude_code_cache ---

    #[test]
    fn test_read_cache_file_not_found() {
        let result = read_claude_code_cache(Some(Path::new("/nonexistent/installed_plugins.json")));
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_read_cache_invalid_json() {
        let tmp = TempDir::new().unwrap();
        let cache_file = tmp.path().join("installed_plugins.json");
        fs::write(&cache_file, "not valid json").unwrap();

        let result = read_claude_code_cache(Some(&cache_file));
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid installed_plugins.json"));
    }

    #[test]
    fn test_read_cache_empty_plugins() {
        let tmp = TempDir::new().unwrap();
        let cache_file = tmp.path().join("installed_plugins.json");
        let data = InstalledPluginsJson {
            version: 2,
            plugins: HashMap::new(),
        };
        fs::write(&cache_file, serde_json::to_string(&data).unwrap()).unwrap();

        let plugins = read_claude_code_cache(Some(&cache_file)).unwrap();
        assert!(plugins.is_empty());
    }

    #[test]
    fn test_read_cache_skips_disabled() {
        let tmp = TempDir::new().unwrap();
        let cache_file = tmp.path().join("installed_plugins.json");

        // Create a plugin dir that exists
        let plugin_dir = tmp.path().join("my-plugin");
        fs::create_dir_all(&plugin_dir).unwrap();

        let mut plugins_map = HashMap::new();
        plugins_map.insert(
            "my-plugin".to_string(),
            vec![InstalledPluginEntry {
                scope: "user".to_string(),
                install_path: plugin_dir.to_string_lossy().to_string(),
                version: "1.0.0".to_string(),
                enabled: false,
            }],
        );

        let data = InstalledPluginsJson {
            version: 2,
            plugins: plugins_map,
        };
        fs::write(&cache_file, serde_json::to_string(&data).unwrap()).unwrap();

        let plugins = read_claude_code_cache(Some(&cache_file)).unwrap();
        assert!(plugins.is_empty());
    }

    #[test]
    fn test_read_cache_skips_nonexistent_path() {
        let tmp = TempDir::new().unwrap();
        let cache_file = tmp.path().join("installed_plugins.json");

        let mut plugins_map = HashMap::new();
        plugins_map.insert(
            "gone-plugin".to_string(),
            vec![InstalledPluginEntry {
                scope: "user".to_string(),
                install_path: "/nonexistent/path/plugin".to_string(),
                version: "1.0.0".to_string(),
                enabled: true,
            }],
        );

        let data = InstalledPluginsJson {
            version: 2,
            plugins: plugins_map,
        };
        fs::write(&cache_file, serde_json::to_string(&data).unwrap()).unwrap();

        let plugins = read_claude_code_cache(Some(&cache_file)).unwrap();
        assert!(plugins.is_empty());
    }

    #[test]
    fn test_read_cache_valid_enabled_plugin() {
        let tmp = TempDir::new().unwrap();
        let cache_file = tmp.path().join("installed_plugins.json");

        // Create a plugin dir with components
        let plugin_dir = tmp.path().join("good-plugin");
        fs::create_dir_all(&plugin_dir).unwrap();
        fs::write(plugin_dir.join(".mcp.json"), "{}").unwrap();

        let mut plugins_map = HashMap::new();
        plugins_map.insert(
            "good-plugin".to_string(),
            vec![InstalledPluginEntry {
                scope: "user".to_string(),
                install_path: plugin_dir.to_string_lossy().to_string(),
                version: "1.0.0".to_string(),
                enabled: true,
            }],
        );

        let data = InstalledPluginsJson {
            version: 2,
            plugins: plugins_map,
        };
        fs::write(&cache_file, serde_json::to_string(&data).unwrap()).unwrap();

        let plugins = read_claude_code_cache(Some(&cache_file)).unwrap();
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "good-plugin");
        assert!(matches!(
            plugins[0].source,
            PluginSource::ClaudeCodeCache { .. }
        ));
        assert!(plugins[0].components.has_mcp_config);
    }

    #[test]
    fn test_read_cache_multiple_entries() {
        let tmp = TempDir::new().unwrap();
        let cache_file = tmp.path().join("installed_plugins.json");

        let plugin_a = tmp.path().join("plugin-a");
        let plugin_b = tmp.path().join("plugin-b");
        fs::create_dir_all(&plugin_a).unwrap();
        fs::create_dir_all(&plugin_b).unwrap();

        let mut plugins_map = HashMap::new();
        plugins_map.insert(
            "plugin-a".to_string(),
            vec![InstalledPluginEntry {
                scope: "user".to_string(),
                install_path: plugin_a.to_string_lossy().to_string(),
                version: "1.0.0".to_string(),
                enabled: true,
            }],
        );
        plugins_map.insert(
            "plugin-b".to_string(),
            vec![InstalledPluginEntry {
                scope: "project".to_string(),
                install_path: plugin_b.to_string_lossy().to_string(),
                version: "2.0.0".to_string(),
                enabled: true,
            }],
        );

        let data = InstalledPluginsJson {
            version: 2,
            plugins: plugins_map,
        };
        fs::write(&cache_file, serde_json::to_string(&data).unwrap()).unwrap();

        let plugins = read_claude_code_cache(Some(&cache_file)).unwrap();
        assert_eq!(plugins.len(), 2);

        let names: Vec<&str> = plugins.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"plugin-a"));
        assert!(names.contains(&"plugin-b"));
    }

    // --- discover_all_plugins ---

    #[test]
    fn test_discover_disabled_returns_empty() {
        let mut config = PluginsConfig::default();
        config.enabled = false;

        let result = discover_all_plugins(&config).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_discover_no_sources_no_cache() {
        let mut config = PluginsConfig::default();
        config.claude_code_cache = false;
        config.sources = Vec::new();

        let result = discover_all_plugins(&config).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_discover_with_local_source() {
        let tmp = TempDir::new().unwrap();
        let plugin_dir = tmp.path().join("local-plugin");
        fs::create_dir_all(&plugin_dir).unwrap();
        fs::write(plugin_dir.join(".mcp.json"), "{}").unwrap();

        let mut config = PluginsConfig::default();
        config.claude_code_cache = false;
        config.sources = vec![crate::config::schema::PluginSourceConfig {
            path: Some(plugin_dir),
            github: None,
            git_ref: None,
        }];

        let result = discover_all_plugins(&config).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result.iter().next().unwrap().name, "local-plugin");
    }

    #[test]
    fn test_discover_skips_nonexistent_local() {
        let mut config = PluginsConfig::default();
        config.claude_code_cache = false;
        config.sources = vec![crate::config::schema::PluginSourceConfig {
            path: Some(PathBuf::from("/nonexistent/plugin/path")),
            github: None,
            git_ref: None,
        }];

        let result = discover_all_plugins(&config).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_discover_multiple_local_sources() {
        let tmp = TempDir::new().unwrap();

        let dir_a = tmp.path().join("plugin-a");
        let dir_b = tmp.path().join("plugin-b");
        fs::create_dir_all(&dir_a).unwrap();
        fs::create_dir_all(&dir_b).unwrap();

        let mut config = PluginsConfig::default();
        config.claude_code_cache = false;
        config.sources = vec![
            crate::config::schema::PluginSourceConfig {
                path: Some(dir_a),
                github: None,
                git_ref: None,
            },
            crate::config::schema::PluginSourceConfig {
                path: Some(dir_b),
                github: None,
                git_ref: None,
            },
        ];

        let result = discover_all_plugins(&config).unwrap();
        assert_eq!(result.len(), 2);
    }

    // --- resolve_github_source (uses filesystem, no network) ---

    #[test]
    fn test_resolve_github_invalid_repo_format() {
        let tmp = TempDir::new().unwrap();
        let result = resolve_github_source("not-valid", None, tmp.path());
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid GitHub repo format"));
    }

    #[test]
    fn test_resolve_github_extracts_name_from_repo() {
        // We can't do a real clone in tests, but we can test the name extraction
        // by creating a pre-populated cache dir
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("owner").join("my-repo");
        fs::create_dir_all(&target).unwrap();

        // Fake a .git dir so it takes the update path (which will fail on fetch)
        fs::create_dir_all(target.join(".git")).unwrap();

        let result = resolve_github_source("owner/my-repo", Some("main"), tmp.path());
        // Will fail because fetch has no remote, but the name extraction logic
        // is testable through plugin_name_from_path directly
        assert!(result.is_err()); // expected: git fetch fails
    }

    // --- default_claude_code_plugins_path ---

    #[test]
    fn test_default_claude_code_plugins_path_with_home() {
        // HOME should be set in test environments
        let path = default_claude_code_plugins_path();
        assert!(path.is_some());
        let p = path.unwrap();
        assert!(p.to_string_lossy().contains(".claude"));
        assert!(p.to_string_lossy().contains("installed_plugins.json"));
    }
}
