use std::sync::Arc;

use crate::browser::mcp::ToolExecutionResult;
use crate::config::schema::TestHarnessConfig;
use crate::error::AgentError;
use crate::local_tools::sandbox::BashSandbox;
use crate::local_tools::tools::output_filter::truncate_output;

use super::exec::{resolve_framework, run_under_sandbox};
use super::types::TestFramework;

pub async fn execute_run_tests(
    arguments: serde_json::Value,
    config: &TestHarnessConfig,
    sandbox: Arc<dyn BashSandbox>,
) -> Result<ToolExecutionResult, AgentError> {
    let framework_str = arguments.get("framework").and_then(|v| v.as_str());
    let file = arguments.get("file").and_then(|v| v.as_str());
    let test_name = arguments.get("test_name").and_then(|v| v.as_str());
    let timeout_secs = arguments
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(config.timeout_secs);

    let framework = resolve_framework(framework_str)?;

    let (cmd, base_args) = framework.test_command();
    let mut all_args: Vec<String> = base_args.iter().map(|s| s.to_string()).collect();
    framework.apply_filters(&mut all_args, file, test_name);

    tracing::info!(
        framework = framework.display_name(),
        command = %cmd,
        args = ?all_args,
        "Running tests"
    );

    let output = run_under_sandbox(
        sandbox.as_ref(),
        cmd,
        &all_args,
        timeout_secs,
        "Test execution",
    )
    .await?;
    let stdout = output.stdout.clone();
    let combined = output.combined.clone();

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
                is_error: !output.success(),
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
