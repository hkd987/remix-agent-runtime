use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::browser::mcp::{ToolExecutionResult, ToolExecutor};
use crate::config::schema::DevToolsConfig;
use crate::error::AgentError;
use crate::llm::types::ToolDefinition;
use crate::local_tools::sandbox::BashSandbox;

use super::lsp::manager::LspServerManager;

/// Decorator that wraps a ToolExecutor and adds dev tools:
/// - LSP integration (goto definition, find references, diagnostics)
/// - Test harness (detect framework, run tests, list tests)
/// - Repo map (structural code overview)
pub struct DevToolsExecutor<T: ToolExecutor> {
    inner: T,
    config: DevToolsConfig,
    all_tools: Vec<ToolDefinition>,
    lsp_manager: Arc<Mutex<LspServerManager>>,
    /// Sandbox used to run test-harness subprocesses.
    ///
    /// `run_tests` and `list_tests` spawn real build tooling. Without this they used
    /// `tokio::process::Command` directly, escaping both the OS-level sandbox and the
    /// configured root — a hole as wide as the `bash` tool but with none of its gating.
    sandbox: Arc<dyn BashSandbox>,
}

impl<T: ToolExecutor> DevToolsExecutor<T> {
    pub fn new(inner: T, config: DevToolsConfig, sandbox: Arc<dyn BashSandbox>) -> Self {
        let mut all_tools: Vec<ToolDefinition> = inner.tool_definitions().to_vec();

        if config.enabled {
            if config.lsp.enabled {
                all_tools.extend(lsp_tool_definitions());
            }
            if config.test_harness.enabled {
                all_tools.extend(test_harness_tool_definitions());
            }
            if config.repo_map.enabled {
                all_tools.extend(repo_map_tool_definitions());
            }
        }

        let lsp_manager = Arc::new(Mutex::new(LspServerManager::new(config.lsp.clone())));

        Self {
            inner,
            config,
            all_tools,
            sandbox,
            lsp_manager,
        }
    }

    /// Consume the decorator and return the inner executor for cleanup.
    pub fn into_inner(self) -> T {
        self.inner
    }
}

#[async_trait]
impl<T: ToolExecutor> ToolExecutor for DevToolsExecutor<T> {
    fn tool_definitions(&self) -> &[ToolDefinition] {
        &self.all_tools
    }

    async fn execute_tool(
        &self,
        name: &str,
        arguments: Value,
    ) -> Result<ToolExecutionResult, AgentError> {
        if self.config.enabled {
            // LSP tools
            if self.config.lsp.enabled {
                match name {
                    "lsp_goto_definition" => {
                        return super::lsp::goto_definition::execute_lsp_goto_definition(
                            arguments,
                            &self.lsp_manager,
                            &self.config.lsp,
                        )
                        .await;
                    }
                    "lsp_find_references" => {
                        return super::lsp::find_references::execute_lsp_find_references(
                            arguments,
                            &self.lsp_manager,
                            &self.config.lsp,
                        )
                        .await;
                    }
                    "lsp_diagnostics" => {
                        return super::lsp::diagnostics::execute_lsp_diagnostics(
                            arguments,
                            &self.lsp_manager,
                            &self.config.lsp,
                        )
                        .await;
                    }
                    _ => {}
                }
            }

            // Test harness tools
            if self.config.test_harness.enabled {
                match name {
                    "detect_test_framework" => {
                        return super::test_harness::detect::execute_detect_test_framework(
                            arguments,
                        )
                        .await;
                    }
                    "run_tests" => {
                        return super::test_harness::run::execute_run_tests(
                            arguments,
                            &self.config.test_harness,
                            self.sandbox.clone(),
                        )
                        .await;
                    }
                    "list_tests" => {
                        return super::test_harness::list::execute_list_tests(
                            arguments,
                            &self.config.test_harness,
                            self.sandbox.clone(),
                        )
                        .await;
                    }
                    _ => {}
                }
            }

            // Repo map tool
            if self.config.repo_map.enabled && name == "repo_map" {
                return super::repo_map::generator::execute_repo_map(
                    arguments,
                    &self.config.repo_map,
                )
                .await;
            }
        }

        self.inner.execute_tool(name, arguments).await
    }

    fn shutdown(self: Box<Self>) {
        // Shut down LSP servers synchronously
        let manager = Arc::try_unwrap(self.lsp_manager)
            .ok()
            .map(|m| m.into_inner());
        if let Some(manager) = manager {
            manager.shutdown_all();
        }
        Box::new(self.inner).shutdown();
    }

    async fn check_inbox(&self) -> Option<Vec<String>> {
        self.inner.check_inbox().await
    }
}

fn lsp_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "lsp_goto_definition".to_string(),
            description: "Jump to the definition of a symbol at a given file location. Returns the file path and position of the definition.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "Path to the source file" },
                    "line": { "type": "integer", "description": "Line number (0-indexed)" },
                    "column": { "type": "integer", "description": "Column number (0-indexed)" }
                },
                "required": ["file", "line", "column"]
            }),
            cache_control: None,
            read_only: true,
        },
        ToolDefinition {
            name: "lsp_find_references".to_string(),
            description: "Find all references to a symbol at a given file location. Returns a list of locations where the symbol is used.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "Path to the source file" },
                    "line": { "type": "integer", "description": "Line number (0-indexed)" },
                    "column": { "type": "integer", "description": "Column number (0-indexed)" },
                    "include_declaration": { "type": "boolean", "description": "Include the declaration in results (default: true)" }
                },
                "required": ["file", "line", "column"]
            }),
            cache_control: None,
            read_only: true,
        },
        ToolDefinition {
            name: "lsp_diagnostics".to_string(),
            description: "Get compiler errors and warnings for a file or the entire project.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "Path to the source file (optional, omit for project-wide diagnostics)" }
                }
            }),
            cache_control: None,
            read_only: true,
        },
    ]
}

fn test_harness_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "detect_test_framework".to_string(),
            description: "Auto-detect the test framework(s) used in the project by scanning for marker files (Cargo.toml, package.json, pyproject.toml, go.mod).".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Directory to scan (default: current working directory)" }
                }
            }),
            cache_control: None,
            read_only: true,
        },
        ToolDefinition {
            name: "run_tests".to_string(),
            description: "Run tests and return structured pass/fail results. Automatically detects the test framework if not specified.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "framework": { "type": "string", "description": "Test framework to use (cargo, pytest, jest, vitest, go). Auto-detected if omitted." },
                    "file": { "type": "string", "description": "Specific test file to run" },
                    "test_name": { "type": "string", "description": "Specific test name or pattern to run" },
                    "timeout_secs": { "type": "integer", "description": "Timeout in seconds (default: from config)" }
                }
            }),
            cache_control: None,
            read_only: false,
        },
        ToolDefinition {
            name: "list_tests".to_string(),
            description: "List available tests in the project without running them.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "framework": { "type": "string", "description": "Test framework to use (cargo, pytest, jest, vitest, go). Auto-detected if omitted." },
                    "file": { "type": "string", "description": "Specific test file to list tests from" }
                }
            }),
            cache_control: None,
            read_only: true,
        },
    ]
}

fn repo_map_tool_definitions() -> Vec<ToolDefinition> {
    vec![ToolDefinition {
        name: "repo_map".to_string(),
        description: "Generate a structural code map showing modules, functions, types, and their relationships. Useful for getting an architectural overview of a codebase.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Directory to map (default: current working directory)" },
                "depth": { "type": "integer", "description": "Maximum directory depth to traverse (default: from config)" },
                "include": { "type": "string", "description": "Glob pattern to filter files (e.g., '*.rs', '*.py')" },
                "symbols": { "type": "boolean", "description": "Include symbol details like function signatures (default: true)" }
            }
        }),
        cache_control: None,
        read_only: true,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sandbox rooted at the temp dir, for tests that only inspect tool wiring.
    fn test_sandbox() -> Arc<dyn BashSandbox> {
        Arc::from(
            crate::local_tools::sandbox::create_sandbox(std::env::temp_dir())
                .expect("temp dir sandbox"),
        )
    }
    use async_trait::async_trait;

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

    #[test]
    fn test_enabled_adds_all_tools() {
        let config = DevToolsConfig::default();
        let executor = DevToolsExecutor::new(mock_inner(), config, test_sandbox());
        // 1 inner + 3 LSP + 3 test harness + 1 repo map = 8
        assert_eq!(executor.tool_definitions().len(), 8);
        let names: Vec<&str> = executor
            .tool_definitions()
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert!(names.contains(&"navigate"));
        assert!(names.contains(&"lsp_goto_definition"));
        assert!(names.contains(&"lsp_find_references"));
        assert!(names.contains(&"lsp_diagnostics"));
        assert!(names.contains(&"detect_test_framework"));
        assert!(names.contains(&"run_tests"));
        assert!(names.contains(&"list_tests"));
        assert!(names.contains(&"repo_map"));
    }

    #[test]
    fn test_disabled_no_tools_added() {
        let config = DevToolsConfig {
            enabled: false,
            ..Default::default()
        };
        let executor = DevToolsExecutor::new(mock_inner(), config, test_sandbox());
        assert_eq!(executor.tool_definitions().len(), 1);
        assert_eq!(executor.tool_definitions()[0].name, "navigate");
    }

    #[test]
    fn test_selective_disable_lsp() {
        let config = DevToolsConfig {
            enabled: true,
            lsp: crate::config::schema::LspConfig {
                enabled: false,
                ..Default::default()
            },
            ..Default::default()
        };
        let executor = DevToolsExecutor::new(mock_inner(), config, test_sandbox());
        // 1 inner + 3 test harness + 1 repo map = 5
        assert_eq!(executor.tool_definitions().len(), 5);
        let names: Vec<&str> = executor
            .tool_definitions()
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert!(!names.contains(&"lsp_goto_definition"));
        assert!(names.contains(&"detect_test_framework"));
        assert!(names.contains(&"repo_map"));
    }

    #[test]
    fn test_selective_disable_test_harness() {
        let config = DevToolsConfig {
            enabled: true,
            test_harness: crate::config::schema::TestHarnessConfig {
                enabled: false,
                ..Default::default()
            },
            ..Default::default()
        };
        let executor = DevToolsExecutor::new(mock_inner(), config, test_sandbox());
        // 1 inner + 3 LSP + 1 repo map = 5
        assert_eq!(executor.tool_definitions().len(), 5);
        let names: Vec<&str> = executor
            .tool_definitions()
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert!(names.contains(&"lsp_goto_definition"));
        assert!(!names.contains(&"detect_test_framework"));
        assert!(names.contains(&"repo_map"));
    }

    #[test]
    fn test_selective_disable_repo_map() {
        let config = DevToolsConfig {
            enabled: true,
            repo_map: crate::config::schema::RepoMapConfig {
                enabled: false,
                ..Default::default()
            },
            ..Default::default()
        };
        let executor = DevToolsExecutor::new(mock_inner(), config, test_sandbox());
        // 1 inner + 3 LSP + 3 test harness = 7
        assert_eq!(executor.tool_definitions().len(), 7);
        let names: Vec<&str> = executor
            .tool_definitions()
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert!(names.contains(&"lsp_goto_definition"));
        assert!(names.contains(&"detect_test_framework"));
        assert!(!names.contains(&"repo_map"));
    }

    #[tokio::test]
    async fn test_unknown_tool_delegates_to_inner() {
        let config = DevToolsConfig::default();
        let executor = DevToolsExecutor::new(mock_inner(), config, test_sandbox());
        let result = executor
            .execute_tool("navigate", json!({"url": "test"}))
            .await
            .unwrap();
        assert_eq!(result.content, "inner executed: navigate");
    }

    #[tokio::test]
    async fn test_disabled_delegates_all() {
        let config = DevToolsConfig {
            enabled: false,
            ..Default::default()
        };
        let executor = DevToolsExecutor::new(mock_inner(), config, test_sandbox());
        let result = executor
            .execute_tool("lsp_goto_definition", json!({}))
            .await
            .unwrap();
        assert_eq!(result.content, "inner executed: lsp_goto_definition");
    }

    #[test]
    fn test_into_inner() {
        let config = DevToolsConfig::default();
        let executor = DevToolsExecutor::new(mock_inner(), config, test_sandbox());
        let recovered = executor.into_inner();
        assert_eq!(recovered.tool_definitions().len(), 1);
    }

    #[test]
    fn test_lsp_tools_are_read_only() {
        let defs = lsp_tool_definitions();
        for def in &defs {
            assert!(def.read_only, "LSP tool {} should be read_only", def.name);
        }
    }

    #[test]
    fn test_repo_map_is_read_only() {
        let defs = repo_map_tool_definitions();
        assert!(defs[0].read_only);
    }

    #[test]
    fn test_test_harness_read_only_flags() {
        let defs = test_harness_tool_definitions();
        let detect = defs
            .iter()
            .find(|d| d.name == "detect_test_framework")
            .unwrap();
        assert!(detect.read_only);
        let list = defs.iter().find(|d| d.name == "list_tests").unwrap();
        assert!(list.read_only);
        let run = defs.iter().find(|d| d.name == "run_tests").unwrap();
        assert!(!run.read_only);
    }
}
