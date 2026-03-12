use async_trait::async_trait;
use serde_json::{json, Value};

use crate::browser::mcp::{ToolExecutionResult, ToolExecutor};
use crate::config::schema::LocalToolsConfig;
use crate::error::AgentError;
use crate::llm::types::ToolDefinition;
use crate::local_tools::sandbox::{create_sandbox, BashSandbox, PathValidator};

/// Decorator that wraps a ToolExecutor and adds local file/shell tools.
///
/// When `config.enabled` is true, seven virtual tools are added:
/// - `read_file`, `write_file`, `edit_file` for file operations
/// - `bash` for sandboxed shell execution
/// - `grep` for regex search across files
/// - `glob` for file pattern matching
/// - `web_fetch` for fetching web pages and returning content as markdown
///
/// When disabled, zero overhead — all calls delegate to the inner executor.
pub struct LocalToolsExecutor<T: ToolExecutor> {
    inner: T,
    config: LocalToolsConfig,
    path_validator: PathValidator,
    sandbox: Box<dyn BashSandbox>,
    all_tools: Vec<ToolDefinition>,
}

impl<T: ToolExecutor> LocalToolsExecutor<T> {
    pub fn new(inner: T, config: LocalToolsConfig) -> Result<Self, AgentError> {
        let sandbox_root = config.sandbox_dir.clone().unwrap_or_else(|| {
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
        });

        let path_validator = PathValidator::new(sandbox_root.clone())?;
        let sandbox = create_sandbox(sandbox_root)?;

        let mut all_tools: Vec<ToolDefinition> = inner.tool_definitions().to_vec();

        if config.enabled {
            all_tools.extend(local_tool_definitions());
        }

        Ok(Self {
            inner,
            config,
            path_validator,
            sandbox,
            all_tools,
        })
    }

    /// Consume the decorator and return the inner executor for cleanup.
    pub fn into_inner(self) -> T {
        self.inner
    }
}

#[async_trait]
impl<T: ToolExecutor> ToolExecutor for LocalToolsExecutor<T> {
    fn tool_definitions(&self) -> &[ToolDefinition] {
        &self.all_tools
    }

    async fn execute_tool(
        &self,
        name: &str,
        arguments: Value,
    ) -> Result<ToolExecutionResult, AgentError> {
        if self.config.enabled {
            match name {
                "read_file" => {
                    return super::tools::read_file::execute_read_file(
                        arguments,
                        &self.path_validator,
                        self.config.read_max_bytes,
                    )
                    .await;
                }
                "write_file" => {
                    return super::tools::write_file::execute_write_file(
                        arguments,
                        &self.path_validator,
                        self.config.write_max_bytes,
                    )
                    .await;
                }
                "edit_file" => {
                    return super::tools::edit_file::execute_edit_file(
                        arguments,
                        &self.path_validator,
                    )
                    .await;
                }
                "bash" => {
                    return super::tools::bash::execute_bash(
                        arguments,
                        self.sandbox.as_ref(),
                        &self.config,
                    )
                    .await;
                }
                "grep" => {
                    return super::tools::grep::execute_grep(arguments, &self.path_validator).await;
                }
                "glob" => {
                    return super::tools::glob_tool::execute_glob(arguments, &self.path_validator)
                        .await;
                }
                "web_fetch" => {
                    return super::tools::web_fetch::execute_web_fetch(
                        arguments,
                        self.config.web_fetch_timeout_secs,
                        self.config.web_fetch_max_bytes,
                    )
                    .await;
                }
                _ => {}
            }
        }
        self.inner.execute_tool(name, arguments).await
    }
}

fn local_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "read_file".to_string(),
            description: "Read the contents of a file. Supports offset and limit for reading specific line ranges.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the file (relative to sandbox root or absolute)" },
                    "offset": { "type": "integer", "description": "Line number to start reading from (0-indexed)" },
                    "limit": { "type": "integer", "description": "Maximum number of lines to read" }
                },
                "required": ["path"]
            }),
            cache_control: None,
            read_only: false,
        },
        ToolDefinition {
            name: "write_file".to_string(),
            description: "Write content to a file. Creates the file and parent directories if they don't exist.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the file (relative to sandbox root or absolute)" },
                    "content": { "type": "string", "description": "Content to write to the file" }
                },
                "required": ["path", "content"]
            }),
            cache_control: None,
            read_only: false,
        },
        ToolDefinition {
            name: "edit_file".to_string(),
            description: "Edit a file by replacing an exact string match. The old_string must appear exactly once in the file.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the file (relative to sandbox root or absolute)" },
                    "old_string": { "type": "string", "description": "The exact string to find and replace" },
                    "new_string": { "type": "string", "description": "The replacement string" }
                },
                "required": ["path", "old_string", "new_string"]
            }),
            cache_control: None,
            read_only: false,
        },
        ToolDefinition {
            name: "bash".to_string(),
            description: "Execute a shell command in a sandboxed environment. The command runs with the sandbox directory as the working directory.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "The shell command to execute" },
                    "timeout_secs": { "type": "integer", "description": "Timeout in seconds (default: 120)" }
                },
                "required": ["command"]
            }),
            cache_control: None,
            read_only: false,
        },
        ToolDefinition {
            name: "grep".to_string(),
            description: "Search for a regex pattern across files. Returns matching lines with file paths and line numbers.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Regex pattern to search for" },
                    "path": { "type": "string", "description": "File or directory to search in (default: sandbox root)" },
                    "include": { "type": "string", "description": "Glob pattern to filter files (e.g., '*.rs', '*.py')" },
                    "context_lines": { "type": "integer", "description": "Number of context lines before/after each match" }
                },
                "required": ["pattern"]
            }),
            cache_control: None,
            read_only: false,
        },
        ToolDefinition {
            name: "glob".to_string(),
            description: "Find files matching a glob pattern. Returns a list of matching file paths relative to the sandbox root.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Glob pattern to match files (e.g., '**/*.rs', 'src/*.py')" },
                    "path": { "type": "string", "description": "Base directory to search from (default: sandbox root)" }
                },
                "required": ["pattern"]
            }),
            cache_control: None,
            read_only: false,
        },
        ToolDefinition {
            name: "web_fetch".to_string(),
            description: "Fetch a webpage via HTTP GET and return its content as markdown. Use this for looking up documentation, gathering context from URLs, or reading web content. HTTP URLs are automatically upgraded to HTTPS. For full browser automation (JavaScript rendering, clicking, form filling), use the browser tools instead.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The URL to fetch (http or https, http is auto-upgraded to https)"
                    },
                    "headers": {
                        "type": "object",
                        "description": "Optional custom HTTP headers to include in the request",
                        "additionalProperties": { "type": "string" }
                    }
                },
                "required": ["url"]
            }),
            cache_control: None,
            read_only: true,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::fs;
    use tempfile::TempDir;

    struct MockInnerExecutor {
        tools: Vec<ToolDefinition>,
    }

    #[async_trait]
    impl ToolExecutor for MockInnerExecutor {
        fn tool_definitions(&self) -> &[ToolDefinition] {
            &self.tools
        }

        async fn execute_tool(
            &self,
            name: &str,
            _arguments: Value,
        ) -> Result<ToolExecutionResult, AgentError> {
            Ok(ToolExecutionResult {
                content: format!("inner executed: {name}"),
                is_error: false,
            })
        }
    }

    fn mock_inner() -> MockInnerExecutor {
        MockInnerExecutor {
            tools: vec![ToolDefinition {
                name: "navigate".to_string(),
                description: "Navigate to URL".to_string(),
                input_schema: json!({"type": "object"}),
                cache_control: None,
                read_only: false,
            }],
        }
    }

    fn test_config(dir: &TempDir) -> LocalToolsConfig {
        LocalToolsConfig {
            enabled: true,
            sandbox_dir: Some(dir.path().to_path_buf()),
            ..Default::default()
        }
    }

    fn disabled_config(dir: &TempDir) -> LocalToolsConfig {
        LocalToolsConfig {
            enabled: false,
            sandbox_dir: Some(dir.path().to_path_buf()),
            ..Default::default()
        }
    }

    #[test]
    fn test_enabled_adds_virtual_tools() {
        let dir = TempDir::new().unwrap();
        let inner = mock_inner();
        let executor = LocalToolsExecutor::new(inner, test_config(&dir)).unwrap();
        // 1 inner + 7 local tools = 8
        assert_eq!(executor.tool_definitions().len(), 8);
        let names: Vec<&str> = executor
            .tool_definitions()
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert!(names.contains(&"navigate"));
        assert!(names.contains(&"read_file"));
        assert!(names.contains(&"write_file"));
        assert!(names.contains(&"edit_file"));
        assert!(names.contains(&"bash"));
        assert!(names.contains(&"grep"));
        assert!(names.contains(&"glob"));
        assert!(names.contains(&"web_fetch"));
    }

    #[test]
    fn test_disabled_no_virtual_tools() {
        let dir = TempDir::new().unwrap();
        let inner = mock_inner();
        let executor = LocalToolsExecutor::new(inner, disabled_config(&dir)).unwrap();
        assert_eq!(executor.tool_definitions().len(), 1);
        assert_eq!(executor.tool_definitions()[0].name, "navigate");
    }

    #[tokio::test]
    async fn test_unknown_tool_delegates_to_inner() {
        let dir = TempDir::new().unwrap();
        let inner = mock_inner();
        let executor = LocalToolsExecutor::new(inner, test_config(&dir)).unwrap();

        let result = executor
            .execute_tool("navigate", json!({"url": "test"}))
            .await
            .unwrap();
        assert_eq!(result.content, "inner executed: navigate");
    }

    #[tokio::test]
    async fn test_read_file_dispatch() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("hello.txt"), "Hello from dispatch").unwrap();
        let inner = mock_inner();
        let executor = LocalToolsExecutor::new(inner, test_config(&dir)).unwrap();

        let result = executor
            .execute_tool("read_file", json!({"path": "hello.txt"}))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert_eq!(result.content, "Hello from dispatch");
    }

    #[tokio::test]
    async fn test_write_file_dispatch() {
        let dir = TempDir::new().unwrap();
        let inner = mock_inner();
        let executor = LocalToolsExecutor::new(inner, test_config(&dir)).unwrap();

        let result = executor
            .execute_tool(
                "write_file",
                json!({"path": "out.txt", "content": "written"}),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("Successfully wrote"));

        let content = fs::read_to_string(dir.path().join("out.txt")).unwrap();
        assert_eq!(content, "written");
    }

    #[tokio::test]
    async fn test_edit_file_dispatch() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("edit.txt"), "hello world").unwrap();
        let inner = mock_inner();
        let executor = LocalToolsExecutor::new(inner, test_config(&dir)).unwrap();

        let result = executor
            .execute_tool(
                "edit_file",
                json!({"path": "edit.txt", "old_string": "hello", "new_string": "goodbye"}),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("Successfully edited"));

        let content = fs::read_to_string(dir.path().join("edit.txt")).unwrap();
        assert_eq!(content, "goodbye world");
    }

    #[tokio::test]
    async fn test_bash_dispatch() {
        let dir = TempDir::new().unwrap();
        let inner = mock_inner();
        let executor = LocalToolsExecutor::new(inner, test_config(&dir)).unwrap();

        let result = executor
            .execute_tool("bash", json!({"command": "echo sandbox_test"}))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("sandbox_test"));
    }

    #[tokio::test]
    async fn test_grep_dispatch() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("search.txt"), "find this needle here").unwrap();
        let inner = mock_inner();
        let executor = LocalToolsExecutor::new(inner, test_config(&dir)).unwrap();

        let result = executor
            .execute_tool("grep", json!({"pattern": "needle"}))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("needle"));
    }

    #[tokio::test]
    async fn test_glob_dispatch() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("a.txt"), "a").unwrap();
        fs::write(dir.path().join("b.rs"), "b").unwrap();
        let inner = mock_inner();
        let executor = LocalToolsExecutor::new(inner, test_config(&dir)).unwrap();

        let result = executor
            .execute_tool("glob", json!({"pattern": "*.txt"}))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("a.txt"));
        assert!(!result.content.contains("b.rs"));
    }

    #[test]
    fn test_into_inner() {
        let dir = TempDir::new().unwrap();
        let inner = mock_inner();
        let executor = LocalToolsExecutor::new(inner, test_config(&dir)).unwrap();
        let recovered = executor.into_inner();
        assert_eq!(recovered.tool_definitions().len(), 1);
    }

    #[test]
    fn test_tool_definitions_count() {
        let defs = local_tool_definitions();
        assert_eq!(defs.len(), 7);
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "read_file",
                "write_file",
                "edit_file",
                "bash",
                "grep",
                "glob",
                "web_fetch",
            ]
        );
    }

    #[tokio::test]
    async fn test_disabled_delegates_all() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("file.txt"), "content").unwrap();
        let inner = mock_inner();
        let executor = LocalToolsExecutor::new(inner, disabled_config(&dir)).unwrap();

        // Even local tool names should delegate to inner when disabled
        let result = executor
            .execute_tool("read_file", json!({"path": "file.txt"}))
            .await
            .unwrap();
        assert_eq!(result.content, "inner executed: read_file");

        let result = executor
            .execute_tool("bash", json!({"command": "echo hi"}))
            .await
            .unwrap();
        assert_eq!(result.content, "inner executed: bash");
    }
}
