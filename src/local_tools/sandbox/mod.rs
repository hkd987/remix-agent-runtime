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

/// Quote a single argument for safe inclusion in a `sh -c` command line.
///
/// Wraps the value in single quotes and escapes any embedded single quote using the
/// standard `'\''` idiom, so no metacharacter in the input can influence parsing.
pub fn shell_quote(arg: &str) -> String {
    format!("'{}'", arg.replace('\'', r"'\''"))
}

/// Trait for sandboxed command execution.
#[async_trait]
pub trait BashSandbox: Send + Sync {
    async fn execute(&self, command: &str, timeout_secs: u64) -> Result<CommandOutput, AgentError>;
    fn root(&self) -> &Path;

    /// Run a program with explicit arguments under the same sandbox as [`Self::execute`].
    ///
    /// Every subprocess the agent starts must go through a sandbox. Tools that already
    /// have a program and an argument vector — the test harness, skill scripts — should
    /// use this rather than spawning `tokio::process::Command` directly, which would
    /// escape both the OS-level restrictions and the sandbox root.
    ///
    /// The default implementation quotes each argument and delegates to
    /// [`Self::execute`], so every sandbox implementation gets it for free.
    async fn execute_argv(
        &self,
        program: &str,
        args: &[String],
        timeout_secs: u64,
    ) -> Result<CommandOutput, AgentError> {
        let mut command = shell_quote(program);
        for arg in args {
            command.push(' ');
            command.push_str(&shell_quote(arg));
        }
        self.execute(&command, timeout_secs).await
    }
}

/// Spawn `cmd` under a timeout and collect its output.
///
/// Shared by all three sandbox implementations, which each carried their own copy of
/// this spawn/timeout/match block. That duplication had already caused a divergence:
/// the process-group handling below existed only in the fallback sandbox, so a
/// timed-out command under Landlock or Seatbelt left its grandchildren running.
pub(crate) async fn run_with_timeout(
    mut cmd: tokio::process::Command,
    root: &Path,
    timeout_secs: u64,
) -> Result<CommandOutput, AgentError> {
    cmd.current_dir(root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    // Own process group, so a timeout can kill the whole tree. `kill_on_drop` signals
    // only the direct child, so a shell that has forked leaves its children running.
    #[cfg(unix)]
    cmd.process_group(0);

    let child = cmd
        .spawn()
        .map_err(|e| AgentError::LocalTool(format!("Failed to spawn sandboxed command: {e}")))?;

    #[cfg(unix)]
    let child_pid = child.id();

    let timeout = std::time::Duration::from_secs(timeout_secs);
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => Ok(CommandOutput {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code().unwrap_or(-1),
        }),
        Ok(Err(e)) => Err(AgentError::LocalTool(format!(
            "Command execution failed: {e}"
        ))),
        Err(_) => {
            #[cfg(unix)]
            kill_process_group(child_pid);
            Err(AgentError::LocalTool(format!(
                "Command timed out after {timeout_secs}s"
            )))
        }
    }
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

/// Build a shell invocation for the host platform.
///
/// `sh` does not exist on a stock Windows install, so a hardcoded `sh -c` makes the
/// bash tool fail to spawn there — on a target the project ships release binaries for.
#[cfg(windows)]
fn shell_command(command: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("cmd");
    cmd.args(["/C", command]);
    cmd
}

#[cfg(not(windows))]
fn shell_command(command: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("sh");
    cmd.args(["-c", command]);
    cmd
}

#[async_trait]
impl BashSandbox for FallbackSandbox {
    async fn execute(&self, command: &str, timeout_secs: u64) -> Result<CommandOutput, AgentError> {
        run_with_timeout(shell_command(command), &self.root, timeout_secs).await
    }

    fn root(&self) -> &Path {
        &self.root
    }
}

/// Send SIGKILL to the process group led by `pid`, reaping any grandchildren the
/// timed-out command left behind.
#[cfg(unix)]
pub(crate) fn kill_process_group(pid: Option<u32>) {
    let Some(pid) = pid else { return };
    // Safety: `killpg` takes a process group id and a signal and has no memory
    // effects. A failure (group already gone) is reported via the return value and
    // is not actionable here.
    unsafe {
        libc_killpg(pid as i32, 9);
    }
}

#[cfg(unix)]
unsafe fn libc_killpg(pgid: i32, sig: i32) -> i32 {
    extern "C" {
        fn killpg(pgrp: i32, sig: i32) -> i32;
    }
    killpg(pgid, sig)
}

/// Create the appropriate sandbox for the current platform.
///
/// Warns when no OS-level enforcement is available, so an operator is never left
/// believing a run is contained when it is not.
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
    tracing::warn!(
        root = %root.display(),
        "No OS-level sandbox is available on this platform. Shell commands run with the \
         agent's full privileges, restricted only by working directory and timeout. Do not \
         run untrusted tasks in this configuration."
    );
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
