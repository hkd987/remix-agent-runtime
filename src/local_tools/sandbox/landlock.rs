#[cfg(target_os = "linux")]
mod linux_impl {
    use std::path::{Path, PathBuf};

    use async_trait::async_trait;

    use super::super::{BashSandbox, CommandOutput};
    use crate::error::AgentError;

    pub struct LandlockSandbox {
        root: PathBuf,
    }

    impl LandlockSandbox {
        pub fn new(root: PathBuf) -> Result<Self, AgentError> {
            let root = root.canonicalize().map_err(|e| {
                AgentError::LocalTool(format!("Failed to canonicalize sandbox root: {e}"))
            })?;
            Ok(Self { root })
        }
    }

    #[async_trait]
    impl BashSandbox for LandlockSandbox {
        async fn execute(
            &self,
            command: &str,
            timeout_secs: u64,
        ) -> Result<CommandOutput, AgentError> {
            use std::time::Duration;

            let root_clone = self.root.clone();

            let mut cmd = tokio::process::Command::new("sh");
            cmd.args(["-c", command])
                .current_dir(&self.root)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true);

            // Apply Landlock restrictions via pre_exec
            // SAFETY: apply_landlock_rules only calls Landlock syscalls (async-signal-safe)
            unsafe {
                use std::os::unix::process::CommandExt;
                let root_for_preexec = root_clone.clone();
                cmd.pre_exec(move || apply_landlock_rules(&root_for_preexec));
            }

            let child = cmd.spawn().map_err(|e| {
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

    fn apply_landlock_rules(root: &Path) -> std::io::Result<()> {
        use landlock::{
            Access, AccessFs, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr, ABI,
        };

        let abi = ABI::V3;

        let read_access = AccessFs::from_read(abi);
        let read_write_access = AccessFs::from_all(abi);

        let ruleset = Ruleset::default()
            .handle_access(read_write_access)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?
            .create()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;

        // Read access to system dirs
        // add_rule() takes self by value and returns the modified ruleset,
        // so we chain through a mutable binding.
        let mut ruleset = ruleset;
        for sys_dir in &[
            "/usr", "/bin", "/lib", "/lib64", "/etc", "/dev", "/sbin", "/proc",
        ] {
            if let Ok(fd) = PathFd::new(sys_dir) {
                ruleset = ruleset
                    .add_rule(PathBeneath::new(fd, read_access))
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
            }
        }

        // Read + write access to sandbox root
        if let Ok(fd) = PathFd::new(root) {
            ruleset = ruleset
                .add_rule(PathBeneath::new(fd, read_write_access))
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        }

        ruleset
            .restrict_self()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;

        Ok(())
    }
}

#[cfg(target_os = "linux")]
pub use linux_impl::LandlockSandbox;

// Non-Linux stub
#[cfg(not(target_os = "linux"))]
pub struct LandlockSandbox;

#[cfg(not(target_os = "linux"))]
impl LandlockSandbox {
    pub fn new(_root: std::path::PathBuf) -> Result<Self, crate::error::AgentError> {
        Err(crate::error::AgentError::LocalTool(
            "Landlock is only available on Linux".to_string(),
        ))
    }
}
