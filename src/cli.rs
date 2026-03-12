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
    Run(Box<RunArgs>),
    /// Manage sessions
    Sessions(SessionsArgs),
}

#[derive(clap::Args, Debug)]
pub struct SessionsArgs {
    #[command(subcommand)]
    pub command: SessionsCommand,
}

#[derive(Subcommand, Debug)]
pub enum SessionsCommand {
    /// List all sessions
    List {
        /// Override session storage directory
        #[arg(long, env = "REMIX_SESSION_DIR")]
        session_dir: Option<std::path::PathBuf>,
    },
    /// Show details of a specific session
    Show {
        /// Session ID
        id: String,
        /// Override session storage directory
        #[arg(long, env = "REMIX_SESSION_DIR")]
        session_dir: Option<std::path::PathBuf>,
    },
}

#[derive(Debug, Clone, clap::ValueEnum)]
pub enum EffortLevel {
    Low,
    Medium,
    High,
    Max,
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

    /// Resume the most recent session automatically
    #[arg(long = "continue", alias = "resume", alias = "continue-session")]
    pub continue_session: bool,

    /// Effort level (low, medium, high, max)
    #[arg(long)]
    pub effort: Option<EffortLevel>,

    /// Port for SSE event server (enables real-time streaming to UI)
    #[arg(long, env = "REMIX_SSE_PORT")]
    pub sse_port: Option<u16>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// Helper to extract RunArgs from a parsed CLI, panicking if it's not a Run command.
    fn extract_run_args(cli: Cli) -> RunArgs {
        match cli.command {
            Commands::Run(args) => *args,
            other => panic!("Expected Commands::Run, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_run_with_task() {
        let args = extract_run_args(Cli::parse_from([
            "remix-agent",
            "run",
            "navigate to google.com",
        ]));
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

    #[test]
    fn test_parse_run_without_task() {
        let args = extract_run_args(Cli::parse_from(["remix-agent", "run"]));
        assert!(args.task.is_none());
    }

    #[test]
    fn test_parse_run_with_config() {
        let args = extract_run_args(Cli::parse_from([
            "remix-agent",
            "run",
            "--config",
            "task.yaml",
        ]));
        assert_eq!(args.config, Some(PathBuf::from("task.yaml")));
    }

    #[test]
    fn test_parse_run_with_all_flags() {
        let args = extract_run_args(Cli::parse_from([
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
            "--sse-port",
            "3100",
            "do something",
        ]));
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
        assert_eq!(args.sse_port, Some(3100));
    }

    #[test]
    fn test_parse_run_short_flags() {
        let args = extract_run_args(Cli::parse_from([
            "remix-agent",
            "run",
            "-c",
            "task.yaml",
            "-v",
            "-o",
            "out.json",
        ]));
        assert_eq!(args.config, Some(PathBuf::from("task.yaml")));
        assert!(args.verbose);
        assert_eq!(args.output, Some(PathBuf::from("out.json")));
    }

    #[test]
    fn test_parse_run_with_skills_dir() {
        let args = extract_run_args(Cli::parse_from([
            "remix-agent",
            "run",
            "--skills-dir",
            "/path/to/skills",
        ]));
        assert_eq!(args.skills_dir, Some(PathBuf::from("/path/to/skills")));
        assert!(!args.no_skills);
    }

    #[test]
    fn test_parse_run_with_no_skills() {
        let args = extract_run_args(Cli::parse_from(["remix-agent", "run", "--no-skills"]));
        assert!(args.no_skills);
        assert!(args.skills_dir.is_none());
    }

    #[test]
    fn test_parse_run_with_no_plugins() {
        let args = extract_run_args(Cli::parse_from(["remix-agent", "run", "--no-plugins"]));
        assert!(args.no_plugins);
        assert!(args.plugins_dir.is_none());
        assert!(!args.no_claude_plugins);
    }

    #[test]
    fn test_parse_run_with_plugins_dir() {
        let args = extract_run_args(Cli::parse_from([
            "remix-agent",
            "run",
            "--plugins-dir",
            "/path/to/plugins",
        ]));
        assert_eq!(args.plugins_dir, Some(PathBuf::from("/path/to/plugins")));
        assert!(!args.no_plugins);
    }

    #[test]
    fn test_parse_run_with_no_claude_plugins() {
        let args = extract_run_args(Cli::parse_from([
            "remix-agent",
            "run",
            "--no-claude-plugins",
        ]));
        assert!(args.no_claude_plugins);
        assert!(!args.no_plugins);
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
        let args = extract_run_args(Cli::parse_from([
            "remix-agent",
            "run",
            "--agents-md-dir",
            "/path/to/project",
        ]));
        assert_eq!(args.agents_md_dir, Some(PathBuf::from("/path/to/project")));
        assert!(!args.no_agents_md);
    }

    #[test]
    fn test_parse_run_with_no_agents_md() {
        let args = extract_run_args(Cli::parse_from(["remix-agent", "run", "--no-agents-md"]));
        assert!(args.no_agents_md);
        assert!(args.agents_md_dir.is_none());
    }

    #[test]
    fn test_parse_run_with_no_local_tools() {
        let args = extract_run_args(Cli::parse_from(["remix-agent", "run", "--no-local-tools"]));
        assert!(args.no_local_tools);
        assert!(args.sandbox_dir.is_none());
    }

    #[test]
    fn test_parse_run_with_sandbox_dir() {
        let args = extract_run_args(Cli::parse_from([
            "remix-agent",
            "run",
            "--sandbox-dir",
            "/tmp/sandbox",
        ]));
        assert_eq!(args.sandbox_dir, Some(PathBuf::from("/tmp/sandbox")));
        assert!(!args.no_local_tools);
    }

    #[test]
    fn test_parse_run_with_session_id() {
        let args = extract_run_args(Cli::parse_from([
            "remix-agent",
            "run",
            "--session-id",
            "abc-123",
        ]));
        assert_eq!(args.session_id, Some("abc-123".to_string()));
        assert!(args.fork_session.is_none());
    }

    #[test]
    fn test_parse_run_with_fork_session() {
        let args = extract_run_args(Cli::parse_from([
            "remix-agent",
            "run",
            "--fork-session",
            "def-456",
        ]));
        assert_eq!(args.fork_session, Some("def-456".to_string()));
        assert!(args.session_id.is_none());
    }

    #[test]
    fn test_parse_run_with_session_dir() {
        let args = extract_run_args(Cli::parse_from([
            "remix-agent",
            "run",
            "--session-dir",
            "/tmp/sessions",
        ]));
        assert_eq!(args.session_dir, Some(PathBuf::from("/tmp/sessions")));
    }

    #[test]
    fn test_parse_run_with_permission_mode() {
        let args = extract_run_args(Cli::parse_from([
            "remix-agent",
            "run",
            "--permission-mode",
            "plan",
        ]));
        assert_eq!(args.permission_mode, Some("plan".to_string()));
    }

    #[test]
    fn test_parse_run_with_no_coordination() {
        let args = extract_run_args(Cli::parse_from(["remix-agent", "run", "--no-coordination"]));
        assert!(args.no_coordination);
        assert!(args.max_workers.is_none());
        assert!(args.coordination_dir.is_none());
    }

    #[test]
    fn test_parse_run_with_max_workers() {
        let args = extract_run_args(Cli::parse_from([
            "remix-agent",
            "run",
            "--max-workers",
            "8",
        ]));
        assert_eq!(args.max_workers, Some(8));
        assert!(!args.no_coordination);
    }

    #[test]
    fn test_parse_run_with_coordination_dir() {
        let args = extract_run_args(Cli::parse_from([
            "remix-agent",
            "run",
            "--coordination-dir",
            "/tmp/coordination",
        ]));
        assert_eq!(
            args.coordination_dir,
            Some(PathBuf::from("/tmp/coordination"))
        );
    }

    #[test]
    fn test_parse_run_with_allow_deny_tools() {
        let args = extract_run_args(Cli::parse_from([
            "remix-agent",
            "run",
            "--allow-tool",
            "navigate",
            "--allow-tool",
            "click",
            "--deny-tool",
            "bash",
        ]));
        assert_eq!(args.allow_tool, vec!["navigate", "click"]);
        assert_eq!(args.deny_tool, vec!["bash"]);
    }

    // --- New tests for --continue, --effort, and sessions subcommand ---

    #[test]
    fn test_parse_run_with_continue_flag() {
        let args = extract_run_args(Cli::parse_from(["remix-agent", "run", "--continue"]));
        assert!(args.continue_session);
    }

    #[test]
    fn test_parse_run_with_continue_session_alias() {
        let args = extract_run_args(Cli::parse_from([
            "remix-agent",
            "run",
            "--continue-session",
        ]));
        assert!(args.continue_session);
    }

    #[test]
    fn test_parse_run_with_resume_alias() {
        let args = extract_run_args(Cli::parse_from(["remix-agent", "run", "--resume"]));
        assert!(args.continue_session);
    }

    #[test]
    fn test_parse_run_continue_default_false() {
        let args = extract_run_args(Cli::parse_from(["remix-agent", "run"]));
        assert!(!args.continue_session);
    }

    #[test]
    fn test_parse_run_with_effort() {
        let args = extract_run_args(Cli::parse_from(["remix-agent", "run", "--effort", "high"]));
        assert!(matches!(args.effort, Some(EffortLevel::High)));
    }

    #[test]
    fn test_parse_run_with_effort_low() {
        let args = extract_run_args(Cli::parse_from(["remix-agent", "run", "--effort", "low"]));
        assert!(matches!(args.effort, Some(EffortLevel::Low)));
    }

    #[test]
    fn test_parse_run_with_effort_max() {
        let args = extract_run_args(Cli::parse_from(["remix-agent", "run", "--effort", "max"]));
        assert!(matches!(args.effort, Some(EffortLevel::Max)));
    }

    #[test]
    fn test_parse_run_effort_default_none() {
        let args = extract_run_args(Cli::parse_from(["remix-agent", "run"]));
        assert!(args.effort.is_none());
    }

    #[test]
    fn test_parse_run_with_sse_port() {
        let args = extract_run_args(Cli::parse_from([
            "remix-agent",
            "run",
            "--sse-port",
            "8080",
        ]));
        assert_eq!(args.sse_port, Some(8080));
    }

    #[test]
    fn test_parse_run_sse_port_default_none() {
        let args = extract_run_args(Cli::parse_from(["remix-agent", "run"]));
        assert!(args.sse_port.is_none());
    }

    #[test]
    fn test_parse_sessions_list() {
        let cli = Cli::parse_from(["remix-agent", "sessions", "list"]);
        match cli.command {
            Commands::Sessions(sessions_args) => match sessions_args.command {
                SessionsCommand::List { session_dir } => {
                    assert!(session_dir.is_none());
                }
                other => panic!("Expected SessionsCommand::List, got {:?}", other),
            },
            other => panic!("Expected Commands::Sessions, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_sessions_list_with_session_dir() {
        let cli = Cli::parse_from([
            "remix-agent",
            "sessions",
            "list",
            "--session-dir",
            "/tmp/sessions",
        ]);
        match cli.command {
            Commands::Sessions(sessions_args) => match sessions_args.command {
                SessionsCommand::List { session_dir } => {
                    assert_eq!(session_dir, Some(PathBuf::from("/tmp/sessions")));
                }
                other => panic!("Expected SessionsCommand::List, got {:?}", other),
            },
            other => panic!("Expected Commands::Sessions, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_sessions_show() {
        let cli = Cli::parse_from(["remix-agent", "sessions", "show", "abc-123"]);
        match cli.command {
            Commands::Sessions(sessions_args) => match sessions_args.command {
                SessionsCommand::Show { id, session_dir } => {
                    assert_eq!(id, "abc-123");
                    assert!(session_dir.is_none());
                }
                other => panic!("Expected SessionsCommand::Show, got {:?}", other),
            },
            other => panic!("Expected Commands::Sessions, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_sessions_show_with_session_dir() {
        let cli = Cli::parse_from([
            "remix-agent",
            "sessions",
            "show",
            "abc-123",
            "--session-dir",
            "/tmp/sessions",
        ]);
        match cli.command {
            Commands::Sessions(sessions_args) => match sessions_args.command {
                SessionsCommand::Show { id, session_dir } => {
                    assert_eq!(id, "abc-123");
                    assert_eq!(session_dir, Some(PathBuf::from("/tmp/sessions")));
                }
                other => panic!("Expected SessionsCommand::Show, got {:?}", other),
            },
            other => panic!("Expected Commands::Sessions, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_sessions_missing_subcommand_fails() {
        let result = Cli::try_parse_from(["remix-agent", "sessions"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_sessions_show_missing_id_fails() {
        let result = Cli::try_parse_from(["remix-agent", "sessions", "show"]);
        assert!(result.is_err());
    }
}
