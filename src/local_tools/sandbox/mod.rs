pub mod path_validator;

#[cfg(target_os = "macos")]
pub mod seatbelt;

pub mod landlock;

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::error::AgentError;

pub use path_validator::PathValidator;

/// Output from a sandboxed command execution.
#[derive(Debug, Clone)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

/// Trait for sandboxed bash command execution.
#[async_trait]
pub trait BashSandbox: Send + Sync {
    async fn execute(&self, command: &str, timeout_secs: u64) -> Result<CommandOutput, AgentError>;
    fn root(&self) -> &Path;
}

/// Fallback sandbox with no OS-level enforcement (timeout + cwd only).
pub struct FallbackSandbox {
    root: PathBuf,
}

impl FallbackSandbox {
    pub fn new(root: PathBuf) -> Result<Self, AgentError> {
        let root = root.canonicalize().map_err(|e| {
            AgentError::LocalTool(format!("Failed to canonicalize sandbox root: {e}"))
        })?;
        Ok(Self { root })
    }
}

#[async_trait]
impl BashSandbox for FallbackSandbox {
    async fn execute(&self, command: &str, timeout_secs: u64) -> Result<CommandOutput, AgentError> {
        use std::time::Duration;

        let child = tokio::process::Command::new("sh")
            .args(["-c", command])
            .current_dir(&self.root)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| AgentError::LocalTool(format!("Failed to spawn command: {e}")))?;

        let timeout = Duration::from_secs(timeout_secs);
        let result = tokio::time::timeout(timeout, child.wait_with_output()).await;

        match result {
            Ok(Ok(output)) => Ok(CommandOutput {
                stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
                exit_code: output.status.code().unwrap_or(-1),
            }),
            Ok(Err(e)) => Err(AgentError::LocalTool(format!(
                "Command execution failed: {e}"
            ))),
            Err(_) => Err(AgentError::LocalTool(format!(
                "Command timed out after {timeout_secs}s"
            ))),
        }
    }

    fn root(&self) -> &Path {
        &self.root
    }
}

/// Create the appropriate sandbox for the current platform.
pub fn create_sandbox(root: PathBuf) -> Result<Box<dyn BashSandbox>, AgentError> {
    create_platform_sandbox(root)
}

#[cfg(target_os = "macos")]
fn create_platform_sandbox(root: PathBuf) -> Result<Box<dyn BashSandbox>, AgentError> {
    Ok(Box::new(seatbelt::SeatbeltSandbox::new(root)?))
}

#[cfg(target_os = "linux")]
fn create_platform_sandbox(root: PathBuf) -> Result<Box<dyn BashSandbox>, AgentError> {
    Ok(Box::new(landlock::LandlockSandbox::new(root)?))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn create_platform_sandbox(root: PathBuf) -> Result<Box<dyn BashSandbox>, AgentError> {
    Ok(Box::new(FallbackSandbox::new(root)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_command_output_construction() {
        let output = CommandOutput {
            stdout: "hello\n".to_string(),
            stderr: String::new(),
            exit_code: 0,
        };
        assert_eq!(output.stdout, "hello\n");
        assert!(output.stderr.is_empty());
        assert_eq!(output.exit_code, 0);
    }

    #[tokio::test]
    async fn test_fallback_sandbox_execute() {
        let tmp = TempDir::new().unwrap();
        let sandbox = FallbackSandbox::new(tmp.path().to_path_buf()).unwrap();
        let output = sandbox.execute("echo fallback", 10).await.unwrap();
        assert_eq!(output.stdout.trim(), "fallback");
        assert_eq!(output.exit_code, 0);
    }

    #[test]
    fn test_create_sandbox_returns_ok() {
        let tmp = TempDir::new().unwrap();
        let result = create_sandbox(tmp.path().to_path_buf());
        assert!(result.is_ok());
    }
}
