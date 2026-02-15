use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "remix-agent",
    version,
    about = "LLM-driven browser automation agent runtime"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Run an agent task
    Run(RunArgs),
}

#[derive(clap::Args, Debug)]
pub struct RunArgs {
    /// Task to execute (natural language)
    pub task: Option<String>,

    /// Path to YAML configuration file
    #[arg(short, long)]
    pub config: Option<PathBuf>,

    /// LLM provider base URL
    #[arg(long, env = "REMIX_LLM_BASE_URL")]
    pub base_url: Option<String>,

    /// LLM API key
    #[arg(long, env = "REMIX_LLM_API_KEY")]
    pub api_key: Option<String>,

    /// LLM model identifier
    #[arg(long, env = "REMIX_LLM_MODEL")]
    pub model: Option<String>,

    /// Maximum tokens per LLM response
    #[arg(long)]
    pub max_tokens: Option<u32>,

    /// Maximum duration in seconds
    #[arg(long)]
    pub timeout: Option<u64>,

    /// Maximum agent loop iterations
    #[arg(long)]
    pub max_iterations: Option<u32>,

    /// Run browser in headed mode (visible)
    #[arg(long)]
    pub headed: bool,

    /// Enable verbose logging to stderr
    #[arg(short, long)]
    pub verbose: bool,

    /// Write results to file instead of stdout
    #[arg(short, long)]
    pub output: Option<PathBuf>,

    /// Path to remix-browser binary
    #[arg(long, env = "REMIX_BROWSER_PATH")]
    pub browser_path: Option<String>,

    /// Additional directory to scan for skills
    #[arg(long, env = "REMIX_SKILLS_DIR")]
    pub skills_dir: Option<PathBuf>,

    /// Disable skill discovery
    #[arg(long)]
    pub no_skills: bool,

    /// Directory to search for AGENTS.md files
    #[arg(long, env = "REMIX_AGENTS_MD_DIR")]
    pub agents_md_dir: Option<PathBuf>,

    /// Disable AGENTS.md discovery
    #[arg(long)]
    pub no_agents_md: bool,

    /// Disable local filesystem tools
    #[arg(long)]
    pub no_local_tools: bool,

    /// Sandbox root directory for local tools
    #[arg(long, env = "REMIX_SANDBOX_DIR")]
    pub sandbox_dir: Option<PathBuf>,

    /// Disable all plugin discovery
    #[arg(long)]
    pub no_plugins: bool,

    /// Additional directory to scan for plugins
    #[arg(long, env = "REMIX_PLUGINS_DIR")]
    pub plugins_dir: Option<PathBuf>,

    /// Disable Claude Code plugin cache discovery
    #[arg(long)]
    pub no_claude_plugins: bool,

    /// Resume an existing session by ID
    #[arg(long)]
    pub session_id: Option<String>,

    /// Fork from an existing session
    #[arg(long)]
    pub fork_session: Option<String>,

    /// Override session storage directory
    #[arg(long, env = "REMIX_SESSION_DIR")]
    pub session_dir: Option<PathBuf>,

    /// Permission mode (default, accept_edits, bypass_permissions, plan)
    #[arg(long)]
    pub permission_mode: Option<String>,

    /// Allow specific tool patterns (repeatable)
    #[arg(long)]
    pub allow_tool: Vec<String>,

    /// Deny specific tool patterns (repeatable)
    #[arg(long)]
    pub deny_tool: Vec<String>,

    /// Disable multi-agent coordination
    #[arg(long)]
    pub no_coordination: bool,

    /// Maximum concurrent worker agents
    #[arg(long)]
    pub max_workers: Option<u32>,

    /// Override coordination storage directory
    #[arg(long, env = "REMIX_COORDINATION_DIR")]
    pub coordination_dir: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn test_parse_run_with_task() {
        let cli = Cli::parse_from(["remix-agent", "run", "navigate to google.com"]);
        match cli.command {
            Commands::Run(args) => {
                assert_eq!(args.task, Some("navigate to google.com".to_string()));
                assert!(args.config.is_none());
                assert!(args.base_url.is_none());
                assert!(args.api_key.is_none());
                assert!(args.model.is_none());
                assert!(args.max_tokens.is_none());
                assert!(args.timeout.is_none());
                assert!(args.max_iterations.is_none());
                assert!(!args.headed);
                assert!(!args.verbose);
                assert!(args.output.is_none());
                assert!(args.browser_path.is_none());
            }
        }
    }

    #[test]
    fn test_parse_run_without_task() {
        let cli = Cli::parse_from(["remix-agent", "run"]);
        match cli.command {
            Commands::Run(args) => {
                assert!(args.task.is_none());
            }
        }
    }

    #[test]
    fn test_parse_run_with_config() {
        let cli = Cli::parse_from(["remix-agent", "run", "--config", "task.yaml"]);
        match cli.command {
            Commands::Run(args) => {
                assert_eq!(args.config, Some(PathBuf::from("task.yaml")));
            }
        }
    }

    #[test]
    fn test_parse_run_with_all_flags() {
        let cli = Cli::parse_from([
            "remix-agent",
            "run",
            "--config",
            "task.yaml",
            "--base-url",
            "https://api.example.com",
            "--api-key",
            "sk-test",
            "--model",
            "gpt-4",
            "--max-tokens",
            "4096",
            "--timeout",
            "600",
            "--max-iterations",
            "100",
            "--headed",
            "--verbose",
            "--output",
            "result.json",
            "--browser-path",
            "/usr/local/bin/remix-browser",
            "--skills-dir",
            "/tmp/skills",
            "--no-skills",
            "--agents-md-dir",
            "/tmp/agents",
            "--no-agents-md",
            "--no-local-tools",
            "--sandbox-dir",
            "/tmp/sandbox",
            "--no-plugins",
            "--plugins-dir",
            "/tmp/plugins",
            "--no-claude-plugins",
            "--session-id",
            "abc-123",
            "--fork-session",
            "def-456",
            "--session-dir",
            "/tmp/sessions",
            "--permission-mode",
            "plan",
            "--allow-tool",
            "navigate",
            "--allow-tool",
            "click",
            "--deny-tool",
            "bash",
            "--no-coordination",
            "--max-workers",
            "10",
            "--coordination-dir",
            "/tmp/coordination",
            "do something",
        ]);
        match cli.command {
            Commands::Run(args) => {
                assert_eq!(args.task, Some("do something".to_string()));
                assert_eq!(args.config, Some(PathBuf::from("task.yaml")));
                assert_eq!(args.base_url, Some("https://api.example.com".to_string()));
                assert_eq!(args.api_key, Some("sk-test".to_string()));
                assert_eq!(args.model, Some("gpt-4".to_string()));
                assert_eq!(args.max_tokens, Some(4096));
                assert_eq!(args.timeout, Some(600));
                assert_eq!(args.max_iterations, Some(100));
                assert!(args.headed);
                assert!(args.verbose);
                assert_eq!(args.output, Some(PathBuf::from("result.json")));
                assert_eq!(
                    args.browser_path,
                    Some("/usr/local/bin/remix-browser".to_string())
                );
                assert_eq!(args.skills_dir, Some(PathBuf::from("/tmp/skills")));
                assert!(args.no_skills);
                assert_eq!(args.agents_md_dir, Some(PathBuf::from("/tmp/agents")));
                assert!(args.no_agents_md);
                assert!(args.no_local_tools);
                assert_eq!(args.sandbox_dir, Some(PathBuf::from("/tmp/sandbox")));
                assert!(args.no_plugins);
                assert_eq!(args.plugins_dir, Some(PathBuf::from("/tmp/plugins")));
                assert!(args.no_claude_plugins);
                assert_eq!(args.session_id, Some("abc-123".to_string()));
                assert_eq!(args.fork_session, Some("def-456".to_string()));
                assert_eq!(args.session_dir, Some(PathBuf::from("/tmp/sessions")));
                assert_eq!(args.permission_mode, Some("plan".to_string()));
                assert_eq!(args.allow_tool, vec!["navigate", "click"]);
                assert_eq!(args.deny_tool, vec!["bash"]);
                assert!(args.no_coordination);
                assert_eq!(args.max_workers, Some(10));
                assert_eq!(
                    args.coordination_dir,
                    Some(PathBuf::from("/tmp/coordination"))
                );
            }
        }
    }

    #[test]
    fn test_parse_run_short_flags() {
        let cli = Cli::parse_from([
            "remix-agent",
            "run",
            "-c",
            "task.yaml",
            "-v",
            "-o",
            "out.json",
        ]);
        match cli.command {
            Commands::Run(args) => {
                assert_eq!(args.config, Some(PathBuf::from("task.yaml")));
                assert!(args.verbose);
                assert_eq!(args.output, Some(PathBuf::from("out.json")));
            }
        }
    }

    #[test]
    fn test_parse_run_with_skills_dir() {
        let cli = Cli::parse_from(["remix-agent", "run", "--skills-dir", "/path/to/skills"]);
        match cli.command {
            Commands::Run(args) => {
                assert_eq!(args.skills_dir, Some(PathBuf::from("/path/to/skills")));
                assert!(!args.no_skills);
            }
        }
    }

    #[test]
    fn test_parse_run_with_no_skills() {
        let cli = Cli::parse_from(["remix-agent", "run", "--no-skills"]);
        match cli.command {
            Commands::Run(args) => {
                assert!(args.no_skills);
                assert!(args.skills_dir.is_none());
            }
        }
    }

    #[test]
    fn test_parse_run_with_no_plugins() {
        let cli = Cli::parse_from(["remix-agent", "run", "--no-plugins"]);
        match cli.command {
            Commands::Run(args) => {
                assert!(args.no_plugins);
                assert!(args.plugins_dir.is_none());
                assert!(!args.no_claude_plugins);
            }
        }
    }

    #[test]
    fn test_parse_run_with_plugins_dir() {
        let cli = Cli::parse_from(["remix-agent", "run", "--plugins-dir", "/path/to/plugins"]);
        match cli.command {
            Commands::Run(args) => {
                assert_eq!(args.plugins_dir, Some(PathBuf::from("/path/to/plugins")));
                assert!(!args.no_plugins);
            }
        }
    }

    #[test]
    fn test_parse_run_with_no_claude_plugins() {
        let cli = Cli::parse_from(["remix-agent", "run", "--no-claude-plugins"]);
        match cli.command {
            Commands::Run(args) => {
                assert!(args.no_claude_plugins);
                assert!(!args.no_plugins);
            }
        }
    }

    #[test]
    fn test_missing_subcommand_fails() {
        let result = Cli::try_parse_from(["remix-agent"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_subcommand_fails() {
        let result = Cli::try_parse_from(["remix-agent", "invalid"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_run_with_agents_md_dir() {
        let cli = Cli::parse_from(["remix-agent", "run", "--agents-md-dir", "/path/to/project"]);
        match cli.command {
            Commands::Run(args) => {
                assert_eq!(args.agents_md_dir, Some(PathBuf::from("/path/to/project")));
                assert!(!args.no_agents_md);
            }
        }
    }

    #[test]
    fn test_parse_run_with_no_agents_md() {
        let cli = Cli::parse_from(["remix-agent", "run", "--no-agents-md"]);
        match cli.command {
            Commands::Run(args) => {
                assert!(args.no_agents_md);
                assert!(args.agents_md_dir.is_none());
            }
        }
    }

    #[test]
    fn test_parse_run_with_no_local_tools() {
        let cli = Cli::parse_from(["remix-agent", "run", "--no-local-tools"]);
        match cli.command {
            Commands::Run(args) => {
                assert!(args.no_local_tools);
                assert!(args.sandbox_dir.is_none());
            }
        }
    }

    #[test]
    fn test_parse_run_with_sandbox_dir() {
        let cli = Cli::parse_from(["remix-agent", "run", "--sandbox-dir", "/tmp/sandbox"]);
        match cli.command {
            Commands::Run(args) => {
                assert_eq!(args.sandbox_dir, Some(PathBuf::from("/tmp/sandbox")));
                assert!(!args.no_local_tools);
            }
        }
    }

    #[test]
    fn test_parse_run_with_session_id() {
        let cli = Cli::parse_from(["remix-agent", "run", "--session-id", "abc-123"]);
        match cli.command {
            Commands::Run(args) => {
                assert_eq!(args.session_id, Some("abc-123".to_string()));
                assert!(args.fork_session.is_none());
            }
        }
    }

    #[test]
    fn test_parse_run_with_fork_session() {
        let cli = Cli::parse_from(["remix-agent", "run", "--fork-session", "def-456"]);
        match cli.command {
            Commands::Run(args) => {
                assert_eq!(args.fork_session, Some("def-456".to_string()));
                assert!(args.session_id.is_none());
            }
        }
    }

    #[test]
    fn test_parse_run_with_session_dir() {
        let cli = Cli::parse_from(["remix-agent", "run", "--session-dir", "/tmp/sessions"]);
        match cli.command {
            Commands::Run(args) => {
                assert_eq!(args.session_dir, Some(PathBuf::from("/tmp/sessions")));
            }
        }
    }

    #[test]
    fn test_parse_run_with_permission_mode() {
        let cli = Cli::parse_from(["remix-agent", "run", "--permission-mode", "plan"]);
        match cli.command {
            Commands::Run(args) => {
                assert_eq!(args.permission_mode, Some("plan".to_string()));
            }
        }
    }

    #[test]
    fn test_parse_run_with_no_coordination() {
        let cli = Cli::parse_from(["remix-agent", "run", "--no-coordination"]);
        match cli.command {
            Commands::Run(args) => {
                assert!(args.no_coordination);
                assert!(args.max_workers.is_none());
                assert!(args.coordination_dir.is_none());
            }
        }
    }

    #[test]
    fn test_parse_run_with_max_workers() {
        let cli = Cli::parse_from(["remix-agent", "run", "--max-workers", "8"]);
        match cli.command {
            Commands::Run(args) => {
                assert_eq!(args.max_workers, Some(8));
                assert!(!args.no_coordination);
            }
        }
    }

    #[test]
    fn test_parse_run_with_coordination_dir() {
        let cli = Cli::parse_from([
            "remix-agent",
            "run",
            "--coordination-dir",
            "/tmp/coordination",
        ]);
        match cli.command {
            Commands::Run(args) => {
                assert_eq!(
                    args.coordination_dir,
                    Some(PathBuf::from("/tmp/coordination"))
                );
            }
        }
    }

    #[test]
    fn test_parse_run_with_allow_deny_tools() {
        let cli = Cli::parse_from([
            "remix-agent",
            "run",
            "--allow-tool",
            "navigate",
            "--allow-tool",
            "click",
            "--deny-tool",
            "bash",
        ]);
        match cli.command {
            Commands::Run(args) => {
                assert_eq!(args.allow_tool, vec!["navigate", "click"]);
                assert_eq!(args.deny_tool, vec!["bash"]);
            }
        }
    }
}
