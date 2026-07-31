use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

fn default_enabled() -> bool {
    true
}

/// Represents the v2 format of Claude Code's installed_plugins.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledPluginsJson {
    pub version: u32,
    pub plugins: HashMap<String, Vec<InstalledPluginEntry>>,
}

/// A single installed plugin entry from the Claude Code cache.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledPluginEntry {
    pub scope: String,
    pub install_path: String,
    pub version: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

/// Source where a plugin was discovered from.
#[derive(Debug, Clone, PartialEq)]
pub enum PluginSource {
    ClaudeCodeCache {
        key: String,
        scope: String,
    },
    LocalDir {
        path: PathBuf,
    },
    GitHub {
        repo: String,
        git_ref: Option<String>,
    },
}

impl fmt::Display for PluginSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PluginSource::ClaudeCodeCache { key, scope } => {
                write!(f, "claude-code-cache({}, scope={})", key, scope)
            }
            PluginSource::LocalDir { path } => {
                write!(f, "local-dir({})", path.display())
            }
            PluginSource::GitHub { repo, git_ref } => match git_ref {
                Some(r) => write!(f, "github({}@{})", repo, r),
                None => write!(f, "github({})", repo),
            },
        }
    }
}

/// Discovered components within a plugin directory.
#[derive(Debug, Clone, Default)]
pub struct PluginComponents {
    pub has_skills: bool,
    pub skill_dirs: Vec<PathBuf>,
    pub has_mcp_config: bool,
    pub mcp_config_path: Option<PathBuf>,
    pub has_hooks: bool,
    pub hooks_config_path: Option<PathBuf>,
    pub has_agents: bool,
    pub agent_files: Vec<PathBuf>,
}

/// A fully resolved plugin with its root path and discovered components.
#[derive(Debug, Clone)]
pub struct ResolvedPlugin {
    pub name: String,
    pub source: PluginSource,
    pub root_path: PathBuf,
    pub components: PluginComponents,
}

/// Collection of resolved plugins.
#[derive(Debug, Clone, Default)]
pub struct PluginSet {
    plugins: Vec<ResolvedPlugin>,
}

impl PluginSet {
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    pub fn push(&mut self, plugin: ResolvedPlugin) {
        self.plugins.push(plugin);
    }

    pub fn iter(&self) -> impl Iterator<Item = &ResolvedPlugin> {
        self.plugins.iter()
    }

    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// Collect all skill directories from all plugins.
    pub fn all_skill_dirs(&self) -> Vec<&PathBuf> {
        self.plugins
            .iter()
            .flat_map(|p| p.components.skill_dirs.iter())
            .collect()
    }

    /// Collect all plugins that have an `.mcp.json` config, returning the
    /// plugin and the config path.
    pub fn all_mcp_configs(&self) -> Vec<(&ResolvedPlugin, &PathBuf)> {
        self.plugins
            .iter()
            .filter_map(|p| p.components.mcp_config_path.as_ref().map(|path| (p, path)))
            .collect()
    }

    /// Collect all plugins that have a hooks config, returning the plugin and
    /// the config path.
    pub fn all_hook_configs(&self) -> Vec<(&ResolvedPlugin, &PathBuf)> {
        self.plugins
            .iter()
            .filter_map(|p| {
                p.components
                    .hooks_config_path
                    .as_ref()
                    .map(|path| (p, path))
            })
            .collect()
    }

    /// Collect all plugins that have agent definition files, returning the
    /// plugin and each agent file path.
    pub fn all_agent_files(&self) -> Vec<(&ResolvedPlugin, &PathBuf)> {
        self.plugins
            .iter()
            .flat_map(|p| p.components.agent_files.iter().map(move |f| (p, f)))
            .collect()
    }
}

/// Scan a directory and return the components it contains.
///
/// Looks for:
/// - `skills/*/SKILL.md` subdirectories
/// - `.mcp.json` file
/// - `hooks/hooks.json` file
/// - `agents/*.md` files
pub fn resolve_plugin_at_path(root: &Path) -> PluginComponents {
    let mut components = PluginComponents::default();

    // Check for skills/*/SKILL.md
    let skills_dir = root.join("skills");
    if skills_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&skills_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let skill_md = path.join("SKILL.md");
                    if skill_md.is_file() {
                        components.skill_dirs.push(path);
                    }
                }
            }
        }
        components.skill_dirs.sort();
        components.has_skills = !components.skill_dirs.is_empty();
    }

    // Check for .mcp.json
    let mcp_json = root.join(".mcp.json");
    if mcp_json.is_file() {
        components.has_mcp_config = true;
        components.mcp_config_path = Some(mcp_json);
    }

    // Check for hooks/hooks.json
    let hooks_json = root.join("hooks").join("hooks.json");
    if hooks_json.is_file() {
        components.has_hooks = true;
        components.hooks_config_path = Some(hooks_json);
    }

    // Check for agents/*.md
    let agents_dir = root.join("agents");
    if agents_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&agents_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Some(ext) = path.extension() {
                        if ext == "md" {
                            components.agent_files.push(path);
                        }
                    }
                }
            }
        }
        components.agent_files.sort();
        components.has_agents = !components.agent_files.is_empty();
    }

    components
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // --- InstalledPluginsJson serde ---

    #[test]
    fn test_installed_plugins_json_serde_roundtrip() {
        let mut plugins_map = HashMap::new();
        plugins_map.insert(
            "my-plugin".to_string(),
            vec![InstalledPluginEntry {
                scope: "user".to_string(),
                install_path: "/home/user/.cache/plugins/my-plugin".to_string(),
                version: "1.0.0".to_string(),
                enabled: true,
            }],
        );

        let original = InstalledPluginsJson {
            version: 2,
            plugins: plugins_map,
        };

        let json = serde_json::to_string(&original).unwrap();
        let deserialized: InstalledPluginsJson = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.version, 2);
        assert_eq!(deserialized.plugins.len(), 1);
        let entries = deserialized.plugins.get("my-plugin").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].scope, "user");
        assert_eq!(
            entries[0].install_path,
            "/home/user/.cache/plugins/my-plugin"
        );
        assert_eq!(entries[0].version, "1.0.0");
        assert!(entries[0].enabled);
    }

    #[test]
    fn test_installed_plugin_entry_enabled_defaults_to_true() {
        let json = r#"{"scope":"user","install_path":"/tmp/p","version":"1.0.0"}"#;
        let entry: InstalledPluginEntry = serde_json::from_str(json).unwrap();
        assert!(entry.enabled);
    }

    #[test]
    fn test_installed_plugins_json_multiple_entries_per_key() {
        let mut plugins_map = HashMap::new();
        plugins_map.insert(
            "multi".to_string(),
            vec![
                InstalledPluginEntry {
                    scope: "user".to_string(),
                    install_path: "/tmp/a".to_string(),
                    version: "1.0.0".to_string(),
                    enabled: true,
                },
                InstalledPluginEntry {
                    scope: "project".to_string(),
                    install_path: "/tmp/b".to_string(),
                    version: "2.0.0".to_string(),
                    enabled: false,
                },
            ],
        );

        let ipj = InstalledPluginsJson {
            version: 2,
            plugins: plugins_map,
        };

        let json = serde_json::to_string(&ipj).unwrap();
        let deserialized: InstalledPluginsJson = serde_json::from_str(&json).unwrap();

        let entries = deserialized.plugins.get("multi").unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].scope, "user");
        assert_eq!(entries[1].scope, "project");
    }

    // --- PluginSource Display ---

    #[test]
    fn test_plugin_source_display_claude_code_cache() {
        let src = PluginSource::ClaudeCodeCache {
            key: "my-plugin".to_string(),
            scope: "user".to_string(),
        };
        assert_eq!(src.to_string(), "claude-code-cache(my-plugin, scope=user)");
    }

    #[test]
    fn test_plugin_source_display_local_dir() {
        let src = PluginSource::LocalDir {
            path: PathBuf::from("/home/user/plugins/local-one"),
        };
        assert_eq!(src.to_string(), "local-dir(/home/user/plugins/local-one)");
    }

    #[test]
    fn test_plugin_source_display_github_no_ref() {
        let src = PluginSource::GitHub {
            repo: "owner/repo".to_string(),
            git_ref: None,
        };
        assert_eq!(src.to_string(), "github(owner/repo)");
    }

    #[test]
    fn test_plugin_source_display_github_with_ref() {
        let src = PluginSource::GitHub {
            repo: "owner/repo".to_string(),
            git_ref: Some("v1.2.3".to_string()),
        };
        assert_eq!(src.to_string(), "github(owner/repo@v1.2.3)");
    }

    #[test]
    fn test_plugin_source_equality() {
        let a = PluginSource::LocalDir {
            path: PathBuf::from("/tmp/a"),
        };
        let b = PluginSource::LocalDir {
            path: PathBuf::from("/tmp/a"),
        };
        let c = PluginSource::LocalDir {
            path: PathBuf::from("/tmp/c"),
        };
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    // --- PluginComponents default ---

    #[test]
    fn test_plugin_components_default() {
        let c = PluginComponents::default();
        assert!(!c.has_skills);
        assert!(c.skill_dirs.is_empty());
        assert!(!c.has_mcp_config);
        assert!(c.mcp_config_path.is_none());
        assert!(!c.has_hooks);
        assert!(c.hooks_config_path.is_none());
        assert!(!c.has_agents);
        assert!(c.agent_files.is_empty());
    }

    // --- PluginSet ---

    fn make_resolved(name: &str, components: PluginComponents) -> ResolvedPlugin {
        ResolvedPlugin {
            name: name.to_string(),
            source: PluginSource::LocalDir {
                path: PathBuf::from(format!("/plugins/{}", name)),
            },
            root_path: PathBuf::from(format!("/plugins/{}", name)),
            components,
        }
    }

    #[test]
    fn test_plugin_set_new_is_empty() {
        let set = PluginSet::new();
        assert!(set.is_empty());
        assert_eq!(set.len(), 0);
    }

    #[test]
    fn test_plugin_set_push_and_len() {
        let mut set = PluginSet::new();
        set.push(make_resolved("p1", PluginComponents::default()));
        assert_eq!(set.len(), 1);
        assert!(!set.is_empty());

        set.push(make_resolved("p2", PluginComponents::default()));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn test_plugin_set_iter() {
        let mut set = PluginSet::new();
        set.push(make_resolved("alpha", PluginComponents::default()));
        set.push(make_resolved("beta", PluginComponents::default()));

        let names: Vec<&str> = set.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "beta"]);
    }

    #[test]
    fn test_plugin_set_all_skill_dirs() {
        let mut set = PluginSet::new();

        let c1 = PluginComponents {
            has_skills: true,
            skill_dirs: vec![PathBuf::from("/p1/skills/a"), PathBuf::from("/p1/skills/b")],
            ..Default::default()
        };
        set.push(make_resolved("p1", c1));

        let c2 = PluginComponents {
            has_skills: true,
            skill_dirs: vec![PathBuf::from("/p2/skills/c")],
            ..Default::default()
        };
        set.push(make_resolved("p2", c2));

        // Third plugin with no skills
        set.push(make_resolved("p3", PluginComponents::default()));

        let dirs = set.all_skill_dirs();
        assert_eq!(dirs.len(), 3);
        assert_eq!(dirs[0], &PathBuf::from("/p1/skills/a"));
        assert_eq!(dirs[1], &PathBuf::from("/p1/skills/b"));
        assert_eq!(dirs[2], &PathBuf::from("/p2/skills/c"));
    }

    #[test]
    fn test_plugin_set_all_mcp_configs() {
        let mut set = PluginSet::new();

        let c1 = PluginComponents {
            has_mcp_config: true,
            mcp_config_path: Some(PathBuf::from("/p1/.mcp.json")),
            ..Default::default()
        };
        set.push(make_resolved("p1", c1));

        // No mcp config
        set.push(make_resolved("p2", PluginComponents::default()));

        let configs = set.all_mcp_configs();
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].0.name, "p1");
        assert_eq!(configs[0].1, &PathBuf::from("/p1/.mcp.json"));
    }

    #[test]
    fn test_plugin_set_all_hook_configs() {
        let mut set = PluginSet::new();

        let c1 = PluginComponents {
            has_hooks: true,
            hooks_config_path: Some(PathBuf::from("/p1/hooks/hooks.json")),
            ..Default::default()
        };
        set.push(make_resolved("p1", c1));

        let c2 = PluginComponents {
            has_hooks: true,
            hooks_config_path: Some(PathBuf::from("/p2/hooks/hooks.json")),
            ..Default::default()
        };
        set.push(make_resolved("p2", c2));

        let configs = set.all_hook_configs();
        assert_eq!(configs.len(), 2);
        assert_eq!(configs[0].0.name, "p1");
        assert_eq!(configs[1].0.name, "p2");
    }

    #[test]
    fn test_plugin_set_all_agent_files() {
        let mut set = PluginSet::new();

        let c1 = PluginComponents {
            has_agents: true,
            agent_files: vec![
                PathBuf::from("/p1/agents/qa.md"),
                PathBuf::from("/p1/agents/dev.md"),
            ],
            ..Default::default()
        };
        set.push(make_resolved("p1", c1));

        // No agent files
        set.push(make_resolved("p2", PluginComponents::default()));

        let files = set.all_agent_files();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].0.name, "p1");
        assert_eq!(files[0].1, &PathBuf::from("/p1/agents/qa.md"));
        assert_eq!(files[1].1, &PathBuf::from("/p1/agents/dev.md"));
    }

    #[test]
    fn test_plugin_set_all_methods_empty() {
        let set = PluginSet::new();
        assert!(set.all_skill_dirs().is_empty());
        assert!(set.all_mcp_configs().is_empty());
        assert!(set.all_hook_configs().is_empty());
        assert!(set.all_agent_files().is_empty());
    }

    // --- resolve_plugin_at_path ---

    #[test]
    fn test_resolve_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let components = resolve_plugin_at_path(tmp.path());

        assert!(!components.has_skills);
        assert!(components.skill_dirs.is_empty());
        assert!(!components.has_mcp_config);
        assert!(components.mcp_config_path.is_none());
        assert!(!components.has_hooks);
        assert!(components.hooks_config_path.is_none());
        assert!(!components.has_agents);
        assert!(components.agent_files.is_empty());
    }

    #[test]
    fn test_resolve_with_skills() {
        let tmp = TempDir::new().unwrap();
        let skill_a = tmp.path().join("skills").join("login-flow");
        let skill_b = tmp.path().join("skills").join("checkout");
        std::fs::create_dir_all(&skill_a).unwrap();
        std::fs::create_dir_all(&skill_b).unwrap();
        std::fs::write(skill_a.join("SKILL.md"), "# Login Flow").unwrap();
        std::fs::write(skill_b.join("SKILL.md"), "# Checkout").unwrap();

        let components = resolve_plugin_at_path(tmp.path());

        assert!(components.has_skills);
        assert_eq!(components.skill_dirs.len(), 2);
        // Sorted, so checkout comes before login-flow
        assert!(components.skill_dirs[0].ends_with("checkout"));
        assert!(components.skill_dirs[1].ends_with("login-flow"));
    }

    #[test]
    fn test_resolve_skills_dir_without_skill_md_ignored() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("skills").join("no-skill-md");
        std::fs::create_dir_all(&skill_dir).unwrap();
        // No SKILL.md file created

        let components = resolve_plugin_at_path(tmp.path());

        assert!(!components.has_skills);
        assert!(components.skill_dirs.is_empty());
    }

    #[test]
    fn test_resolve_with_mcp_json() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join(".mcp.json"), r#"{"mcpServers":{}}"#).unwrap();

        let components = resolve_plugin_at_path(tmp.path());

        assert!(components.has_mcp_config);
        assert_eq!(
            components.mcp_config_path.unwrap(),
            tmp.path().join(".mcp.json")
        );
    }

    #[test]
    fn test_resolve_with_hooks_json() {
        let tmp = TempDir::new().unwrap();
        let hooks_dir = tmp.path().join("hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        std::fs::write(hooks_dir.join("hooks.json"), r#"{"hooks":[]}"#).unwrap();

        let components = resolve_plugin_at_path(tmp.path());

        assert!(components.has_hooks);
        assert_eq!(
            components.hooks_config_path.unwrap(),
            tmp.path().join("hooks").join("hooks.json")
        );
    }

    #[test]
    fn test_resolve_with_agent_files() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(agents_dir.join("qa-agent.md"), "# QA Agent").unwrap();
        std::fs::write(agents_dir.join("dev-agent.md"), "# Dev Agent").unwrap();
        // Non-md file should be ignored
        std::fs::write(agents_dir.join("readme.txt"), "ignore me").unwrap();

        let components = resolve_plugin_at_path(tmp.path());

        assert!(components.has_agents);
        assert_eq!(components.agent_files.len(), 2);
        // Sorted alphabetically
        assert!(components.agent_files[0].ends_with("dev-agent.md"));
        assert!(components.agent_files[1].ends_with("qa-agent.md"));
    }

    #[test]
    fn test_resolve_agents_dir_empty() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        // No .md files

        let components = resolve_plugin_at_path(tmp.path());

        assert!(!components.has_agents);
        assert!(components.agent_files.is_empty());
    }

    #[test]
    fn test_resolve_full_plugin() {
        let tmp = TempDir::new().unwrap();

        // Skills
        let skill_dir = tmp.path().join("skills").join("my-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "# Skill").unwrap();

        // MCP
        std::fs::write(tmp.path().join(".mcp.json"), "{}").unwrap();

        // Hooks
        let hooks_dir = tmp.path().join("hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        std::fs::write(hooks_dir.join("hooks.json"), "{}").unwrap();

        // Agents
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(agents_dir.join("main.md"), "# Agent").unwrap();

        let components = resolve_plugin_at_path(tmp.path());

        assert!(components.has_skills);
        assert_eq!(components.skill_dirs.len(), 1);
        assert!(components.has_mcp_config);
        assert!(components.mcp_config_path.is_some());
        assert!(components.has_hooks);
        assert!(components.hooks_config_path.is_some());
        assert!(components.has_agents);
        assert_eq!(components.agent_files.len(), 1);
    }

    #[test]
    fn test_resolve_nonexistent_dir() {
        let components = resolve_plugin_at_path(Path::new("/nonexistent/path/that/does/not/exist"));

        assert!(!components.has_skills);
        assert!(!components.has_mcp_config);
        assert!(!components.has_hooks);
        assert!(!components.has_agents);
    }

    // --- ResolvedPlugin construction ---

    #[test]
    fn test_resolved_plugin_clone() {
        let plugin = ResolvedPlugin {
            name: "test-plugin".to_string(),
            source: PluginSource::GitHub {
                repo: "owner/repo".to_string(),
                git_ref: Some("main".to_string()),
            },
            root_path: PathBuf::from("/tmp/test-plugin"),
            components: PluginComponents::default(),
        };

        let cloned = plugin.clone();
        assert_eq!(cloned.name, "test-plugin");
        assert_eq!(
            cloned.source,
            PluginSource::GitHub {
                repo: "owner/repo".to_string(),
                git_ref: Some("main".to_string()),
            }
        );
        assert_eq!(cloned.root_path, PathBuf::from("/tmp/test-plugin"));
    }
}
