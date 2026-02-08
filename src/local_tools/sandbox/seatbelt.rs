use std::path::{Path, PathBuf};

use async_trait::async_trait;

use super::{BashSandbox, CommandOutput};
use crate::error::AgentError;

pub struct SeatbeltSandbox {
    root: PathBuf,
    profile_content: String,
}

impl SeatbeltSandbox {
    pub fn new(root: PathBuf) -> Result<Self, AgentError> {
        let root = root.canonicalize().map_err(|e| {
            AgentError::LocalTool(format!("Failed to canonicalize sandbox root: {e}"))
        })?;
        let profile_content = generate_profile(&root);
        Ok(Self {
            root,
            profile_content,
        })
    }
}

/// Generate a Seatbelt profile string.
///
/// Uses regex-based file-write filtering for compatibility with macOS 15+,
/// where subpath-based filters in sandbox-exec cause SIGABRT.
fn generate_profile(root: &Path) -> String {
    let root_str = regex_escape(&root.to_string_lossy());
    format!(
        r#"(version 1)
(deny default)
(allow process-exec)
(allow process-fork)
(allow file-read*)
(allow file-write* (regex #"^{root}/.*") (regex #"^/dev/null$"))
(allow sysctl-read)
(allow mach-lookup)
(allow network-outbound)
(allow signal)"#,
        root = root_str
    )
}

/// Escape special regex characters in a path string for Seatbelt profiles.
fn regex_escape(s: &str) -> String {
    let mut escaped = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '.' | '(' | ')' | '[' | ']' | '{' | '}' | '\\' | '+' | '*' | '?' | '|' | '^' | '$' => {
                escaped.push('\\');
                escaped.push(ch);
            }
            _ => escaped.push(ch),
        }
    }
    escaped
}

#[async_trait]
impl BashSandbox for SeatbeltSandbox {
    async fn execute(&self, command: &str, timeout_secs: u64) -> Result<CommandOutput, AgentError> {
        use std::time::Duration;

        let child = tokio::process::Command::new("sandbox-exec")
            .args(["-p", &self.profile_content, "sh", "-c", command])
            .current_dir(&self.root)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                AgentError::LocalTool(format!("Failed to spawn sandboxed command: {e}"))
            })?;

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

#[cfg(test)]
#[cfg(target_os = "macos")]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_generate_profile_contains_root() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let profile = generate_profile(&root);
        assert!(profile.contains(&regex_escape(&root.to_string_lossy())));
    }

    #[test]
    fn test_generate_profile_contains_required_rules() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let profile = generate_profile(&root);
        assert!(profile.contains("(deny default)"));
        assert!(profile.contains("(allow process-exec)"));
        assert!(profile.contains("(allow process-fork)"));
        assert!(profile.contains("(allow file-read*)"));
        assert!(profile.contains("(allow file-write*"));
        assert!(profile.contains("(allow sysctl-read)"));
        assert!(profile.contains("(allow mach-lookup)"));
        assert!(profile.contains("(allow signal)"));
    }

    #[test]
    fn test_regex_escape() {
        assert_eq!(regex_escape("/simple/path"), "/simple/path");
        assert_eq!(regex_escape("/path.with.dots"), "/path\\.with\\.dots");
        assert_eq!(regex_escape("test(1)"), "test\\(1\\)");
    }

    #[tokio::test]
    async fn test_seatbelt_execute_echo() {
        let tmp = TempDir::new().unwrap();
        let sandbox = SeatbeltSandbox::new(tmp.path().to_path_buf()).unwrap();
        let output = sandbox.execute("echo hello", 10).await.unwrap();
        assert_eq!(output.stdout.trim(), "hello");
        assert_eq!(output.exit_code, 0);
    }

    #[tokio::test]
    async fn test_seatbelt_timeout() {
        let tmp = TempDir::new().unwrap();
        let sandbox = SeatbeltSandbox::new(tmp.path().to_path_buf()).unwrap();
        let result = sandbox.execute("sleep 30", 1).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("timed out"));
    }

    #[tokio::test]
    async fn test_seatbelt_exit_code() {
        let tmp = TempDir::new().unwrap();
        let sandbox = SeatbeltSandbox::new(tmp.path().to_path_buf()).unwrap();
        let output = sandbox.execute("exit 42", 10).await.unwrap();
        assert_eq!(output.exit_code, 42);
    }
}
