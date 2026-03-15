use crate::browser::mcp::ToolExecutionResult;
use crate::config::schema::TestHarnessConfig;
use crate::error::AgentError;
use crate::local_tools::tools::output_filter::{strip_ansi, truncate_output};

use super::detect::detect_frameworks;
use super::types::TestFramework;

pub async fn execute_run_tests(
    arguments: serde_json::Value,
    config: &TestHarnessConfig,
) -> Result<ToolExecutionResult, AgentError> {
    let framework_str = arguments.get("framework").and_then(|v| v.as_str());
    let file = arguments.get("file").and_then(|v| v.as_str());
    let test_name = arguments.get("test_name").and_then(|v| v.as_str());
    let timeout_secs = arguments
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(config.timeout_secs);

    let framework = if let Some(fw_str) = framework_str {
        TestFramework::from_str_id(fw_str).ok_or_else(|| {
            AgentError::ToolExecution(format!(
                "Unknown test framework: '{}'. Supported: cargo, pytest, jest, vitest, go, mocha",
                fw_str
            ))
        })?
    } else {
        let cwd = std::env::current_dir().unwrap_or_default();
        let detected = detect_frameworks(&cwd);
        detected.into_iter().next().ok_or_else(|| {
            AgentError::ToolExecution(
                "No test framework detected. Specify framework explicitly with the 'framework' parameter.".to_string(),
            )
        })?
    };

    let (cmd, base_args) = framework.test_command();
    let mut all_args: Vec<String> = base_args.iter().map(|s| s.to_string()).collect();
    framework.apply_filters(&mut all_args, file, test_name);

    tracing::info!(
        framework = framework.display_name(),
        command = %cmd,
        args = ?all_args,
        "Running tests"
    );

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        tokio::process::Command::new(cmd).args(&all_args).output(),
    )
    .await
    .map_err(|_| {
        AgentError::ToolExecution(format!("Test execution timed out after {}s", timeout_secs))
    })?
    .map_err(|e| AgentError::ToolExecution(format!("Failed to execute {}: {}", cmd, e)))?;

    let stdout = strip_ansi(&String::from_utf8_lossy(&output.stdout));
    let stderr = strip_ansi(&String::from_utf8_lossy(&output.stderr));
    let combined = if stderr.is_empty() {
        stdout.clone()
    } else {
        format!("{}\n{}", stdout, stderr)
    };

    // Parse the output
    let summary = match framework {
        TestFramework::Cargo => super::parsers::cargo::parse_cargo_test_output(&combined),
        TestFramework::Jest => super::parsers::jest::parse_jest_output(&stdout),
        TestFramework::Go => super::parsers::go::parse_go_test_output(&stdout),
        TestFramework::Pytest => super::parsers::pytest::parse_pytest_output(&combined),
        _ => {
            // Fallback: return raw output
            return Ok(ToolExecutionResult {
                content: format!(
                    "Test output ({}):\n\n{}",
                    framework.display_name(),
                    truncate_output(&combined, 32_768)
                ),
                is_error: !output.status.success(),
            });
        }
    };

    Ok(ToolExecutionResult {
        content: summary.format_output(),
        is_error: summary.failed() > 0 || summary.errors() > 0,
    })
}

#[cfg(test)]
mod tests {
    use crate::local_tools::tools::output_filter::truncate_output;

    #[test]
    fn test_truncate_output_short() {
        let input = "hello";
        assert_eq!(truncate_output(input, 100), "hello");
    }

    #[test]
    fn test_truncate_output_long() {
        let input = "hello world";
        let result = truncate_output(input, 5);
        // The shared truncate_output keeps head/tail with a marker
        assert!(result.contains("truncated") || result.len() <= input.len());
    }

    #[test]
    fn test_truncate_output_utf8_boundary() {
        let input = "héllo";
        let result = truncate_output(input, 2);
        // Should be valid UTF-8
        let _ = result.to_string();
    }
}
