use crate::browser::mcp::ToolExecutionResult;
use crate::config::schema::TestHarnessConfig;
use crate::error::AgentError;
use crate::local_tools::tools::output_filter::{strip_ansi, truncate_output};

use super::detect::detect_frameworks;
use super::types::TestFramework;

pub async fn execute_list_tests(
    arguments: serde_json::Value,
    config: &TestHarnessConfig,
) -> Result<ToolExecutionResult, AgentError> {
    let framework_str = arguments.get("framework").and_then(|v| v.as_str());
    let file = arguments.get("file").and_then(|v| v.as_str());

    let framework = if let Some(fw_str) = framework_str {
        TestFramework::from_str_id(fw_str).ok_or_else(|| {
            AgentError::ToolExecution(format!(
                "Unknown test framework: '{}'. Supported: cargo, pytest, jest, vitest, go",
                fw_str
            ))
        })?
    } else {
        let cwd = std::env::current_dir().unwrap_or_default();
        let detected = detect_frameworks(&cwd);
        detected.into_iter().next().ok_or_else(|| {
            AgentError::ToolExecution(
                "No test framework detected. Specify framework explicitly.".to_string(),
            )
        })?
    };

    let (cmd, base_args) = match framework.list_command() {
        Some(c) => c,
        None => {
            return Ok(ToolExecutionResult {
                content: format!(
                    "{} does not support listing tests without running them.",
                    framework.display_name()
                ),
                is_error: false,
            });
        }
    };

    let mut args: Vec<String> = base_args.iter().map(|s| s.to_string()).collect();
    framework.apply_filters(&mut args, file, None);

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(config.timeout_secs),
        tokio::process::Command::new(cmd).args(&args).output(),
    )
    .await
    .map_err(|_| AgentError::ToolExecution("Test list timed out".to_string()))?
    .map_err(|e| AgentError::ToolExecution(format!("Failed to execute {}: {}", cmd, e)))?;

    let stdout = strip_ansi(&String::from_utf8_lossy(&output.stdout));
    let stderr = strip_ansi(&String::from_utf8_lossy(&output.stderr));

    let combined = if stderr.is_empty() {
        stdout
    } else {
        format!("{}\n{}", stdout, stderr)
    };

    let content = truncate_output(&combined, 32_768);

    Ok(ToolExecutionResult {
        content: format!(
            "Tests ({}){}\n\n{}",
            framework.display_name(),
            file.map(|f| format!(" in {}", f)).unwrap_or_default(),
            content
        ),
        is_error: !output.status.success(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_framework_list_commands_exist() {
        // Verify frameworks we support have list commands
        assert!(TestFramework::Cargo.list_command().is_some());
        assert!(TestFramework::Pytest.list_command().is_some());
        assert!(TestFramework::Jest.list_command().is_some());
        assert!(TestFramework::Go.list_command().is_some());
    }
}
