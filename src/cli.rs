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
    /// Launch an interactive chat session
    #[cfg(feature = "tui")]
    Chat(Box<ChatArgs>),
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

/// Flags shared by `run` and `chat`.
///
/// `RunArgs` and `ChatArgs` previously declared these 38 fields twice, with
/// their own doc comments, env-var bindings and defaults, and a hand-written
/// field-by-field conversion between them. Flattening one definition into both means a
/// new flag is declared once and cannot drift between the two subcommands.
#[derive(clap::Args, Debug, Clone, Default)]
pub struct CommonArgs {
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

    /// Maximum agent loop iterations
    #[arg(long)]
    pub max_iterations: Option<u32>,

    /// Run browser in headed mode (visible)
    #[arg(long)]
    pub headed: bool,

    /// Disable browser connection (terminal-only mode)
    #[arg(long)]
    pub no_browser: bool,

    /// Enable verbose logging to stderr
    #[arg(short, long)]
    pub verbose: bool,

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

    /// Custom system prompt
    #[arg(long)]
    pub system_prompt: Option<String>,

    /// Maximum bytes for tool result output (truncation threshold)
    #[arg(long, env = "REMIX_TOOL_RESULT_MAX_BYTES")]
    pub tool_result_max_bytes: Option<usize>,

    /// Override context window size in tokens for compaction
    #[arg(long, env = "REMIX_CONTEXT_WINDOW")]
    pub context_window: Option<u32>,

    /// Disable context compaction entirely
    #[arg(long)]
    pub disable_compaction: bool,

    /// Disable all dev tools (LSP, test harness, repo map)
    #[arg(long)]
    pub no_dev_tools: bool,

    /// Disable LSP integration
    #[arg(long)]
    pub no_lsp: bool,

    /// Disable test harness tools
    #[arg(long)]
    pub no_test_harness: bool,

    /// Disable repo map tool
    #[arg(long)]
    pub no_repo_map: bool,

    /// Override LSP server command for a language (e.g., "rust=rust-analyzer")
    #[arg(long = "lsp-server", value_name = "LANG=CMD")]
    pub lsp_server: Vec<String>,

    /// Base thinking budget tokens for LLM requests
    #[arg(long)]
    pub thinking_budget_tokens: Option<u32>,
}

#[derive(clap::Args, Debug, Default)]
pub struct RunArgs {
    #[command(flatten)]
    pub common: CommonArgs,

    /// Task to execute (natural language)
    pub task: Option<String>,

    /// Maximum duration in seconds
    #[arg(long)]
    pub timeout: Option<u64>,

    /// Write results to file instead of stdout
    #[arg(short, long)]
    pub output: Option<PathBuf>,

    /// Fork from an existing session
    #[arg(long)]
    pub fork_session: Option<String>,

    /// Effort level (low, medium, high, max)
    #[arg(long)]
    pub effort: Option<EffortLevel>,

    /// Port for SSE event server (enables real-time streaming to UI)
    #[arg(long, env = "REMIX_SSE_PORT")]
    pub sse_port: Option<u16>,

    /// Nudge the LLM to continue when it returns text-only responses without tool use
    #[arg(long)]
    pub nudge_on_text_only: bool,

    /// Maximum number of text-only nudges before terminating (default: 3)
    #[arg(long)]
    pub nudge_max_count: Option<u32>,

    /// Verify goal completion before terminating (one-time check)
    #[arg(long)]
    pub goal_check_on_complete: bool,

    /// Inject action reminders every N iterations (e.g., 15)
    #[arg(long)]
    pub action_reminder_interval: Option<u32>,

    /// Enable loop detection (repeated same tool+input) with default settings
    #[arg(long)]
    pub loop_detection: bool,

    /// Max repeated identical tool calls before triggering loop warning (default: 3)
    #[arg(long)]
    pub loop_detection_max_repeats: Option<u32>,

    /// Lookback window size for loop detection (default: 10)
    #[arg(long)]
    pub loop_detection_window: Option<u32>,

    /// Max failing commands without a file write before semantic loop warning
    #[arg(long)]
    pub loop_detection_max_failures: Option<u32>,

    /// Enable reasoning stages (varies thinking budget across planning/execution/verification)
    #[arg(long)]
    pub reasoning_stages: bool,

    /// Thinking budget tokens for planning phase (default: 10000)
    #[arg(long)]
    pub planning_budget_tokens: Option<u32>,

    /// Thinking budget tokens for execution phase (default: 5000)
    #[arg(long)]
    pub execution_budget_tokens: Option<u32>,

    /// Thinking budget tokens for verification phase (default: 10000)
    #[arg(long)]
    pub verification_budget_tokens: Option<u32>,

    /// Inject one-time budget warning at this fraction of max_iterations (e.g., 0.7)
    #[arg(long)]
    pub iteration_budget_warning_threshold: Option<f32>,
}

/// Arguments for the interactive chat command.
/// Shares most fields with RunArgs but has different defaults for interactive use.
#[cfg(feature = "tui")]
#[derive(clap::Args, Debug)]
pub struct ChatArgs {
    #[command(flatten)]
    pub common: CommonArgs,
}

#[cfg(feature = "tui")]
impl ChatArgs {
    /// Convert ChatArgs into a RunArgs with interactive-optimized defaults.
    pub fn to_run_args(&self) -> RunArgs {
        // Only the fields chat does not share, plus the interactive-mode defaults.
        // Everything else rides along in `common`, so a new shared flag needs no change
        // here — it used to require a line in this function and a field in both structs.
        let mut common = self.common.clone();
        common.max_iterations = common.max_iterations.or(Some(75));
        common.permission_mode = common
            .permission_mode
            .clone()
            .or_else(|| Some("accept_edits".to_string()));
        common.thinking_budget_tokens = common.thinking_budget_tokens.or(Some(10000));

        RunArgs {
            common,
            task: None,
            timeout: None,
            output: None,
            fork_session: None,
            effort: None,
            sse_port: None,
            nudge_on_text_only: false,
            nudge_max_count: None,
            goal_check_on_complete: false,
            action_reminder_interval: None,
            loop_detection: true,
            loop_detection_max_repeats: Some(3),
            loop_detection_window: Some(10),
            loop_detection_max_failures: None,
            reasoning_stages: true,
            planning_budget_tokens: Some(10000),
            execution_budget_tokens: Some(5000),
            verification_budget_tokens: Some(10000),
            iteration_budget_warning_threshold: None,
        }
    }
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
        assert!(args.common.config.is_none());
        assert!(args.common.base_url.is_none());
        assert!(args.common.api_key.is_none());
        assert!(args.common.model.is_none());
        assert!(args.common.max_tokens.is_none());
        assert!(args.timeout.is_none());
        assert!(args.common.max_iterations.is_none());
        assert!(!args.common.headed);
        assert!(!args.common.verbose);
        assert!(args.output.is_none());
        assert!(args.common.browser_path.is_none());
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
        assert_eq!(args.common.config, Some(PathBuf::from("task.yaml")));
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
            "--no-browser",
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
            "--system-prompt",
            "You are a helpful agent",
            "--context-window",
            "128000",
            "--disable-compaction",
            "--tool-result-max-bytes",
            "16384",
            "--nudge-on-text-only",
            "--nudge-max-count",
            "5",
            "--goal-check-on-complete",
            "--action-reminder-interval",
            "15",
            "--loop-detection",
            "--loop-detection-max-repeats",
            "4",
            "--loop-detection-window",
            "12",
            "--loop-detection-max-failures",
            "6",
            "--reasoning-stages",
            "--planning-budget-tokens",
            "12000",
            "--execution-budget-tokens",
            "6000",
            "--verification-budget-tokens",
            "15000",
            "--iteration-budget-warning-threshold",
            "0.7",
            "--thinking-budget-tokens",
            "10000",
            "do something",
        ]));
        assert_eq!(args.task, Some("do something".to_string()));
        assert_eq!(args.common.config, Some(PathBuf::from("task.yaml")));
        assert_eq!(
            args.common.base_url,
            Some("https://api.example.com".to_string())
        );
        assert_eq!(args.common.api_key, Some("sk-test".to_string()));
        assert_eq!(args.common.model, Some("gpt-4".to_string()));
        assert_eq!(args.common.max_tokens, Some(4096));
        assert_eq!(args.timeout, Some(600));
        assert_eq!(args.common.max_iterations, Some(100));
        assert!(args.common.headed);
        assert!(args.common.no_browser);
        assert!(args.common.verbose);
        assert_eq!(args.output, Some(PathBuf::from("result.json")));
        assert_eq!(
            args.common.browser_path,
            Some("/usr/local/bin/remix-browser".to_string())
        );
        assert_eq!(args.common.skills_dir, Some(PathBuf::from("/tmp/skills")));
        assert!(args.common.no_skills);
        assert_eq!(
            args.common.agents_md_dir,
            Some(PathBuf::from("/tmp/agents"))
        );
        assert!(args.common.no_agents_md);
        assert!(args.common.no_local_tools);
        assert_eq!(args.common.sandbox_dir, Some(PathBuf::from("/tmp/sandbox")));
        assert!(args.common.no_plugins);
        assert_eq!(args.common.plugins_dir, Some(PathBuf::from("/tmp/plugins")));
        assert!(args.common.no_claude_plugins);
        assert_eq!(args.common.session_id, Some("abc-123".to_string()));
        assert_eq!(args.fork_session, Some("def-456".to_string()));
        assert_eq!(
            args.common.session_dir,
            Some(PathBuf::from("/tmp/sessions"))
        );
        assert_eq!(args.common.permission_mode, Some("plan".to_string()));
        assert_eq!(args.common.allow_tool, vec!["navigate", "click"]);
        assert_eq!(args.common.deny_tool, vec!["bash"]);
        assert!(args.common.no_coordination);
        assert_eq!(args.common.max_workers, Some(10));
        assert_eq!(
            args.common.coordination_dir,
            Some(PathBuf::from("/tmp/coordination"))
        );
        assert_eq!(args.sse_port, Some(3100));
        assert_eq!(
            args.common.system_prompt,
            Some("You are a helpful agent".to_string())
        );
        assert_eq!(args.common.context_window, Some(128_000));
        assert!(args.common.disable_compaction);
        assert_eq!(args.common.tool_result_max_bytes, Some(16_384));
        assert!(args.nudge_on_text_only);
        assert_eq!(args.nudge_max_count, Some(5));
        assert!(args.goal_check_on_complete);
        assert_eq!(args.action_reminder_interval, Some(15));
        assert!(args.loop_detection);
        assert_eq!(args.loop_detection_max_repeats, Some(4));
        assert_eq!(args.loop_detection_window, Some(12));
        assert_eq!(args.loop_detection_max_failures, Some(6));
        assert!(args.reasoning_stages);
        assert_eq!(args.planning_budget_tokens, Some(12_000));
        assert_eq!(args.execution_budget_tokens, Some(6_000));
        assert_eq!(args.verification_budget_tokens, Some(15_000));
        assert_eq!(args.iteration_budget_warning_threshold, Some(0.7));
        assert_eq!(args.common.thinking_budget_tokens, Some(10_000));
    }

    #[test]
    fn test_parse_run_with_system_prompt() {
        let args = extract_run_args(Cli::parse_from([
            "remix-agent",
            "run",
            "--system-prompt",
            "You are a helpful agent",
        ]));
        assert_eq!(
            args.common.system_prompt,
            Some("You are a helpful agent".to_string())
        );
    }

    #[test]
    fn test_parse_run_system_prompt_default_none() {
        let args = extract_run_args(Cli::parse_from(["remix-agent", "run"]));
        assert!(args.common.system_prompt.is_none());
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
        assert_eq!(args.common.config, Some(PathBuf::from("task.yaml")));
        assert!(args.common.verbose);
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
        assert_eq!(
            args.common.skills_dir,
            Some(PathBuf::from("/path/to/skills"))
        );
        assert!(!args.common.no_skills);
    }

    #[test]
    fn test_parse_run_with_no_skills() {
        let args = extract_run_args(Cli::parse_from(["remix-agent", "run", "--no-skills"]));
        assert!(args.common.no_skills);
        assert!(args.common.skills_dir.is_none());
    }

    #[test]
    fn test_parse_run_with_no_plugins() {
        let args = extract_run_args(Cli::parse_from(["remix-agent", "run", "--no-plugins"]));
        assert!(args.common.no_plugins);
        assert!(args.common.plugins_dir.is_none());
        assert!(!args.common.no_claude_plugins);
    }

    #[test]
    fn test_parse_run_with_plugins_dir() {
        let args = extract_run_args(Cli::parse_from([
            "remix-agent",
            "run",
            "--plugins-dir",
            "/path/to/plugins",
        ]));
        assert_eq!(
            args.common.plugins_dir,
            Some(PathBuf::from("/path/to/plugins"))
        );
        assert!(!args.common.no_plugins);
    }

    #[test]
    fn test_parse_run_with_no_claude_plugins() {
        let args = extract_run_args(Cli::parse_from([
            "remix-agent",
            "run",
            "--no-claude-plugins",
        ]));
        assert!(args.common.no_claude_plugins);
        assert!(!args.common.no_plugins);
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
        assert_eq!(
            args.common.agents_md_dir,
            Some(PathBuf::from("/path/to/project"))
        );
        assert!(!args.common.no_agents_md);
    }

    #[test]
    fn test_parse_run_with_no_agents_md() {
        let args = extract_run_args(Cli::parse_from(["remix-agent", "run", "--no-agents-md"]));
        assert!(args.common.no_agents_md);
        assert!(args.common.agents_md_dir.is_none());
    }

    #[test]
    fn test_parse_run_with_no_local_tools() {
        let args = extract_run_args(Cli::parse_from(["remix-agent", "run", "--no-local-tools"]));
        assert!(args.common.no_local_tools);
        assert!(args.common.sandbox_dir.is_none());
    }

    #[test]
    fn test_parse_run_with_sandbox_dir() {
        let args = extract_run_args(Cli::parse_from([
            "remix-agent",
            "run",
            "--sandbox-dir",
            "/tmp/sandbox",
        ]));
        assert_eq!(args.common.sandbox_dir, Some(PathBuf::from("/tmp/sandbox")));
        assert!(!args.common.no_local_tools);
    }

    #[test]
    fn test_parse_run_with_session_id() {
        let args = extract_run_args(Cli::parse_from([
            "remix-agent",
            "run",
            "--session-id",
            "abc-123",
        ]));
        assert_eq!(args.common.session_id, Some("abc-123".to_string()));
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
        assert!(args.common.session_id.is_none());
    }

    #[test]
    fn test_parse_run_with_session_dir() {
        let args = extract_run_args(Cli::parse_from([
            "remix-agent",
            "run",
            "--session-dir",
            "/tmp/sessions",
        ]));
        assert_eq!(
            args.common.session_dir,
            Some(PathBuf::from("/tmp/sessions"))
        );
    }

    #[test]
    fn test_parse_run_with_permission_mode() {
        let args = extract_run_args(Cli::parse_from([
            "remix-agent",
            "run",
            "--permission-mode",
            "plan",
        ]));
        assert_eq!(args.common.permission_mode, Some("plan".to_string()));
    }

    #[test]
    fn test_parse_run_with_no_coordination() {
        let args = extract_run_args(Cli::parse_from(["remix-agent", "run", "--no-coordination"]));
        assert!(args.common.no_coordination);
        assert!(args.common.max_workers.is_none());
        assert!(args.common.coordination_dir.is_none());
    }

    #[test]
    fn test_parse_run_with_max_workers() {
        let args = extract_run_args(Cli::parse_from([
            "remix-agent",
            "run",
            "--max-workers",
            "8",
        ]));
        assert_eq!(args.common.max_workers, Some(8));
        assert!(!args.common.no_coordination);
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
            args.common.coordination_dir,
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
        assert_eq!(args.common.allow_tool, vec!["navigate", "click"]);
        assert_eq!(args.common.deny_tool, vec!["bash"]);
    }

    // --- New tests for --continue, --effort, and sessions subcommand ---

    #[test]
    fn test_parse_run_with_continue_flag() {
        let args = extract_run_args(Cli::parse_from(["remix-agent", "run", "--continue"]));
        assert!(args.common.continue_session);
    }

    #[test]
    fn test_parse_run_with_continue_session_alias() {
        let args = extract_run_args(Cli::parse_from([
            "remix-agent",
            "run",
            "--continue-session",
        ]));
        assert!(args.common.continue_session);
    }

    #[test]
    fn test_parse_run_with_resume_alias() {
        let args = extract_run_args(Cli::parse_from(["remix-agent", "run", "--resume"]));
        assert!(args.common.continue_session);
    }

    #[test]
    fn test_parse_run_continue_default_false() {
        let args = extract_run_args(Cli::parse_from(["remix-agent", "run"]));
        assert!(!args.common.continue_session);
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
    fn test_parse_run_with_no_browser() {
        let args = extract_run_args(Cli::parse_from(["remix-agent", "run", "--no-browser"]));
        assert!(args.common.no_browser);
    }

    #[test]
    fn test_parse_run_no_browser_default_false() {
        let args = extract_run_args(Cli::parse_from(["remix-agent", "run"]));
        assert!(!args.common.no_browser);
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

    #[test]
    fn test_parse_run_with_context_window() {
        let args = extract_run_args(Cli::parse_from([
            "remix-agent",
            "run",
            "--context-window",
            "128000",
        ]));
        assert_eq!(args.common.context_window, Some(128_000));
    }

    #[test]
    fn test_parse_run_context_window_default_none() {
        let args = extract_run_args(Cli::parse_from(["remix-agent", "run"]));
        assert!(args.common.context_window.is_none());
    }

    #[test]
    fn test_parse_run_with_disable_compaction() {
        let args = extract_run_args(Cli::parse_from([
            "remix-agent",
            "run",
            "--disable-compaction",
        ]));
        assert!(args.common.disable_compaction);
    }

    #[test]
    fn test_parse_run_disable_compaction_default_false() {
        let args = extract_run_args(Cli::parse_from(["remix-agent", "run"]));
        assert!(!args.common.disable_compaction);
    }

    #[test]
    fn test_parse_run_with_tool_result_max_bytes() {
        let args = extract_run_args(Cli::parse_from([
            "remix-agent",
            "run",
            "--tool-result-max-bytes",
            "16384",
        ]));
        assert_eq!(args.common.tool_result_max_bytes, Some(16_384));
    }

    #[test]
    fn test_parse_run_tool_result_max_bytes_default_none() {
        let args = extract_run_args(Cli::parse_from(["remix-agent", "run"]));
        assert!(args.common.tool_result_max_bytes.is_none());
    }

    #[test]
    fn test_parse_run_with_nudge_on_text_only() {
        let args = extract_run_args(Cli::parse_from([
            "remix-agent",
            "run",
            "--nudge-on-text-only",
        ]));
        assert!(args.nudge_on_text_only);
        assert!(args.nudge_max_count.is_none());
    }

    #[test]
    fn test_parse_run_with_nudge_max_count() {
        let args = extract_run_args(Cli::parse_from([
            "remix-agent",
            "run",
            "--nudge-max-count",
            "10",
        ]));
        assert_eq!(args.nudge_max_count, Some(10));
        assert!(!args.nudge_on_text_only);
    }

    #[test]
    fn test_parse_run_with_goal_check_on_complete() {
        let args = extract_run_args(Cli::parse_from([
            "remix-agent",
            "run",
            "--goal-check-on-complete",
        ]));
        assert!(args.goal_check_on_complete);
    }

    #[test]
    fn test_parse_run_goal_check_on_complete_default_false() {
        let args = extract_run_args(Cli::parse_from(["remix-agent", "run"]));
        assert!(!args.goal_check_on_complete);
    }

    #[test]
    fn test_parse_run_nudge_defaults() {
        let args = extract_run_args(Cli::parse_from(["remix-agent", "run"]));
        assert!(!args.nudge_on_text_only);
        assert!(args.nudge_max_count.is_none());
    }

    #[test]
    fn test_parse_run_with_action_reminder_interval() {
        let args = extract_run_args(Cli::parse_from([
            "remix-agent",
            "run",
            "--action-reminder-interval",
            "15",
        ]));
        assert_eq!(args.action_reminder_interval, Some(15));
    }

    #[test]
    fn test_parse_run_action_reminder_interval_default_none() {
        let args = extract_run_args(Cli::parse_from(["remix-agent", "run"]));
        assert!(args.action_reminder_interval.is_none());
    }

    #[test]
    fn test_parse_run_with_loop_detection() {
        let args = extract_run_args(Cli::parse_from(["remix-agent", "run", "--loop-detection"]));
        assert!(args.loop_detection);
        assert!(args.loop_detection_max_repeats.is_none());
        assert!(args.loop_detection_window.is_none());
    }

    #[test]
    fn test_parse_run_with_loop_detection_params() {
        let args = extract_run_args(Cli::parse_from([
            "remix-agent",
            "run",
            "--loop-detection-max-repeats",
            "5",
            "--loop-detection-window",
            "20",
        ]));
        assert!(!args.loop_detection);
        assert_eq!(args.loop_detection_max_repeats, Some(5));
        assert_eq!(args.loop_detection_window, Some(20));
    }

    #[test]
    fn test_parse_run_loop_detection_defaults() {
        let args = extract_run_args(Cli::parse_from(["remix-agent", "run"]));
        assert!(!args.loop_detection);
        assert!(args.loop_detection_max_repeats.is_none());
        assert!(args.loop_detection_window.is_none());
        assert!(args.loop_detection_max_failures.is_none());
    }

    #[test]
    fn test_parse_run_with_loop_detection_max_failures() {
        let args = extract_run_args(Cli::parse_from([
            "remix-agent",
            "run",
            "--loop-detection-max-failures",
            "6",
        ]));
        assert_eq!(args.loop_detection_max_failures, Some(6));
    }

    #[test]
    fn test_parse_run_with_reasoning_stages() {
        let args = extract_run_args(Cli::parse_from([
            "remix-agent",
            "run",
            "--reasoning-stages",
        ]));
        assert!(args.reasoning_stages);
    }

    #[test]
    fn test_parse_run_with_reasoning_stages_budgets() {
        let args = extract_run_args(Cli::parse_from([
            "remix-agent",
            "run",
            "--planning-budget-tokens",
            "20000",
            "--execution-budget-tokens",
            "8000",
            "--verification-budget-tokens",
            "15000",
        ]));
        assert_eq!(args.planning_budget_tokens, Some(20_000));
        assert_eq!(args.execution_budget_tokens, Some(8_000));
        assert_eq!(args.verification_budget_tokens, Some(15_000));
    }

    #[test]
    fn test_parse_run_reasoning_stages_defaults() {
        let args = extract_run_args(Cli::parse_from(["remix-agent", "run"]));
        assert!(!args.reasoning_stages);
        assert!(args.planning_budget_tokens.is_none());
        assert!(args.execution_budget_tokens.is_none());
        assert!(args.verification_budget_tokens.is_none());
    }

    #[test]
    fn test_parse_run_with_iteration_budget_warning() {
        let args = extract_run_args(Cli::parse_from([
            "remix-agent",
            "run",
            "--iteration-budget-warning-threshold",
            "0.75",
        ]));
        assert_eq!(args.iteration_budget_warning_threshold, Some(0.75));
    }

    #[test]
    fn test_parse_run_iteration_budget_warning_default_none() {
        let args = extract_run_args(Cli::parse_from(["remix-agent", "run"]));
        assert!(args.iteration_budget_warning_threshold.is_none());
    }

    #[test]
    fn test_parse_run_with_thinking_budget_tokens() {
        let args = extract_run_args(Cli::parse_from([
            "remix-agent",
            "run",
            "--thinking-budget-tokens",
            "10000",
        ]));
        assert_eq!(args.common.thinking_budget_tokens, Some(10_000));
    }

    #[test]
    fn test_parse_run_thinking_budget_tokens_default_none() {
        let args = extract_run_args(Cli::parse_from(["remix-agent", "run"]));
        assert!(args.common.thinking_budget_tokens.is_none());
    }
}
