use std::sync::Arc;

use crate::browser::mcp::ToolExecutionResult;
use crate::config::schema::TestHarnessConfig;
use crate::error::AgentError;
use crate::local_tools::sandbox::BashSandbox;
use crate::local_tools::tools::output_filter::truncate_output;

use super::exec::{resolve_framework, run_under_sandbox};

pub async fn execute_list_tests(
    arguments: serde_json::Value,
    config: &TestHarnessConfig,
    sandbox: Arc<dyn BashSandbox>,
) -> Result<ToolExecutionResult, AgentError> {
    let framework_str = arguments.get("framework").and_then(|v| v.as_str());
    let file = arguments.get("file").and_then(|v| v.as_str());

    let framework = resolve_framework(framework_str)?;

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

    let output = run_under_sandbox(
        sandbox.as_ref(),
        cmd,
        &args,
        config.timeout_secs,
        "Test list",
    )
    .await?;
    let succeeded = output.success();
    let combined = output.combined;

    let content = truncate_output(&combined, 32_768);

    Ok(ToolExecutionResult {
        content: format!(
            "Tests ({}){}\n\n{}",
            framework.display_name(),
            file.map(|f| format!(" in {}", f)).unwrap_or_default(),
            content
        ),
        is_error: !succeeded,
    })
}

#[cfg(test)]
mod tests {
    use crate::dev_tools::test_harness::types::TestFramework;

    #[test]
    fn test_framework_list_commands_exist() {
        // Verify frameworks we support have list commands
        assert!(TestFramework::Cargo.list_command().is_some());
        assert!(TestFramework::Pytest.list_command().is_some());
        assert!(TestFramework::Jest.list_command().is_some());
        assert!(TestFramework::Go.list_command().is_some());
    }
}
