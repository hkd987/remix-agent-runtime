pub mod credentials;
pub mod env;
pub mod schema;

use std::path::PathBuf;

use crate::cli::RunArgs;
use crate::error::AgentError;
use schema::AppConfig;

/// Loads and merges configuration from multiple sources.
///
/// Merge priority (highest to lowest):
/// 1. CLI flags
/// 2. Environment variables (REMIX_LLM_BASE_URL, REMIX_LLM_API_KEY, REMIX_LLM_MODEL)
/// 3. YAML config file
/// 4. Defaults
pub fn load_config(args: &RunArgs) -> Result<AppConfig, AgentError> {
    // Start with defaults
    let mut config = AppConfig::default();

    // If --config provided, parse YAML with env var interpolation and merge
    if let Some(ref config_path) = args.config {
        let raw_yaml = std::fs::read_to_string(config_path).map_err(|e| {
            AgentError::Config(format!(
                "Failed to read config file '{}': {}",
                config_path.display(),
                e
            ))
        })?;
        let interpolated = env::interpolate_env_vars(&raw_yaml)?;
        config = serde_yaml::from_str(&interpolated)?;
    }

    // Apply environment variables (override YAML values)
    if let Ok(val) = std::env::var("REMIX_LLM_BASE_URL") {
        config.llm.base_url = val;
    }
    if let Ok(val) = std::env::var("REMIX_LLM_API_KEY") {
        config.llm.api_key = val;
    }
    if let Ok(val) = std::env::var("REMIX_LLM_MODEL") {
        config.llm.model = val;
    }

    // Apply CLI flags (highest priority)
    if let Some(ref base_url) = args.base_url {
        config.llm.base_url = base_url.clone();
    }
    if let Some(ref api_key) = args.api_key {
        config.llm.api_key = api_key.clone();
    }
    if let Some(ref model) = args.model {
        config.llm.model = model.clone();
    }
    if let Some(max_tokens) = args.max_tokens {
        config.llm.max_tokens = max_tokens;
    }
    if let Some(timeout) = args.timeout {
        config.browser.timeout_secs = timeout;
        config.agent.timeout_secs = timeout;
    }
    if let Some(max_iterations) = args.max_iterations {
        config.agent.max_iterations = max_iterations;
    }
    if args.headed {
        config.browser.headless = false;
    }
    if let Some(ref browser_path) = args.browser_path {
        config.browser.browser_path = Some(browser_path.clone());
    }
    if args.no_browser {
        config.browser.enabled = false;
    }

    // Apply skills configuration
    if let Some(ref skills_dir) = args.skills_dir {
        config.skills.dirs.push(skills_dir.clone());
    }
    if args.no_skills {
        config.skills.enabled = false;
    }
    if let Ok(val) = std::env::var("REMIX_SKILLS_DIR") {
        let path = PathBuf::from(val);
        if !config.skills.dirs.contains(&path) {
            config.skills.dirs.push(path);
        }
    }

    // Apply agents_md configuration
    if let Some(ref dir) = args.agents_md_dir {
        config.agents_md.search_dir = Some(dir.clone());
    }
    if args.no_agents_md {
        config.agents_md.enabled = false;
    }
    if let Ok(val) = std::env::var("REMIX_AGENTS_MD_DIR") {
        if config.agents_md.search_dir.is_none() {
            config.agents_md.search_dir = Some(PathBuf::from(val));
        }
    }

    // Apply local_tools configuration
    if let Some(ref dir) = args.sandbox_dir {
        config.local_tools.sandbox_dir = Some(dir.clone());
    }
    if args.no_local_tools {
        config.local_tools.enabled = false;
    }
    if let Ok(val) = std::env::var("REMIX_SANDBOX_DIR") {
        if config.local_tools.sandbox_dir.is_none() {
            config.local_tools.sandbox_dir = Some(PathBuf::from(val));
        }
    }

    // Apply plugins configuration
    if let Some(ref dir) = args.plugins_dir {
        config.plugins.sources.push(schema::PluginSourceConfig {
            path: Some(dir.clone()),
            github: None,
            git_ref: None,
        });
    }
    if args.no_plugins {
        config.plugins.enabled = false;
    }
    if args.no_claude_plugins {
        config.plugins.claude_code_cache = false;
    }
    if let Ok(val) = std::env::var("REMIX_PLUGINS_DIR") {
        let path = PathBuf::from(val);
        let already_present = config
            .plugins
            .sources
            .iter()
            .any(|s| s.path.as_ref() == Some(&path));
        if !already_present {
            config.plugins.sources.push(schema::PluginSourceConfig {
                path: Some(path),
                github: None,
                git_ref: None,
            });
        }
    }

    // Apply session configuration
    if let Some(ref dir) = args.session_dir {
        config.session.storage_dir = dir.clone();
    }
    if let Ok(val) = std::env::var("REMIX_SESSION_DIR") {
        if args.session_dir.is_none() {
            config.session.storage_dir = PathBuf::from(val);
        }
    }

    // Apply permissions configuration
    if let Some(ref mode_str) = args.permission_mode {
        match mode_str.as_str() {
            "default" => config.permissions.mode = schema::PermissionModeConfig::Default,
            "accept_edits" => config.permissions.mode = schema::PermissionModeConfig::AcceptEdits,
            "bypass_permissions" => {
                config.permissions.mode = schema::PermissionModeConfig::BypassPermissions
            }
            "plan" => config.permissions.mode = schema::PermissionModeConfig::Plan,
            _ => {
                return Err(AgentError::Config(format!(
                    "Invalid permission mode: '{mode_str}'. Must be: default, accept_edits, bypass_permissions, plan"
                )));
            }
        }
    }
    if !args.allow_tool.is_empty() {
        config
            .permissions
            .allowed_tools
            .extend(args.allow_tool.clone());
    }
    if !args.deny_tool.is_empty() {
        config
            .permissions
            .denied_tools
            .extend(args.deny_tool.clone());
    }

    // Apply coordination configuration
    if args.no_coordination {
        config.coordination.enabled = false;
    }
    if let Some(max_workers) = args.max_workers {
        config.coordination.max_workers = max_workers;
    }
    if let Some(ref dir) = args.coordination_dir {
        config.coordination.storage_dir = dir.clone();
    }
    if let Ok(val) = std::env::var("REMIX_COORDINATION_DIR") {
        if args.coordination_dir.is_none() {
            config.coordination.storage_dir = PathBuf::from(val);
        }
    }

    // CLI task overrides YAML task
    if args.task.is_some() {
        config.task = args.task.clone();
    }

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use tempfile::NamedTempFile;

    /// Mutex to serialize tests that read/write process-wide environment variables.
    /// `std::env::set_var` / `remove_var` are not thread-safe; concurrent tests that
    /// modify the same env vars (REMIX_LLM_*) will race without this lock.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn default_run_args() -> RunArgs {
        RunArgs {
            task: None,
            config: None,
            base_url: None,
            api_key: None,
            model: None,
            max_tokens: None,
            timeout: None,
            max_iterations: None,
            headed: false,
            no_browser: false,
            verbose: false,
            output: None,
            browser_path: None,
            skills_dir: None,
            no_skills: false,
            agents_md_dir: None,
            no_agents_md: false,
            no_local_tools: false,
            sandbox_dir: None,
            no_plugins: false,
            plugins_dir: None,
            no_claude_plugins: false,
            session_id: None,
            fork_session: None,
            session_dir: None,
            permission_mode: None,
            allow_tool: Vec::new(),
            deny_tool: Vec::new(),
            no_coordination: false,
            max_workers: None,
            coordination_dir: None,
            continue_session: false,
            effort: None,
            sse_port: None,
        }
    }

    fn write_yaml_tempfile(content: &str) -> NamedTempFile {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(content.as_bytes()).unwrap();
        file.flush().unwrap();
        file
    }

    /// Clear all REMIX_* env vars that could interfere with config tests.
    /// Must be called while holding `ENV_LOCK`.
    fn clear_env_vars() {
        unsafe {
            std::env::remove_var("REMIX_LLM_BASE_URL");
            std::env::remove_var("REMIX_LLM_API_KEY");
            std::env::remove_var("REMIX_LLM_MODEL");
            std::env::remove_var("REMIX_SKILLS_DIR");
            std::env::remove_var("REMIX_AGENTS_MD_DIR");
            std::env::remove_var("REMIX_SANDBOX_DIR");
            std::env::remove_var("REMIX_PLUGINS_DIR");
            std::env::remove_var("REMIX_SESSION_DIR");
        }
    }

    #[test]
    fn test_defaults_only() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env_vars();

        let args = default_run_args();
        let config = load_config(&args).unwrap();
        assert!(config.task.is_none());
        assert_eq!(config.llm.base_url, "https://api.anthropic.com");
        assert_eq!(config.llm.model, "claude-sonnet-4-20250514");
        assert_eq!(config.llm.max_tokens, 8192);
        assert!(config.browser.headless);
        assert_eq!(config.agent.max_iterations, 50);
    }

    #[test]
    fn test_yaml_parsing() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env_vars();
        let yaml = r#"
task: "navigate to example.com"
llm:
  base_url: "https://yaml.api.com"
  api_key: "yaml-key"
  model: "yaml-model"
  max_tokens: 2048
browser:
  headless: false
  timeout_secs: 600
agent:
  max_iterations: 25
"#;
        let file = write_yaml_tempfile(yaml);
        let args = RunArgs {
            config: Some(file.path().to_path_buf()),
            ..default_run_args()
        };
        let config = load_config(&args).unwrap();
        assert_eq!(config.task.as_deref(), Some("navigate to example.com"));
        assert_eq!(config.llm.base_url, "https://yaml.api.com");
        assert_eq!(config.llm.api_key, "yaml-key");
        assert_eq!(config.llm.model, "yaml-model");
        assert_eq!(config.llm.max_tokens, 2048);
        assert!(!config.browser.headless);
        assert_eq!(config.browser.timeout_secs, 600);
        assert_eq!(config.agent.max_iterations, 25);
    }

    #[test]
    fn test_env_var_override() {
        let _guard = ENV_LOCK.lock().unwrap();

        let yaml = r#"
llm:
  base_url: "https://yaml.api.com"
  api_key: "yaml-key"
  model: "yaml-model"
"#;
        let file = write_yaml_tempfile(yaml);

        unsafe {
            std::env::set_var("REMIX_LLM_BASE_URL", "https://env.api.com");
            std::env::set_var("REMIX_LLM_API_KEY", "env-key");
            std::env::set_var("REMIX_LLM_MODEL", "env-model");
        }

        let args = RunArgs {
            config: Some(file.path().to_path_buf()),
            ..default_run_args()
        };
        let config = load_config(&args).unwrap();

        assert_eq!(config.llm.base_url, "https://env.api.com");
        assert_eq!(config.llm.api_key, "env-key");
        assert_eq!(config.llm.model, "env-model");

        unsafe {
            std::env::remove_var("REMIX_LLM_BASE_URL");
            std::env::remove_var("REMIX_LLM_API_KEY");
            std::env::remove_var("REMIX_LLM_MODEL");
        }
    }

    #[test]
    fn test_cli_flag_override() {
        let _guard = ENV_LOCK.lock().unwrap();

        unsafe {
            std::env::remove_var("REMIX_LLM_BASE_URL");
            std::env::remove_var("REMIX_LLM_API_KEY");
            std::env::remove_var("REMIX_LLM_MODEL");
        }

        let yaml = r#"
task: "yaml task"
llm:
  base_url: "https://yaml.api.com"
  api_key: "yaml-key"
  model: "yaml-model"
  max_tokens: 2048
browser:
  timeout_secs: 600
agent:
  max_iterations: 25
"#;
        let file = write_yaml_tempfile(yaml);
        let args = RunArgs {
            task: Some("cli task".to_string()),
            config: Some(file.path().to_path_buf()),
            base_url: Some("https://cli.api.com".to_string()),
            api_key: Some("cli-key".to_string()),
            model: Some("cli-model".to_string()),
            max_tokens: Some(4096),
            timeout: Some(120),
            max_iterations: Some(10),
            headed: true,
            no_browser: false,
            verbose: false,
            output: None,
            browser_path: Some("/custom/path".to_string()),
            skills_dir: None,
            no_skills: false,
            agents_md_dir: None,
            no_agents_md: false,
            no_local_tools: false,
            sandbox_dir: None,
            no_plugins: false,
            plugins_dir: None,
            no_claude_plugins: false,
            session_id: None,
            fork_session: None,
            session_dir: None,
            permission_mode: None,
            allow_tool: Vec::new(),
            deny_tool: Vec::new(),
            no_coordination: false,
            max_workers: None,
            coordination_dir: None,
            continue_session: false,
            effort: None,
            sse_port: None,
        };
        let config = load_config(&args).unwrap();

        assert_eq!(config.task.as_deref(), Some("cli task"));
        assert_eq!(config.llm.base_url, "https://cli.api.com");
        assert_eq!(config.llm.api_key, "cli-key");
        assert_eq!(config.llm.model, "cli-model");
        assert_eq!(config.llm.max_tokens, 4096);
        assert_eq!(config.browser.timeout_secs, 120);
        assert_eq!(config.agent.timeout_secs, 120);
        assert_eq!(config.agent.max_iterations, 10);
        assert!(!config.browser.headless);
        assert_eq!(config.browser.browser_path.as_deref(), Some("/custom/path"));
    }

    #[test]
    fn test_merge_priority_cli_over_env_over_yaml() {
        let _guard = ENV_LOCK.lock().unwrap();

        let yaml = r#"
llm:
  base_url: "https://yaml.api.com"
  api_key: "yaml-key"
  model: "yaml-model"
"#;
        let file = write_yaml_tempfile(yaml);

        // Set env vars (should override YAML)
        unsafe {
            std::env::set_var("REMIX_LLM_BASE_URL", "https://env.api.com");
            std::env::set_var("REMIX_LLM_API_KEY", "env-key");
            std::env::set_var("REMIX_LLM_MODEL", "env-model");
        }

        // Set CLI flags (should override env)
        let args = RunArgs {
            config: Some(file.path().to_path_buf()),
            base_url: Some("https://cli.api.com".to_string()),
            api_key: Some("cli-key".to_string()),
            model: Some("cli-model".to_string()),
            ..default_run_args()
        };
        let config = load_config(&args).unwrap();

        // CLI should win over env and YAML
        assert_eq!(config.llm.base_url, "https://cli.api.com");
        assert_eq!(config.llm.api_key, "cli-key");
        assert_eq!(config.llm.model, "cli-model");

        unsafe {
            std::env::remove_var("REMIX_LLM_BASE_URL");
            std::env::remove_var("REMIX_LLM_API_KEY");
            std::env::remove_var("REMIX_LLM_MODEL");
        }
    }

    #[test]
    fn test_missing_config_file_error() {
        let _guard = ENV_LOCK.lock().unwrap();

        unsafe {
            std::env::remove_var("REMIX_LLM_BASE_URL");
            std::env::remove_var("REMIX_LLM_API_KEY");
            std::env::remove_var("REMIX_LLM_MODEL");
        }

        let args = RunArgs {
            config: Some(PathBuf::from("/nonexistent/path/config.yaml")),
            ..default_run_args()
        };
        let result = load_config(&args);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Failed to read config file"));
    }

    #[test]
    fn test_invalid_yaml_error() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env_vars();

        let file = write_yaml_tempfile("{{{{invalid yaml content");
        let args = RunArgs {
            config: Some(file.path().to_path_buf()),
            ..default_run_args()
        };
        let result = load_config(&args);
        assert!(result.is_err());
    }

    #[test]
    fn test_yaml_env_var_interpolation() {
        let _guard = ENV_LOCK.lock().unwrap();

        unsafe {
            std::env::remove_var("REMIX_LLM_BASE_URL");
            std::env::remove_var("REMIX_LLM_API_KEY");
            std::env::remove_var("REMIX_LLM_MODEL");
            std::env::set_var("TEST_CONFIG_API_KEY", "interpolated-key");
        }

        let yaml = r#"
llm:
  api_key: "${TEST_CONFIG_API_KEY}"
"#;
        let file = write_yaml_tempfile(yaml);
        let args = RunArgs {
            config: Some(file.path().to_path_buf()),
            ..default_run_args()
        };
        let config = load_config(&args).unwrap();
        assert_eq!(config.llm.api_key, "interpolated-key");
        unsafe {
            std::env::remove_var("TEST_CONFIG_API_KEY");
        }
    }

    #[test]
    fn test_headed_flag_overrides_headless() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env_vars();

        let args = RunArgs {
            headed: true,
            ..default_run_args()
        };
        let config = load_config(&args).unwrap();
        assert!(!config.browser.headless);
    }

    #[test]
    fn test_no_browser_flag() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env_vars();

        let args = RunArgs {
            no_browser: true,
            ..default_run_args()
        };
        let config = load_config(&args).unwrap();
        assert!(!config.browser.enabled);
    }

    #[test]
    fn test_no_browser_false_preserves_default() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env_vars();

        let args = default_run_args();
        let config = load_config(&args).unwrap();
        assert!(config.browser.enabled);
    }

    #[test]
    fn test_headed_false_preserves_default() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env_vars();

        let args = RunArgs {
            headed: false,
            ..default_run_args()
        };
        let config = load_config(&args).unwrap();
        assert!(config.browser.headless);
    }

    #[test]
    fn test_credentials_normalized_from_yaml() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env_vars();

        let yaml = r#"
credentials:
  - name: "test_login"
    credential_type: username_password
    username: "admin"
    password: "secret"
    url_pattern: "*.test.com"
"#;
        let file = write_yaml_tempfile(yaml);
        let args = RunArgs {
            config: Some(file.path().to_path_buf()),
            ..default_run_args()
        };
        let config = load_config(&args).unwrap();
        assert_eq!(config.credentials.len(), 1);
        let raw = &config.credentials[0];
        // Raw credentials preserve flat fields — normalization happens at conversion boundary
        assert_eq!(raw.username.as_deref(), Some("admin"));
        assert_eq!(raw.password.as_deref(), Some("secret"));
        assert_eq!(raw.url_pattern.as_deref(), Some("*.test.com"));

        // Verify they convert correctly to real CredentialSet
        let set = credentials::convert_raw_credentials(&config.credentials).unwrap();
        let cred = set.get("test_login").unwrap();
        assert_eq!(cred.field("username").unwrap().expose(), "admin");
        assert_eq!(cred.field("password").unwrap().expose(), "secret");
    }

    #[test]
    fn test_cli_task_overrides_yaml_task() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env_vars();

        let yaml = r#"
task: "yaml task"
"#;
        let file = write_yaml_tempfile(yaml);
        let args = RunArgs {
            task: Some("cli task".to_string()),
            config: Some(file.path().to_path_buf()),
            ..default_run_args()
        };
        let config = load_config(&args).unwrap();
        assert_eq!(config.task.as_deref(), Some("cli task"));
    }

    #[test]
    fn test_no_cli_task_preserves_yaml_task() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env_vars();

        let yaml = r#"
task: "yaml task"
"#;
        let file = write_yaml_tempfile(yaml);
        let args = RunArgs {
            config: Some(file.path().to_path_buf()),
            ..default_run_args()
        };
        let config = load_config(&args).unwrap();
        assert_eq!(config.task.as_deref(), Some("yaml task"));
    }

    #[test]
    fn test_skills_dir_from_cli() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env_vars();

        let args = RunArgs {
            skills_dir: Some(PathBuf::from("/custom/skills")),
            ..default_run_args()
        };
        let config = load_config(&args).unwrap();
        assert!(config
            .skills
            .dirs
            .contains(&PathBuf::from("/custom/skills")));
        assert!(config.skills.enabled);
    }

    #[test]
    fn test_no_skills_flag() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env_vars();

        let args = RunArgs {
            no_skills: true,
            ..default_run_args()
        };
        let config = load_config(&args).unwrap();
        assert!(!config.skills.enabled);
    }

    #[test]
    fn test_skills_dir_from_env() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env_vars();

        unsafe {
            std::env::set_var("REMIX_SKILLS_DIR", "/env/skills");
        }
        let args = default_run_args();
        let config = load_config(&args).unwrap();
        assert!(config.skills.dirs.contains(&PathBuf::from("/env/skills")));
        unsafe {
            std::env::remove_var("REMIX_SKILLS_DIR");
        }
    }

    #[test]
    fn test_skills_dir_cli_and_env_no_duplicates() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env_vars();

        unsafe {
            std::env::set_var("REMIX_SKILLS_DIR", "/same/path");
        }
        let args = RunArgs {
            skills_dir: Some(PathBuf::from("/same/path")),
            ..default_run_args()
        };
        let config = load_config(&args).unwrap();
        let count = config
            .skills
            .dirs
            .iter()
            .filter(|d| d == &&PathBuf::from("/same/path"))
            .count();
        assert_eq!(count, 1);
        unsafe {
            std::env::remove_var("REMIX_SKILLS_DIR");
        }
    }

    #[test]
    fn test_agents_md_dir_from_cli() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env_vars();

        let args = RunArgs {
            agents_md_dir: Some(PathBuf::from("/custom/agents")),
            ..default_run_args()
        };
        let config = load_config(&args).unwrap();
        assert_eq!(
            config.agents_md.search_dir,
            Some(PathBuf::from("/custom/agents"))
        );
        assert!(config.agents_md.enabled);
    }

    #[test]
    fn test_no_agents_md_flag() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env_vars();

        let args = RunArgs {
            no_agents_md: true,
            ..default_run_args()
        };
        let config = load_config(&args).unwrap();
        assert!(!config.agents_md.enabled);
    }

    #[test]
    fn test_sandbox_dir_from_cli() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env_vars();

        let args = RunArgs {
            sandbox_dir: Some(PathBuf::from("/custom/sandbox")),
            ..default_run_args()
        };
        let config = load_config(&args).unwrap();
        assert_eq!(
            config.local_tools.sandbox_dir,
            Some(PathBuf::from("/custom/sandbox"))
        );
        assert!(config.local_tools.enabled);
    }

    #[test]
    fn test_no_local_tools_flag() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env_vars();

        let args = RunArgs {
            no_local_tools: true,
            ..default_run_args()
        };
        let config = load_config(&args).unwrap();
        assert!(!config.local_tools.enabled);
    }

    #[test]
    fn test_skills_config_from_yaml() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env_vars();

        let yaml = r#"
task: "test"
skills:
  dirs:
    - "/yaml/skills"
  enabled: false
  script_timeout_secs: 120
"#;
        let file = write_yaml_tempfile(yaml);
        let args = RunArgs {
            config: Some(file.path().to_path_buf()),
            ..default_run_args()
        };
        let config = load_config(&args).unwrap();
        assert!(config.skills.dirs.contains(&PathBuf::from("/yaml/skills")));
        assert!(!config.skills.enabled);
        assert_eq!(config.skills.script_timeout_secs, 120);
    }

    #[test]
    fn test_timeout_applies_to_both_browser_and_agent() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env_vars();

        let args = RunArgs {
            timeout: Some(999),
            ..default_run_args()
        };
        let config = load_config(&args).unwrap();
        assert_eq!(config.browser.timeout_secs, 999);
        assert_eq!(config.agent.timeout_secs, 999);
    }

    #[test]
    fn test_no_plugins_flag() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env_vars();

        let args = RunArgs {
            no_plugins: true,
            ..default_run_args()
        };
        let config = load_config(&args).unwrap();
        assert!(!config.plugins.enabled);
    }

    #[test]
    fn test_no_claude_plugins_flag() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env_vars();

        let args = RunArgs {
            no_claude_plugins: true,
            ..default_run_args()
        };
        let config = load_config(&args).unwrap();
        assert!(!config.plugins.claude_code_cache);
        assert!(config.plugins.enabled);
    }

    #[test]
    fn test_plugins_dir_from_cli() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env_vars();

        let args = RunArgs {
            plugins_dir: Some(PathBuf::from("/custom/plugins")),
            ..default_run_args()
        };
        let config = load_config(&args).unwrap();
        assert!(config
            .plugins
            .sources
            .iter()
            .any(|s| s.path == Some(PathBuf::from("/custom/plugins"))));
    }

    #[test]
    fn test_plugins_dir_from_env() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env_vars();

        unsafe {
            std::env::set_var("REMIX_PLUGINS_DIR", "/env/plugins");
        }
        let args = default_run_args();
        let config = load_config(&args).unwrap();
        assert!(config
            .plugins
            .sources
            .iter()
            .any(|s| s.path == Some(PathBuf::from("/env/plugins"))));
        unsafe {
            std::env::remove_var("REMIX_PLUGINS_DIR");
        }
    }

    #[test]
    fn test_plugins_config_from_yaml() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env_vars();

        let yaml = r#"
task: "test"
plugins:
  enabled: true
  claude_code_cache: false
  sources:
    - path: "/yaml/plugins"
    - github: "owner/repo"
      git_ref: "main"
  components:
    skills: true
    mcp_servers: false
    hooks: true
    agents: false
"#;
        let file = write_yaml_tempfile(yaml);
        let args = RunArgs {
            config: Some(file.path().to_path_buf()),
            ..default_run_args()
        };
        let config = load_config(&args).unwrap();
        assert!(config.plugins.enabled);
        assert!(!config.plugins.claude_code_cache);
        assert_eq!(config.plugins.sources.len(), 2);
        assert!(config.plugins.components.skills);
        assert!(!config.plugins.components.mcp_servers);
    }

    #[test]
    fn test_session_dir_from_cli() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env_vars();

        let args = RunArgs {
            session_dir: Some(PathBuf::from("/custom/sessions")),
            ..default_run_args()
        };
        let config = load_config(&args).unwrap();
        assert_eq!(
            config.session.storage_dir,
            PathBuf::from("/custom/sessions")
        );
    }

    #[test]
    fn test_permission_mode_from_cli() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env_vars();

        let args = RunArgs {
            permission_mode: Some("bypass_permissions".to_string()),
            ..default_run_args()
        };
        let config = load_config(&args).unwrap();
        assert_eq!(
            config.permissions.mode,
            schema::PermissionModeConfig::BypassPermissions
        );
    }

    #[test]
    fn test_invalid_permission_mode_error() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env_vars();

        let args = RunArgs {
            permission_mode: Some("invalid_mode".to_string()),
            ..default_run_args()
        };
        let result = load_config(&args);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid permission mode"));
    }

    #[test]
    fn test_allow_deny_tools_from_cli() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env_vars();

        let args = RunArgs {
            allow_tool: vec!["navigate".to_string(), "click".to_string()],
            deny_tool: vec!["bash".to_string()],
            ..default_run_args()
        };
        let config = load_config(&args).unwrap();
        assert_eq!(config.permissions.allowed_tools, vec!["navigate", "click"]);
        assert_eq!(config.permissions.denied_tools, vec!["bash"]);
    }
}
