use serde_json::Value;

use crate::browser::mcp::ToolExecutionResult;
use crate::config::schema::LocalToolsConfig;
use crate::error::AgentError;
use crate::local_tools::sandbox::BashSandbox;

pub async fn execute_bash(
    args: Value,
    sandbox: &dyn BashSandbox,
    config: &LocalToolsConfig,
) -> Result<ToolExecutionResult, AgentError> {
    let command = args
        .get("command")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AgentError::LocalTool("bash requires 'command' parameter".to_string()))?;

    let timeout = args
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(config.bash_timeout_secs);

    // Check blocklist
    for blocked in &config.bash_blocklist {
        if command.starts_with(blocked)
            || command.contains(&format!(" {blocked}"))
            || command.contains(&format!("|{blocked}"))
            || command.contains(&format!("| {blocked}"))
        {
            return Err(AgentError::LocalTool(format!(
                "Command blocked: matches blocklist entry '{blocked}'"
            )));
        }
    }

    // Check allowlist (if non-empty, only allowed commands can run)
    if !config.bash_allowlist.is_empty() {
        let allowed = config.bash_allowlist.iter().any(|a| command.starts_with(a));
        if !allowed {
            return Err(AgentError::LocalTool(
                "Command not in allowlist".to_string(),
            ));
        }
    }

    let output = sandbox.execute(command, timeout).await?;

    let mut result = String::new();
    if !output.stdout.is_empty() {
        result.push_str(&output.stdout);
    }
    if !output.stderr.is_empty() {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str("[stderr] ");
        result.push_str(&output.stderr);
    }

    if output.exit_code != 0 {
        Ok(ToolExecutionResult {
            content: format!("Exit code: {}\n{}", output.exit_code, result),
            is_error: true,
        })
    } else {
        Ok(ToolExecutionResult {
            content: result,
            is_error: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_tools::sandbox::CommandOutput;
    use async_trait::async_trait;
    use serde_json::json;
    use std::path::{Path, PathBuf};

    struct MockSandbox {
        root: PathBuf,
        response: CommandOutput,
    }

    impl MockSandbox {
        fn new(response: CommandOutput) -> Self {
            Self {
                root: PathBuf::from("/tmp/mock"),
                response,
            }
        }
    }

    #[async_trait]
    impl BashSandbox for MockSandbox {
        async fn execute(&self, _cmd: &str, _timeout: u64) -> Result<CommandOutput, AgentError> {
            Ok(self.response.clone())
        }

        fn root(&self) -> &Path {
            &self.root
        }
    }

    fn default_config() -> LocalToolsConfig {
        LocalToolsConfig::default()
    }

    #[tokio::test]
    async fn test_bash_basic_command() {
        let sandbox = MockSandbox::new(CommandOutput {
            stdout: "hello world\n".to_string(),
            stderr: String::new(),
            exit_code: 0,
        });
        let config = default_config();

        let result = execute_bash(json!({"command": "echo hello world"}), &sandbox, &config)
            .await
            .unwrap();
        assert_eq!(result.content, "hello world\n");
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_bash_with_custom_timeout() {
        let sandbox = MockSandbox::new(CommandOutput {
            stdout: "done".to_string(),
            stderr: String::new(),
            exit_code: 0,
        });
        let config = default_config();

        let result = execute_bash(
            json!({"command": "sleep 1", "timeout_secs": 30}),
            &sandbox,
            &config,
        )
        .await
        .unwrap();
        assert_eq!(result.content, "done");
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_bash_blocklist_match() {
        let sandbox = MockSandbox::new(CommandOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
        });
        let config = LocalToolsConfig {
            bash_blocklist: vec!["rm".to_string()],
            ..Default::default()
        };

        let result = execute_bash(json!({"command": "rm -rf /"}), &sandbox, &config).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Command blocked"));
        assert!(err.contains("rm"));
    }

    #[tokio::test]
    async fn test_bash_blocklist_pipe() {
        let sandbox = MockSandbox::new(CommandOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
        });
        let config = LocalToolsConfig {
            bash_blocklist: vec!["rm".to_string()],
            ..Default::default()
        };

        let result =
            execute_bash(json!({"command": "echo test | rm -rf"}), &sandbox, &config).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Command blocked"));
    }

    #[tokio::test]
    async fn test_bash_allowlist_allowed() {
        let sandbox = MockSandbox::new(CommandOutput {
            stdout: "file1\nfile2\n".to_string(),
            stderr: String::new(),
            exit_code: 0,
        });
        let config = LocalToolsConfig {
            bash_allowlist: vec!["ls".to_string(), "cat".to_string()],
            ..Default::default()
        };

        let result = execute_bash(json!({"command": "ls -la"}), &sandbox, &config)
            .await
            .unwrap();
        assert_eq!(result.content, "file1\nfile2\n");
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_bash_allowlist_denied() {
        let sandbox = MockSandbox::new(CommandOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
        });
        let config = LocalToolsConfig {
            bash_allowlist: vec!["ls".to_string(), "cat".to_string()],
            ..Default::default()
        };

        let result = execute_bash(json!({"command": "rm -rf /"}), &sandbox, &config).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not in allowlist"));
    }

    #[tokio::test]
    async fn test_bash_empty_allowlist_allows_all() {
        let sandbox = MockSandbox::new(CommandOutput {
            stdout: "anything".to_string(),
            stderr: String::new(),
            exit_code: 0,
        });
        let config = LocalToolsConfig {
            bash_allowlist: Vec::new(),
            ..Default::default()
        };

        let result = execute_bash(json!({"command": "any-command"}), &sandbox, &config)
            .await
            .unwrap();
        assert_eq!(result.content, "anything");
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_bash_nonzero_exit_code() {
        let sandbox = MockSandbox::new(CommandOutput {
            stdout: String::new(),
            stderr: "error occurred".to_string(),
            exit_code: 1,
        });
        let config = default_config();

        let result = execute_bash(json!({"command": "false"}), &sandbox, &config)
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Exit code: 1"));
        assert!(result.content.contains("[stderr] error occurred"));
    }

    #[tokio::test]
    async fn test_bash_missing_command_param() {
        let sandbox = MockSandbox::new(CommandOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
        });
        let config = default_config();

        let result = execute_bash(json!({}), &sandbox, &config).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("requires 'command' parameter"));
    }

    #[tokio::test]
    async fn test_bash_stderr_output() {
        let sandbox = MockSandbox::new(CommandOutput {
            stdout: "stdout line".to_string(),
            stderr: "stderr line".to_string(),
            exit_code: 0,
        });
        let config = default_config();

        let result = execute_bash(json!({"command": "test"}), &sandbox, &config)
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("stdout line"));
        assert!(result.content.contains("[stderr] stderr line"));
    }
}
