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
            .map_err(|e| std::io::Error::other(e.to_string()))?
            .create()
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        // Read access to system dirs.
        //
        // `/proc` stays on this list deliberately. Build tools, language runtimes and
        // allocators read `/proc/self/*` routinely, and Landlock rules are path-based,
        // so there is no way to grant `/proc/self` without granting sibling PIDs. The
        // residual exposure (`/proc/<pid>/environ` of other same-user processes) is not
        // closable here — it belongs to process isolation, not the filesystem sandbox.
        //
        // add_rule() takes self by value and returns the modified ruleset,
        // so we chain through a mutable binding.
        let mut ruleset = ruleset;
        for sys_dir in &[
            "/usr", "/bin", "/lib", "/lib64", "/etc", "/dev", "/sbin", "/proc",
        ] {
            match PathFd::new(sys_dir) {
                Ok(fd) => {
                    ruleset = ruleset
                        .add_rule(PathBeneath::new(fd, read_access))
                        .map_err(|e| std::io::Error::other(e.to_string()))?;
                }
                Err(e) => {
                    // Not fatal — the directory may simply not exist on this system —
                    // but silently dropping it changes what the sandbox permits, so say so.
                    tracing::debug!(dir = %sys_dir, error = %e, "Skipping sandbox read rule");
                }
            }
        }

        // Read + write access to sandbox root.
        //
        // Unlike the system directories, a failure here is fatal: the ruleset would be
        // applied with no writable path at all, and every write would fail with a
        // confusing permission error instead of a clear startup failure.
        let root_fd = PathFd::new(root).map_err(|e| {
            std::io::Error::other(format!(
                "Cannot open sandbox root {} for Landlock: {e}",
                root.display()
            ))
        })?;
        ruleset = ruleset
            .add_rule(PathBeneath::new(root_fd, read_write_access))
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        let status = ruleset
            .restrict_self()
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        // On a kernel without Landlock support `restrict_self` succeeds but enforces
        // nothing. Reporting that is the difference between a contained run and one the
        // operator only believes is contained.
        if status.ruleset == landlock::RulesetStatus::NotEnforced {
            tracing::warn!(
                "Landlock is not enforced by this kernel. Shell commands run with the \
                 agent's full filesystem access. Do not run untrusted tasks here."
            );
        } else if status.ruleset == landlock::RulesetStatus::PartiallyEnforced {
            tracing::warn!(
                "Landlock is only partially enforced by this kernel; some filesystem \
                 restrictions are not active."
            );
        }

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
