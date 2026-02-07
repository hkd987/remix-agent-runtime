pub mod credentials;
pub mod env;
pub mod schema;

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

    // CLI task overrides YAML task
    if args.task.is_some() {
        config.task = args.task.clone();
    }

    // Normalize credentials
    config.credentials = credentials::load_credentials_from_config(&config.credentials);

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;
    use tempfile::NamedTempFile;

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
            verbose: false,
            output: None,
            browser_path: None,
        }
    }

    fn write_yaml_tempfile(content: &str) -> NamedTempFile {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(content.as_bytes()).unwrap();
        file.flush().unwrap();
        file
    }

    #[test]
    fn test_defaults_only() {
        // Clear env vars that could interfere
        std::env::remove_var("REMIX_LLM_BASE_URL");
        std::env::remove_var("REMIX_LLM_API_KEY");
        std::env::remove_var("REMIX_LLM_MODEL");

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
        std::env::remove_var("REMIX_LLM_BASE_URL");
        std::env::remove_var("REMIX_LLM_API_KEY");
        std::env::remove_var("REMIX_LLM_MODEL");

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
        let yaml = r#"
llm:
  base_url: "https://yaml.api.com"
  api_key: "yaml-key"
  model: "yaml-model"
"#;
        let file = write_yaml_tempfile(yaml);

        std::env::set_var("REMIX_LLM_BASE_URL", "https://env.api.com");
        std::env::set_var("REMIX_LLM_API_KEY", "env-key");
        std::env::set_var("REMIX_LLM_MODEL", "env-model");

        let args = RunArgs {
            config: Some(file.path().to_path_buf()),
            ..default_run_args()
        };
        let config = load_config(&args).unwrap();

        assert_eq!(config.llm.base_url, "https://env.api.com");
        assert_eq!(config.llm.api_key, "env-key");
        assert_eq!(config.llm.model, "env-model");

        std::env::remove_var("REMIX_LLM_BASE_URL");
        std::env::remove_var("REMIX_LLM_API_KEY");
        std::env::remove_var("REMIX_LLM_MODEL");
    }

    #[test]
    fn test_cli_flag_override() {
        std::env::remove_var("REMIX_LLM_BASE_URL");
        std::env::remove_var("REMIX_LLM_API_KEY");
        std::env::remove_var("REMIX_LLM_MODEL");

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
            verbose: false,
            output: None,
            browser_path: Some("/custom/path".to_string()),
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
        let yaml = r#"
llm:
  base_url: "https://yaml.api.com"
  api_key: "yaml-key"
  model: "yaml-model"
"#;
        let file = write_yaml_tempfile(yaml);

        // Set env vars (should override YAML)
        std::env::set_var("REMIX_LLM_BASE_URL", "https://env.api.com");
        std::env::set_var("REMIX_LLM_API_KEY", "env-key");
        std::env::set_var("REMIX_LLM_MODEL", "env-model");

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

        std::env::remove_var("REMIX_LLM_BASE_URL");
        std::env::remove_var("REMIX_LLM_API_KEY");
        std::env::remove_var("REMIX_LLM_MODEL");
    }

    #[test]
    fn test_missing_config_file_error() {
        std::env::remove_var("REMIX_LLM_BASE_URL");
        std::env::remove_var("REMIX_LLM_API_KEY");
        std::env::remove_var("REMIX_LLM_MODEL");

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
        std::env::remove_var("REMIX_LLM_BASE_URL");
        std::env::remove_var("REMIX_LLM_API_KEY");
        std::env::remove_var("REMIX_LLM_MODEL");

        std::env::set_var("TEST_CONFIG_API_KEY", "interpolated-key");
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
        std::env::remove_var("TEST_CONFIG_API_KEY");
    }

    #[test]
    fn test_headed_flag_overrides_headless() {
        std::env::remove_var("REMIX_LLM_BASE_URL");
        std::env::remove_var("REMIX_LLM_API_KEY");
        std::env::remove_var("REMIX_LLM_MODEL");

        let args = RunArgs {
            headed: true,
            ..default_run_args()
        };
        let config = load_config(&args).unwrap();
        assert!(!config.browser.headless);
    }

    #[test]
    fn test_headed_false_preserves_default() {
        std::env::remove_var("REMIX_LLM_BASE_URL");
        std::env::remove_var("REMIX_LLM_API_KEY");
        std::env::remove_var("REMIX_LLM_MODEL");

        let args = RunArgs {
            headed: false,
            ..default_run_args()
        };
        let config = load_config(&args).unwrap();
        assert!(config.browser.headless);
    }

    #[test]
    fn test_credentials_normalized_from_yaml() {
        std::env::remove_var("REMIX_LLM_BASE_URL");
        std::env::remove_var("REMIX_LLM_API_KEY");
        std::env::remove_var("REMIX_LLM_MODEL");

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
        let cred = &config.credentials[0];
        assert_eq!(cred.fields.get("username").unwrap(), "admin");
        assert_eq!(cred.fields.get("password").unwrap(), "secret");
        // username/password flat fields should be cleared after normalization
        assert!(cred.username.is_none());
        assert!(cred.password.is_none());
    }

    #[test]
    fn test_cli_task_overrides_yaml_task() {
        std::env::remove_var("REMIX_LLM_BASE_URL");
        std::env::remove_var("REMIX_LLM_API_KEY");
        std::env::remove_var("REMIX_LLM_MODEL");

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
        std::env::remove_var("REMIX_LLM_BASE_URL");
        std::env::remove_var("REMIX_LLM_API_KEY");
        std::env::remove_var("REMIX_LLM_MODEL");

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
    fn test_timeout_applies_to_both_browser_and_agent() {
        std::env::remove_var("REMIX_LLM_BASE_URL");
        std::env::remove_var("REMIX_LLM_API_KEY");
        std::env::remove_var("REMIX_LLM_MODEL");

        let args = RunArgs {
            timeout: Some(999),
            ..default_run_args()
        };
        let config = load_config(&args).unwrap();
        assert_eq!(config.browser.timeout_secs, 999);
        assert_eq!(config.agent.timeout_secs, 999);
    }
}
