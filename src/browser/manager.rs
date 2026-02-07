use crate::config::schema::BrowserConfig;
use crate::error::AgentError;
use tokio::process::{Child, Command};

pub struct BrowserManager {
    child: Option<Child>,
}

impl BrowserManager {
    /// Build a `tokio::process::Command` for launching remix-browser
    /// with the settings from `BrowserConfig`. The command has its
    /// stdio configured for MCP communication (piped stdin/stdout/stderr)
    /// and will kill the child on drop.
    pub fn build_command(config: &BrowserConfig) -> Command {
        let browser_path = config.browser_path.as_deref().unwrap_or("remix-browser");
        let mut cmd = Command::new(browser_path);

        if config.headless {
            cmd.arg("--headless");
        } else {
            cmd.arg("--headed");
        }

        cmd.arg("--viewport-width")
            .arg(config.viewport_width.to_string());
        cmd.arg("--viewport-height")
            .arg(config.viewport_height.to_string());

        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        cmd.kill_on_drop(true);

        cmd
    }

    /// Create a `BrowserManager` from an already-spawned child process.
    pub fn from_child(child: Child) -> Self {
        Self { child: Some(child) }
    }

    /// Shutdown the browser process by killing it.
    pub async fn shutdown(&mut self) -> Result<(), AgentError> {
        if let Some(mut child) = self.child.take() {
            child
                .kill()
                .await
                .map_err(|e| AgentError::Browser(format!("Failed to kill browser: {e}")))?;
        }
        Ok(())
    }

    /// Check if the browser process is still running.
    pub fn is_running(&mut self) -> bool {
        self.child
            .as_mut()
            .map(|c| c.try_wait().ok().flatten().is_none())
            .unwrap_or(false)
    }
}

impl Drop for BrowserManager {
    fn drop(&mut self) {
        if let Some(ref mut child) = self.child {
            let _ = child.start_kill();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_command_headless_default() {
        let config = BrowserConfig::default();
        assert!(config.headless);

        let cmd = BrowserManager::build_command(&config);
        let program = cmd.as_std().get_program().to_str().unwrap();
        assert_eq!(program, "remix-browser");

        let args: Vec<&str> = cmd.as_std().get_args().filter_map(|a| a.to_str()).collect();
        assert!(args.contains(&"--headless"));
        assert!(!args.contains(&"--headed"));
        assert!(args.contains(&"--viewport-width"));
        assert!(args.contains(&"1280"));
        assert!(args.contains(&"--viewport-height"));
        assert!(args.contains(&"720"));
    }

    #[test]
    fn test_build_command_headed() {
        let config = BrowserConfig {
            headless: false,
            ..Default::default()
        };

        let cmd = BrowserManager::build_command(&config);
        let args: Vec<&str> = cmd.as_std().get_args().filter_map(|a| a.to_str()).collect();
        assert!(args.contains(&"--headed"));
        assert!(!args.contains(&"--headless"));
    }

    #[test]
    fn test_build_command_custom_viewport() {
        let config = BrowserConfig {
            viewport_width: 1920,
            viewport_height: 1080,
            ..Default::default()
        };

        let cmd = BrowserManager::build_command(&config);
        let args: Vec<&str> = cmd.as_std().get_args().filter_map(|a| a.to_str()).collect();
        assert!(args.contains(&"1920"));
        assert!(args.contains(&"1080"));
    }

    #[test]
    fn test_build_command_custom_browser_path() {
        let config = BrowserConfig {
            browser_path: Some("/usr/local/bin/remix-browser".to_string()),
            ..Default::default()
        };

        let cmd = BrowserManager::build_command(&config);
        let program = cmd.as_std().get_program().to_str().unwrap();
        assert_eq!(program, "/usr/local/bin/remix-browser");
    }

    #[test]
    fn test_build_command_arg_order() {
        let config = BrowserConfig::default();
        let cmd = BrowserManager::build_command(&config);
        let args: Vec<&str> = cmd.as_std().get_args().filter_map(|a| a.to_str()).collect();

        // Verify viewport-width comes with its value
        let width_idx = args.iter().position(|a| *a == "--viewport-width").unwrap();
        assert_eq!(args[width_idx + 1], "1280");

        // Verify viewport-height comes with its value
        let height_idx = args.iter().position(|a| *a == "--viewport-height").unwrap();
        assert_eq!(args[height_idx + 1], "720");
    }

    #[test]
    fn test_is_running_no_child() {
        let mut manager = BrowserManager { child: None };
        assert!(!manager.is_running());
    }

    #[tokio::test]
    async fn test_shutdown_no_child() {
        let mut manager = BrowserManager { child: None };
        let result = manager.shutdown().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_from_child_and_shutdown() {
        // Spawn a real short-lived process for testing
        let child = Command::new("sleep")
            .arg("60")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("failed to spawn sleep");

        let mut manager = BrowserManager::from_child(child);
        assert!(manager.is_running());

        manager.shutdown().await.expect("shutdown should succeed");
        assert!(!manager.is_running());
    }

    #[tokio::test]
    async fn test_is_running_after_process_exits() {
        let child = Command::new("true")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("failed to spawn true");

        let mut manager = BrowserManager::from_child(child);
        // Give the process time to exit
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(!manager.is_running());
    }
}
