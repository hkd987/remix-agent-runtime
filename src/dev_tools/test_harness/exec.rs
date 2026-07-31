//! Shared subprocess plumbing for the test-harness tools.
//!
//! `run_tests` and `list_tests` previously each carried their own copy of the
//! framework-resolution and process-spawning logic, which had already drifted (the two
//! "Supported:" error messages listed different frameworks). Both now go through this
//! module, and both run under the sandbox rather than spawning directly.

use crate::error::AgentError;
use crate::local_tools::sandbox::BashSandbox;
use crate::local_tools::tools::output_filter::strip_ansi;

use super::detect::detect_frameworks;
use super::types::TestFramework;

/// Frameworks accepted by the `framework` parameter, kept in one place so the two
/// tools cannot disagree about what is supported.
pub const SUPPORTED_FRAMEWORKS: &str = "cargo, pytest, jest, vitest, go, mocha";

/// Output of a test-harness subprocess, with ANSI escapes already stripped.
pub struct HarnessOutput {
    pub stdout: String,
    pub stderr: String,
    /// stdout, plus stderr appended when non-empty. Parsers consume this.
    pub combined: String,
    pub exit_code: i32,
}

impl HarnessOutput {
    /// Whether the underlying command exited successfully.
    pub fn success(&self) -> bool {
        self.exit_code == 0
    }
}

/// Resolve the framework from an explicit id, falling back to detection.
pub fn resolve_framework(framework_str: Option<&str>) -> Result<TestFramework, AgentError> {
    match framework_str {
        Some(id) => TestFramework::from_str_id(id).ok_or_else(|| {
            AgentError::ToolExecution(format!(
                "Unknown test framework: '{id}'. Supported: {SUPPORTED_FRAMEWORKS}"
            ))
        }),
        None => {
            let cwd = std::env::current_dir().unwrap_or_default();
            detect_frameworks(&cwd).into_iter().next().ok_or_else(|| {
                AgentError::ToolExecution(
                    "No test framework detected. Specify framework explicitly with the \
                     'framework' parameter."
                        .to_string(),
                )
            })
        }
    }
}

/// Run a test-framework command under the sandbox and return its filtered output.
pub async fn run_under_sandbox(
    sandbox: &dyn BashSandbox,
    cmd: &str,
    args: &[String],
    timeout_secs: u64,
    what: &str,
) -> Result<HarnessOutput, AgentError> {
    let output = sandbox
        .execute_argv(cmd, args, timeout_secs)
        .await
        .map_err(|e| AgentError::ToolExecution(format!("{what} failed: {e}")))?;

    let stdout = strip_ansi(&output.stdout);
    let stderr = strip_ansi(&output.stderr);
    let combined = if stderr.is_empty() {
        stdout.clone()
    } else {
        format!("{stdout}\n{stderr}")
    };

    Ok(HarnessOutput {
        stdout,
        stderr,
        combined,
        exit_code: output.exit_code,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_framework_accepts_known_id() {
        let fw = resolve_framework(Some("cargo")).unwrap();
        assert_eq!(fw, TestFramework::Cargo);
    }

    #[test]
    fn resolve_framework_rejects_unknown_id() {
        let err = resolve_framework(Some("nosuchframework")).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("nosuchframework"), "got: {msg}");
        // Both tools now quote the same list.
        assert!(msg.contains(SUPPORTED_FRAMEWORKS), "got: {msg}");
    }

    #[test]
    fn supported_frameworks_list_mentions_mocha() {
        // `list.rs` used to omit mocha from its error message while `run.rs` included
        // it; the shared constant is what keeps them aligned.
        assert!(SUPPORTED_FRAMEWORKS.contains("mocha"));
    }
}
