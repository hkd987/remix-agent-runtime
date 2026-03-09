use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppConfig {
    pub task: Option<String>,
    #[serde(default)]
    pub llm: LlmConfig,
    #[serde(default)]
    pub browser: BrowserConfig,
    #[serde(default)]
    pub agent: AgentConfig,
    #[serde(default)]
    pub credentials: Vec<super::credentials::RawCredential>,
    pub on_complete: Option<WebhookConfig>,
    pub on_error: Option<WebhookConfig>,
    #[serde(default)]
    pub skills: SkillsConfig,
    #[serde(default)]
    pub agents_md: AgentsMdConfig,
    #[serde(default)]
    pub local_tools: LocalToolsConfig,
    #[serde(default)]
    pub plugins: PluginsConfig,
    #[serde(default)]
    pub session: SessionConfig,
    #[serde(default)]
    pub compaction: CompactionConfig,
    #[serde(default)]
    pub permissions: PermissionsConfig,
    #[serde(default)]
    pub subagent: SubagentConfig,
    #[serde(default)]
    pub coordination: CoordinationConfig,
}

fn default_base_url() -> String {
    "https://api.anthropic.com".to_string()
}

fn default_model() -> String {
    "claude-sonnet-4-20250514".to_string()
}

fn default_max_tokens() -> u32 {
    8192
}

fn default_true() -> bool {
    true
}

fn default_timeout() -> u64 {
    300
}

fn default_viewport_width() -> u32 {
    1280
}

fn default_viewport_height() -> u32 {
    720
}

fn default_max_iterations() -> u32 {
    50
}

fn default_script_timeout() -> u64 {
    60
}

fn default_max_size_bytes() -> usize {
    32768
}

fn default_bash_timeout() -> u64 {
    120
}

fn default_read_max_bytes() -> usize {
    1_048_576
}

fn default_write_max_bytes() -> usize {
    10_485_760
}

fn default_webhook_format() -> String {
    "json".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    #[serde(default = "default_base_url")]
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default)]
    pub custom_headers: HashMap<String, String>,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            base_url: default_base_url(),
            api_key: String::new(),
            model: default_model(),
            max_tokens: default_max_tokens(),
            custom_headers: HashMap::new(),
        }
    }
}

impl fmt::Display for LlmConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted_key = if self.api_key.is_empty() {
            "<not set>".to_string()
        } else if self.api_key.len() <= 8 {
            "***".to_string()
        } else {
            let visible = &self.api_key[..4];
            format!("{}...***", visible)
        };
        write!(
            f,
            "LlmConfig {{ base_url: {}, api_key: {}, model: {}, max_tokens: {} }}",
            self.base_url, redacted_key, self.model, self.max_tokens
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserConfig {
    #[serde(default = "default_true")]
    pub headless: bool,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default = "default_viewport_width")]
    pub viewport_width: u32,
    #[serde(default = "default_viewport_height")]
    pub viewport_height: u32,
    pub browser_path: Option<String>,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            headless: default_true(),
            timeout_secs: default_timeout(),
            viewport_width: default_viewport_width(),
            viewport_height: default_viewport_height(),
            browser_path: None,
        }
    }
}

fn default_tool_result_max_bytes() -> usize {
    32768
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    #[serde(default = "default_max_iterations")]
    pub max_iterations: u32,
    pub system_prompt: Option<String>,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default = "default_coordination_config")]
    pub coordination_config: Option<CoordinationConfig>,
    #[serde(default = "default_tool_result_max_bytes")]
    pub tool_result_max_bytes: usize,
    #[serde(default)]
    pub max_budget_usd: Option<f64>,
}

fn default_coordination_config() -> Option<CoordinationConfig> {
    Some(CoordinationConfig::default())
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_iterations: default_max_iterations(),
            system_prompt: None,
            timeout_secs: default_timeout(),
            coordination_config: Some(CoordinationConfig::default()),
            tool_result_max_bytes: default_tool_result_max_bytes(),
            max_budget_usd: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookConfig {
    pub url: String,
    #[serde(default = "default_webhook_format")]
    pub format: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillsConfig {
    #[serde(default)]
    pub dirs: Vec<PathBuf>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_script_timeout")]
    pub script_timeout_secs: u64,
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            dirs: Vec::new(),
            enabled: true,
            script_timeout_secs: 60,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentsMdConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub search_dir: Option<PathBuf>,
    #[serde(default = "default_max_size_bytes")]
    pub max_size_bytes: usize,
}

impl Default for AgentsMdConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            search_dir: None,
            max_size_bytes: 32768,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalToolsConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub sandbox_dir: Option<PathBuf>,
    #[serde(default = "default_bash_timeout")]
    pub bash_timeout_secs: u64,
    #[serde(default = "default_read_max_bytes")]
    pub read_max_bytes: usize,
    #[serde(default = "default_write_max_bytes")]
    pub write_max_bytes: usize,
    #[serde(default)]
    pub bash_allowlist: Vec<String>,
    #[serde(default)]
    pub bash_blocklist: Vec<String>,
}

impl Default for LocalToolsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            sandbox_dir: None,
            bash_timeout_secs: 120,
            read_max_bytes: 1_048_576,
            write_max_bytes: 10_485_760,
            bash_allowlist: Vec::new(),
            bash_blocklist: Vec::new(),
        }
    }
}

fn default_hook_timeout() -> u64 {
    30
}

fn default_github_cache_dir() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".remix").join("plugin-cache")
    } else {
        PathBuf::from(".remix").join("plugin-cache")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginsConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub claude_code_cache: bool,
    #[serde(default)]
    pub sources: Vec<PluginSourceConfig>,
    #[serde(default = "default_github_cache_dir")]
    pub github_cache_dir: PathBuf,
    #[serde(default)]
    pub components: PluginComponentsConfig,
    #[serde(default = "default_hook_timeout")]
    pub hook_timeout_secs: u64,
}

impl Default for PluginsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            claude_code_cache: true,
            sources: Vec::new(),
            github_cache_dir: default_github_cache_dir(),
            components: PluginComponentsConfig::default(),
            hook_timeout_secs: 30,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginSourceConfig {
    #[serde(default)]
    pub path: Option<PathBuf>,
    #[serde(default)]
    pub github: Option<String>,
    #[serde(default)]
    pub git_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginComponentsConfig {
    #[serde(default = "default_true")]
    pub skills: bool,
    #[serde(default = "default_true")]
    pub mcp_servers: bool,
    #[serde(default = "default_true")]
    pub hooks: bool,
    #[serde(default = "default_true")]
    pub agents: bool,
}

impl Default for PluginComponentsConfig {
    fn default() -> Self {
        Self {
            skills: true,
            mcp_servers: true,
            hooks: true,
            agents: true,
        }
    }
}

pub fn default_session_dir() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".remix").join("sessions")
    } else {
        PathBuf::from(".remix").join("sessions")
    }
}

fn default_max_sessions() -> usize {
    100
}

fn default_context_window_tokens() -> u32 {
    200_000
}

fn default_trigger_threshold() -> f32 {
    0.95
}

fn default_preserve_recent_n() -> usize {
    4
}

fn default_subagent_max_iterations() -> u32 {
    10
}

fn default_subagent_timeout() -> u64 {
    120
}

fn default_max_workers() -> u32 {
    5
}

fn default_coordination_dir() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".remix").join("coordination")
    } else {
        PathBuf::from(".remix").join("coordination")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_session_dir")]
    pub storage_dir: PathBuf,
    #[serde(default = "default_max_sessions")]
    pub max_sessions: usize,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            storage_dir: default_session_dir(),
            max_sessions: 100,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_trigger_threshold")]
    pub trigger_threshold: f32,
    #[serde(default = "default_context_window_tokens")]
    pub context_window_tokens: u32,
    #[serde(default = "default_preserve_recent_n")]
    pub preserve_recent_n: usize,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            trigger_threshold: 0.95,
            context_window_tokens: 200_000,
            preserve_recent_n: 4,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionsConfig {
    #[serde(default)]
    pub mode: PermissionModeConfig,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    #[serde(default)]
    pub denied_tools: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionModeConfig {
    #[default]
    Default,
    AcceptEdits,
    BypassPermissions,
    Plan,
    DontAsk,
}

impl Default for PermissionsConfig {
    fn default() -> Self {
        Self {
            mode: PermissionModeConfig::Default,
            allowed_tools: Vec::new(),
            denied_tools: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_subagent_max_iterations")]
    pub max_iterations: u32,
    #[serde(default = "default_subagent_timeout")]
    pub timeout_secs: u64,
}

impl Default for SubagentConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_iterations: 10,
            timeout_secs: 120,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoordinationConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_subagent_max_iterations")]
    pub max_worker_iterations: u32,
    #[serde(default = "default_subagent_timeout")]
    pub worker_timeout_secs: u64,
    #[serde(default = "default_max_workers")]
    pub max_workers: u32,
    #[serde(default = "default_coordination_dir")]
    pub storage_dir: PathBuf,
}

impl Default for CoordinationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_worker_iterations: 10,
            worker_timeout_secs: 120,
            max_workers: 5,
            storage_dir: default_coordination_dir(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_llm_config_defaults() {
        let config = LlmConfig::default();
        assert_eq!(config.base_url, "https://api.anthropic.com");
        assert_eq!(config.model, "claude-sonnet-4-20250514");
        assert_eq!(config.max_tokens, 8192);
        assert!(config.api_key.is_empty());
        assert!(config.custom_headers.is_empty());
    }

    #[test]
    fn test_browser_config_defaults() {
        let config = BrowserConfig::default();
        assert!(config.headless);
        assert_eq!(config.timeout_secs, 300);
        assert_eq!(config.viewport_width, 1280);
        assert_eq!(config.viewport_height, 720);
        assert!(config.browser_path.is_none());
    }

    #[test]
    fn test_agent_config_defaults() {
        let config = AgentConfig::default();
        assert_eq!(config.max_iterations, 50);
        assert_eq!(config.timeout_secs, 300);
        assert!(config.system_prompt.is_none());
        assert!(config.coordination_config.is_some());
    }

    #[test]
    fn test_app_config_defaults() {
        let config = AppConfig::default();
        assert!(config.task.is_none());
        assert_eq!(config.llm.base_url, "https://api.anthropic.com");
        assert!(config.browser.headless);
        assert_eq!(config.agent.max_iterations, 50);
        assert!(config.credentials.is_empty());
        assert!(config.on_complete.is_none());
        assert!(config.on_error.is_none());
    }

    #[test]
    fn test_llm_display_redacts_api_key() {
        let config = LlmConfig {
            api_key: "sk-ant-1234567890abcdef".to_string(),
            ..Default::default()
        };
        let display = format!("{}", config);
        assert!(!display.contains("sk-ant-1234567890abcdef"));
        assert!(display.contains("sk-a...***"));
    }

    #[test]
    fn test_llm_display_empty_key() {
        let config = LlmConfig::default();
        let display = format!("{}", config);
        assert!(display.contains("<not set>"));
    }

    #[test]
    fn test_llm_display_short_key() {
        let config = LlmConfig {
            api_key: "short".to_string(),
            ..Default::default()
        };
        let display = format!("{}", config);
        assert!(!display.contains("short"));
        assert!(display.contains("***"));
    }

    #[test]
    fn test_app_config_yaml_deserialization() {
        let yaml = r#"
task: "navigate to google.com"
llm:
  base_url: "https://custom.api.com"
  api_key: "test-key"
  model: "custom-model"
  max_tokens: 4096
browser:
  headless: false
  timeout_secs: 600
  viewport_width: 1920
  viewport_height: 1080
agent:
  max_iterations: 100
  timeout_secs: 600
  system_prompt: "You are a helpful assistant"
"#;
        let config: AppConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.task.as_deref(), Some("navigate to google.com"));
        assert_eq!(config.llm.base_url, "https://custom.api.com");
        assert_eq!(config.llm.api_key, "test-key");
        assert_eq!(config.llm.model, "custom-model");
        assert_eq!(config.llm.max_tokens, 4096);
        assert!(!config.browser.headless);
        assert_eq!(config.browser.timeout_secs, 600);
        assert_eq!(config.browser.viewport_width, 1920);
        assert_eq!(config.browser.viewport_height, 1080);
        assert_eq!(config.agent.max_iterations, 100);
        assert_eq!(config.agent.timeout_secs, 600);
        assert_eq!(
            config.agent.system_prompt.as_deref(),
            Some("You are a helpful assistant")
        );
    }

    #[test]
    fn test_app_config_yaml_with_defaults() {
        let yaml = r#"
task: "do something"
"#;
        let config: AppConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.task.as_deref(), Some("do something"));
        assert_eq!(config.llm.base_url, "https://api.anthropic.com");
        assert_eq!(config.llm.model, "claude-sonnet-4-20250514");
        assert_eq!(config.llm.max_tokens, 8192);
        assert!(config.browser.headless);
        assert_eq!(config.agent.max_iterations, 50);
    }

    #[test]
    fn test_app_config_yaml_with_webhooks() {
        let yaml = r#"
task: "test"
on_complete:
  url: "https://hooks.example.com/complete"
  format: "slack"
on_error:
  url: "https://hooks.example.com/error"
"#;
        let config: AppConfig = serde_yaml::from_str(yaml).unwrap();
        let on_complete = config.on_complete.unwrap();
        assert_eq!(on_complete.url, "https://hooks.example.com/complete");
        assert_eq!(on_complete.format, "slack");
        let on_error = config.on_error.unwrap();
        assert_eq!(on_error.url, "https://hooks.example.com/error");
        assert_eq!(on_error.format, "json");
    }

    #[test]
    fn test_app_config_yaml_with_credentials() {
        let yaml = r#"
task: "login test"
credentials:
  - name: "site_login"
    credential_type: username_password
    username: "admin"
    password: "secret"
    url_pattern: "*.example.com"
"#;
        let config: AppConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.credentials.len(), 1);
        assert_eq!(config.credentials[0].name, "site_login");
        assert_eq!(config.credentials[0].username.as_deref(), Some("admin"));
    }

    #[test]
    fn test_app_config_empty_yaml() {
        let yaml = "{}";
        let config: AppConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.task.is_none());
        assert_eq!(config.llm.base_url, "https://api.anthropic.com");
    }

    #[test]
    fn test_llm_config_with_custom_headers() {
        let yaml = r#"
base_url: "https://api.example.com"
api_key: "key"
model: "model"
custom_headers:
  X-Custom: "value"
  Authorization: "Bearer token"
"#;
        let config: LlmConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.custom_headers.get("X-Custom").unwrap(), "value");
        assert_eq!(
            config.custom_headers.get("Authorization").unwrap(),
            "Bearer token"
        );
    }

    #[test]
    fn test_browser_config_with_browser_path() {
        let yaml = r#"
headless: false
browser_path: "/usr/local/bin/remix-browser"
"#;
        let config: BrowserConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(!config.headless);
        assert_eq!(
            config.browser_path.as_deref(),
            Some("/usr/local/bin/remix-browser")
        );
    }

    #[test]
    fn test_webhook_config_default_format() {
        let yaml = r#"
url: "https://example.com/hook"
"#;
        let config: WebhookConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.url, "https://example.com/hook");
        assert_eq!(config.format, "json");
    }

    #[test]
    fn test_app_config_serialization_roundtrip() {
        let config = AppConfig {
            task: Some("test task".to_string()),
            llm: LlmConfig {
                base_url: "https://api.test.com".to_string(),
                api_key: "key123".to_string(),
                model: "test-model".to_string(),
                max_tokens: 2048,
                custom_headers: HashMap::new(),
            },
            browser: BrowserConfig::default(),
            agent: AgentConfig::default(),
            credentials: vec![],
            on_complete: None,
            on_error: None,
            skills: SkillsConfig::default(),
            agents_md: AgentsMdConfig::default(),
            local_tools: LocalToolsConfig::default(),
            plugins: PluginsConfig::default(),
            session: SessionConfig::default(),
            compaction: CompactionConfig::default(),
            permissions: PermissionsConfig::default(),
            subagent: SubagentConfig::default(),
            coordination: CoordinationConfig::default(),
        };
        let yaml = serde_yaml::to_string(&config).unwrap();
        let deserialized: AppConfig = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(deserialized.task, config.task);
        assert_eq!(deserialized.llm.base_url, config.llm.base_url);
        assert_eq!(deserialized.llm.model, config.llm.model);
        assert_eq!(deserialized.llm.max_tokens, config.llm.max_tokens);
    }

    #[test]
    fn test_skills_config_defaults() {
        let config = SkillsConfig::default();
        assert!(config.dirs.is_empty());
        assert!(config.enabled);
        assert_eq!(config.script_timeout_secs, 60);
    }

    #[test]
    fn test_skills_config_yaml_deser() {
        let yaml = r#"
dirs:
  - "/home/user/skills"
  - "/opt/skills"
enabled: false
script_timeout_secs: 120
"#;
        let config: SkillsConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.dirs.len(), 2);
        assert!(!config.enabled);
        assert_eq!(config.script_timeout_secs, 120);
    }

    #[test]
    fn test_app_config_with_skills() {
        let yaml = r#"
task: "test"
skills:
  dirs:
    - "/custom/skills"
  enabled: true
  script_timeout_secs: 30
"#;
        let config: AppConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.skills.dirs.len(), 1);
        assert!(config.skills.enabled);
        assert_eq!(config.skills.script_timeout_secs, 30);
    }

    #[test]
    fn test_app_config_defaults_include_skills() {
        let config = AppConfig::default();
        assert!(config.skills.enabled);
        assert!(config.skills.dirs.is_empty());
        assert_eq!(config.skills.script_timeout_secs, 60);
    }

    #[test]
    fn test_agents_md_config_defaults() {
        let config = AgentsMdConfig::default();
        assert!(config.enabled);
        assert!(config.search_dir.is_none());
        assert_eq!(config.max_size_bytes, 32768);
    }

    #[test]
    fn test_agents_md_config_yaml_deser() {
        let yaml = r#"
enabled: false
search_dir: "/home/user/project"
max_size_bytes: 65536
"#;
        let config: AgentsMdConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(!config.enabled);
        assert_eq!(config.search_dir, Some(PathBuf::from("/home/user/project")));
        assert_eq!(config.max_size_bytes, 65536);
    }

    #[test]
    fn test_app_config_with_agents_md() {
        let yaml = r#"
task: "test"
agents_md:
  enabled: true
  search_dir: "/project"
  max_size_bytes: 16384
"#;
        let config: AppConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.agents_md.enabled);
        assert_eq!(config.agents_md.search_dir, Some(PathBuf::from("/project")));
        assert_eq!(config.agents_md.max_size_bytes, 16384);
    }

    #[test]
    fn test_app_config_defaults_include_agents_md() {
        let config = AppConfig::default();
        assert!(config.agents_md.enabled);
        assert!(config.agents_md.search_dir.is_none());
        assert_eq!(config.agents_md.max_size_bytes, 32768);
    }

    #[test]
    fn test_local_tools_config_defaults() {
        let config = LocalToolsConfig::default();
        assert!(config.enabled);
        assert!(config.sandbox_dir.is_none());
        assert_eq!(config.bash_timeout_secs, 120);
        assert_eq!(config.read_max_bytes, 1_048_576);
        assert_eq!(config.write_max_bytes, 10_485_760);
        assert!(config.bash_allowlist.is_empty());
        assert!(config.bash_blocklist.is_empty());
    }

    #[test]
    fn test_local_tools_config_yaml_deser() {
        let yaml = r#"
enabled: false
sandbox_dir: "/tmp/sandbox"
bash_timeout_secs: 60
read_max_bytes: 524288
write_max_bytes: 2097152
bash_allowlist:
  - "ls"
  - "cat"
bash_blocklist:
  - "rm"
  - "dd"
"#;
        let config: LocalToolsConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(!config.enabled);
        assert_eq!(config.sandbox_dir, Some(PathBuf::from("/tmp/sandbox")));
        assert_eq!(config.bash_timeout_secs, 60);
        assert_eq!(config.read_max_bytes, 524288);
        assert_eq!(config.write_max_bytes, 2097152);
        assert_eq!(config.bash_allowlist, vec!["ls", "cat"]);
        assert_eq!(config.bash_blocklist, vec!["rm", "dd"]);
    }

    #[test]
    fn test_app_config_with_local_tools() {
        let yaml = r#"
task: "test"
local_tools:
  enabled: true
  sandbox_dir: "/tmp/sandbox"
  bash_timeout_secs: 90
  read_max_bytes: 2097152
  write_max_bytes: 5242880
  bash_allowlist:
    - "ls"
  bash_blocklist:
    - "rm"
"#;
        let config: AppConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.local_tools.enabled);
        assert_eq!(
            config.local_tools.sandbox_dir,
            Some(PathBuf::from("/tmp/sandbox"))
        );
        assert_eq!(config.local_tools.bash_timeout_secs, 90);
        assert_eq!(config.local_tools.read_max_bytes, 2097152);
        assert_eq!(config.local_tools.write_max_bytes, 5242880);
        assert_eq!(config.local_tools.bash_allowlist, vec!["ls"]);
        assert_eq!(config.local_tools.bash_blocklist, vec!["rm"]);
    }

    #[test]
    fn test_app_config_defaults_include_local_tools() {
        let config = AppConfig::default();
        assert!(config.local_tools.enabled);
        assert!(config.local_tools.sandbox_dir.is_none());
        assert_eq!(config.local_tools.bash_timeout_secs, 120);
        assert_eq!(config.local_tools.read_max_bytes, 1_048_576);
        assert_eq!(config.local_tools.write_max_bytes, 10_485_760);
        assert!(config.local_tools.bash_allowlist.is_empty());
        assert!(config.local_tools.bash_blocklist.is_empty());
    }

    #[test]
    fn test_plugins_config_defaults() {
        let config = PluginsConfig::default();
        assert!(config.enabled);
        assert!(config.claude_code_cache);
        assert!(config.sources.is_empty());
        assert!(config.components.skills);
        assert!(config.components.mcp_servers);
        assert!(config.components.hooks);
        assert!(config.components.agents);
        assert_eq!(config.hook_timeout_secs, 30);
    }

    #[test]
    fn test_plugins_config_yaml_deser() {
        let yaml = r#"
enabled: false
claude_code_cache: false
hook_timeout_secs: 60
sources:
  - path: "/my/plugin"
  - github: "owner/repo"
    git_ref: "v1.0"
components:
  skills: true
  mcp_servers: false
  hooks: true
  agents: false
"#;
        let config: PluginsConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(!config.enabled);
        assert!(!config.claude_code_cache);
        assert_eq!(config.hook_timeout_secs, 60);
        assert_eq!(config.sources.len(), 2);
        assert_eq!(config.sources[0].path, Some(PathBuf::from("/my/plugin")));
        assert_eq!(config.sources[1].github, Some("owner/repo".to_string()));
        assert_eq!(config.sources[1].git_ref, Some("v1.0".to_string()));
        assert!(config.components.skills);
        assert!(!config.components.mcp_servers);
        assert!(config.components.hooks);
        assert!(!config.components.agents);
    }

    #[test]
    fn test_app_config_with_plugins() {
        let yaml = r#"
task: "test"
plugins:
  enabled: true
  claude_code_cache: false
  sources:
    - path: "./my-plugin"
  components:
    skills: true
    mcp_servers: true
    hooks: false
    agents: false
"#;
        let config: AppConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.plugins.enabled);
        assert!(!config.plugins.claude_code_cache);
        assert_eq!(config.plugins.sources.len(), 1);
        assert!(config.plugins.components.skills);
        assert!(config.plugins.components.mcp_servers);
        assert!(!config.plugins.components.hooks);
        assert!(!config.plugins.components.agents);
    }

    #[test]
    fn test_app_config_defaults_include_plugins() {
        let config = AppConfig::default();
        assert!(config.plugins.enabled);
        assert!(config.plugins.claude_code_cache);
        assert!(config.plugins.sources.is_empty());
        assert!(config.plugins.components.skills);
        assert!(config.plugins.components.mcp_servers);
        assert!(config.plugins.components.hooks);
        assert!(config.plugins.components.agents);
    }

    #[test]
    fn test_plugin_components_config_defaults() {
        let config = PluginComponentsConfig::default();
        assert!(config.skills);
        assert!(config.mcp_servers);
        assert!(config.hooks);
        assert!(config.agents);
    }

    #[test]
    fn test_session_config_defaults() {
        let config = SessionConfig::default();
        assert!(config.enabled);
        assert!(config.storage_dir.ends_with("sessions"));
        assert_eq!(config.max_sessions, 100);
    }

    #[test]
    fn test_session_config_yaml_deser() {
        let yaml = r#"
enabled: false
storage_dir: "/tmp/sessions"
max_sessions: 50
"#;
        let config: SessionConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(!config.enabled);
        assert_eq!(config.storage_dir, PathBuf::from("/tmp/sessions"));
        assert_eq!(config.max_sessions, 50);
    }

    #[test]
    fn test_compaction_config_defaults() {
        let config = CompactionConfig::default();
        assert!(config.enabled);
        assert!((config.trigger_threshold - 0.95).abs() < f32::EPSILON);
        assert_eq!(config.context_window_tokens, 200_000);
        assert_eq!(config.preserve_recent_n, 4);
    }

    #[test]
    fn test_compaction_config_yaml_deser() {
        let yaml = r#"
enabled: false
trigger_threshold: 0.80
context_window_tokens: 100000
preserve_recent_n: 6
"#;
        let config: CompactionConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(!config.enabled);
        assert!((config.trigger_threshold - 0.80).abs() < f32::EPSILON);
        assert_eq!(config.context_window_tokens, 100_000);
        assert_eq!(config.preserve_recent_n, 6);
    }

    #[test]
    fn test_permissions_config_defaults() {
        let config = PermissionsConfig::default();
        assert_eq!(config.mode, PermissionModeConfig::Default);
        assert!(config.allowed_tools.is_empty());
        assert!(config.denied_tools.is_empty());
    }

    #[test]
    fn test_permissions_config_yaml_deser() {
        let yaml = r#"
mode: bypass_permissions
allowed_tools:
  - "navigate"
  - "click"
denied_tools:
  - "bash"
"#;
        let config: PermissionsConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.mode, PermissionModeConfig::BypassPermissions);
        assert_eq!(config.allowed_tools, vec!["navigate", "click"]);
        assert_eq!(config.denied_tools, vec!["bash"]);
    }

    #[test]
    fn test_subagent_config_defaults() {
        let config = SubagentConfig::default();
        assert!(config.enabled);
        assert_eq!(config.max_iterations, 10);
        assert_eq!(config.timeout_secs, 120);
    }

    #[test]
    fn test_subagent_config_yaml_deser() {
        let yaml = r#"
enabled: false
max_iterations: 5
timeout_secs: 60
"#;
        let config: SubagentConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(!config.enabled);
        assert_eq!(config.max_iterations, 5);
        assert_eq!(config.timeout_secs, 60);
    }

    #[test]
    fn test_app_config_defaults_include_new_configs() {
        let config = AppConfig::default();
        assert!(config.session.enabled);
        assert!(config.compaction.enabled);
        assert_eq!(config.permissions.mode, PermissionModeConfig::Default);
        assert!(config.subagent.enabled);
    }

    #[test]
    fn test_app_config_with_new_configs() {
        let yaml = r#"
task: "test"
session:
  enabled: true
  max_sessions: 200
compaction:
  enabled: false
  trigger_threshold: 0.90
permissions:
  mode: plan
  denied_tools:
    - "bash"
subagent:
  enabled: true
  max_iterations: 20
"#;
        let config: AppConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.session.enabled);
        assert_eq!(config.session.max_sessions, 200);
        assert!(!config.compaction.enabled);
        assert_eq!(config.permissions.mode, PermissionModeConfig::Plan);
        assert_eq!(config.permissions.denied_tools, vec!["bash"]);
        assert_eq!(config.subagent.max_iterations, 20);
    }

    #[test]
    fn test_coordination_config_defaults() {
        let config = CoordinationConfig::default();
        assert!(config.enabled);
        assert_eq!(config.max_worker_iterations, 10);
        assert_eq!(config.worker_timeout_secs, 120);
        assert_eq!(config.max_workers, 5);
        assert!(config.storage_dir.ends_with("coordination"));
    }

    #[test]
    fn test_coordination_config_yaml_deser() {
        let yaml = r#"
enabled: false
max_worker_iterations: 20
worker_timeout_secs: 300
max_workers: 10
storage_dir: "/tmp/coordination"
"#;
        let config: CoordinationConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(!config.enabled);
        assert_eq!(config.max_worker_iterations, 20);
        assert_eq!(config.worker_timeout_secs, 300);
        assert_eq!(config.max_workers, 10);
        assert_eq!(config.storage_dir, PathBuf::from("/tmp/coordination"));
    }

    #[test]
    fn test_app_config_with_coordination() {
        let yaml = r#"
task: "test"
coordination:
  enabled: true
  max_workers: 8
  max_worker_iterations: 15
"#;
        let config: AppConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.coordination.enabled);
        assert_eq!(config.coordination.max_workers, 8);
        assert_eq!(config.coordination.max_worker_iterations, 15);
    }

    #[test]
    fn test_app_config_defaults_include_coordination() {
        let config = AppConfig::default();
        assert!(config.coordination.enabled);
        assert_eq!(config.coordination.max_workers, 5);
        assert_eq!(config.coordination.max_worker_iterations, 10);
        assert_eq!(config.coordination.worker_timeout_secs, 120);
    }

    #[test]
    fn test_agent_config_tool_result_max_bytes_default() {
        let config = AgentConfig::default();
        assert_eq!(config.tool_result_max_bytes, 32768);
    }

    #[test]
    fn test_agent_config_max_budget_usd_default() {
        let config = AgentConfig::default();
        assert!(config.max_budget_usd.is_none());
    }

    #[test]
    fn test_agent_config_yaml_with_new_fields() {
        let yaml = r#"
max_iterations: 30
timeout_secs: 120
tool_result_max_bytes: 65536
max_budget_usd: 5.0
"#;
        let config: AgentConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.max_iterations, 30);
        assert_eq!(config.timeout_secs, 120);
        assert_eq!(config.tool_result_max_bytes, 65536);
        assert_eq!(config.max_budget_usd, Some(5.0));
    }

    #[test]
    fn test_agent_config_yaml_defaults_for_new_fields() {
        let yaml = r#"
max_iterations: 20
"#;
        let config: AgentConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.tool_result_max_bytes, 32768);
        assert!(config.max_budget_usd.is_none());
    }

    #[test]
    fn test_permission_mode_dont_ask_variant() {
        let yaml = r#"
mode: dont_ask
allowed_tools: []
denied_tools: []
"#;
        let config: PermissionsConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.mode, PermissionModeConfig::DontAsk);
    }

    #[test]
    fn test_permission_mode_dont_ask_serde_roundtrip() {
        let mode = PermissionModeConfig::DontAsk;
        let json = serde_json::to_value(&mode).unwrap();
        assert_eq!(json, serde_json::json!("dont_ask"));
        let deserialized: PermissionModeConfig = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized, PermissionModeConfig::DontAsk);
    }
}
