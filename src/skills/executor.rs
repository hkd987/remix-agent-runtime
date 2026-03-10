use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

use crate::browser::mcp::{ToolExecutionResult, ToolExecutor};
use crate::error::AgentError;
use crate::llm::types::ToolDefinition;

use super::types::SkillSet;

/// Decorator that wraps a ToolExecutor and adds virtual skill tools.
///
/// When skills are discovered, three virtual tools are added:
/// - `load_skill`: Load full SKILL.md instructions for a skill
/// - `run_skill_script`: Execute a script from a skill's scripts/ directory
/// - `read_skill_resource`: Read a resource file from a skill directory
///
/// When no skills are discovered, zero overhead — no virtual tools added.
pub struct SkillAwareExecutor<T: ToolExecutor> {
    inner: T,
    skill_set: SkillSet,
    all_tools: Vec<ToolDefinition>,
    script_timeout_secs: u64,
}

impl<T: ToolExecutor> SkillAwareExecutor<T> {
    pub fn new(inner: T, skill_set: SkillSet, script_timeout_secs: u64) -> Self {
        let mut all_tools: Vec<ToolDefinition> = inner.tool_definitions().to_vec();

        // Only add virtual tools if there are skills to manage
        if !skill_set.is_empty() {
            all_tools.extend(virtual_tool_definitions());
        }

        Self {
            inner,
            skill_set,
            all_tools,
            script_timeout_secs,
        }
    }

    /// Consume the decorator and return the inner executor for cleanup.
    pub fn into_inner(self) -> T {
        self.inner
    }

    async fn handle_load_skill(&self, arguments: Value) -> Result<ToolExecutionResult, AgentError> {
        let name = arguments
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                AgentError::Skill("load_skill requires a 'name' parameter".to_string())
            })?;

        let entry = self
            .skill_set
            .get(name)
            .ok_or_else(|| AgentError::Skill(format!("Skill '{}' not found", name)))?;

        let mut output = String::new();
        output.push_str(&entry.body);
        output.push_str("\n\n---\n");

        // List available scripts
        let scripts_dir = entry.dir_path.join("scripts");
        if scripts_dir.is_dir() {
            let scripts = list_dir_entries(&scripts_dir);
            if !scripts.is_empty() {
                output.push_str("\nAvailable scripts:\n");
                for script in &scripts {
                    output.push_str(&format!("- {script}\n"));
                }
            }
        }

        // List available resources (everything except SKILL.md and scripts/)
        let resources = list_skill_resources(&entry.dir_path);
        if !resources.is_empty() {
            output.push_str("\nAvailable resources:\n");
            for resource in &resources {
                output.push_str(&format!("- {resource}\n"));
            }
        }

        Ok(ToolExecutionResult {
            content: output,
            is_error: false,
        })
    }

    async fn handle_run_skill_script(
        &self,
        arguments: Value,
    ) -> Result<ToolExecutionResult, AgentError> {
        let skill_name = arguments
            .get("skill_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                AgentError::Skill("run_skill_script requires a 'skill_name' parameter".to_string())
            })?;

        let script = arguments
            .get("script")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                AgentError::Skill("run_skill_script requires a 'script' parameter".to_string())
            })?;

        let args: Vec<String> = arguments
            .get("args")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let entry = self
            .skill_set
            .get(skill_name)
            .ok_or_else(|| AgentError::Skill(format!("Skill '{}' not found", skill_name)))?;

        let scripts_dir = entry.dir_path.join("scripts");
        let script_path = validate_path_within_skill(&scripts_dir, script)?;

        if !script_path.is_file() {
            return Err(AgentError::Skill(format!(
                "Script '{}' not found in skill '{}'",
                script, skill_name
            )));
        }

        let timeout = Duration::from_secs(self.script_timeout_secs);

        let mut cmd = tokio::process::Command::new(&script_path);
        cmd.args(&args)
            .current_dir(&entry.dir_path)
            .kill_on_drop(true);

        let output = tokio::time::timeout(timeout, cmd.output())
            .await
            .map_err(|_| {
                AgentError::Skill(format!(
                    "Script '{}' timed out after {}s",
                    script, self.script_timeout_secs
                ))
            })?
            .map_err(|e| {
                AgentError::Skill(format!("Failed to execute script '{}': {e}", script))
            })?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        let mut result_text = String::new();
        if !stdout.is_empty() {
            result_text.push_str(&stdout);
        }
        if !stderr.is_empty() {
            if !result_text.is_empty() {
                result_text.push('\n');
            }
            result_text.push_str("[stderr] ");
            result_text.push_str(&stderr);
        }

        if output.status.success() {
            Ok(ToolExecutionResult {
                content: result_text,
                is_error: false,
            })
        } else {
            Ok(ToolExecutionResult {
                content: format!(
                    "Script exited with code {}\n{}",
                    output.status.code().unwrap_or(-1),
                    result_text
                ),
                is_error: true,
            })
        }
    }

    async fn handle_read_skill_resource(
        &self,
        arguments: Value,
    ) -> Result<ToolExecutionResult, AgentError> {
        let skill_name = arguments
            .get("skill_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                AgentError::Skill(
                    "read_skill_resource requires a 'skill_name' parameter".to_string(),
                )
            })?;

        let path = arguments
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                AgentError::Skill("read_skill_resource requires a 'path' parameter".to_string())
            })?;

        let entry = self
            .skill_set
            .get(skill_name)
            .ok_or_else(|| AgentError::Skill(format!("Skill '{}' not found", skill_name)))?;

        let file_path = validate_path_within_skill(&entry.dir_path, path)?;

        if !file_path.is_file() {
            return Err(AgentError::Skill(format!(
                "Resource '{}' not found in skill '{}'",
                path, skill_name
            )));
        }

        // Check file size (max 1MB)
        let file_meta = std::fs::metadata(&file_path)
            .map_err(|e| AgentError::Skill(format!("Failed to read resource metadata: {e}")))?;
        if file_meta.len() > 1_048_576 {
            return Err(AgentError::Skill(format!(
                "Resource '{}' too large ({} bytes, max 1MB)",
                path,
                file_meta.len()
            )));
        }

        let content = std::fs::read_to_string(&file_path)
            .map_err(|e| AgentError::Skill(format!("Failed to read resource '{}': {e}", path)))?;

        Ok(ToolExecutionResult {
            content,
            is_error: false,
        })
    }
}

#[async_trait]
impl<T: ToolExecutor> ToolExecutor for SkillAwareExecutor<T> {
    fn tool_definitions(&self) -> &[ToolDefinition] {
        &self.all_tools
    }

    async fn execute_tool(
        &self,
        name: &str,
        arguments: Value,
    ) -> Result<ToolExecutionResult, AgentError> {
        match name {
            "load_skill" if !self.skill_set.is_empty() => self.handle_load_skill(arguments).await,
            "run_skill_script" if !self.skill_set.is_empty() => {
                self.handle_run_skill_script(arguments).await
            }
            "read_skill_resource" if !self.skill_set.is_empty() => {
                self.handle_read_skill_resource(arguments).await
            }
            _ => self.inner.execute_tool(name, arguments).await,
        }
    }
}

/// Validate that a relative path resolves within a skill directory.
/// Uses `canonicalize()` to resolve symlinks and `..` components.
fn validate_path_within_skill(
    skill_dir: &Path,
    relative_path: &str,
) -> Result<std::path::PathBuf, AgentError> {
    let candidate = skill_dir.join(relative_path);
    let canonical = std::fs::canonicalize(&candidate)
        .map_err(|e| AgentError::Skill(format!("Path not found: {e}")))?;
    let skill_canonical = std::fs::canonicalize(skill_dir)
        .map_err(|e| AgentError::Skill(format!("Skill dir error: {e}")))?;
    if !canonical.starts_with(&skill_canonical) {
        return Err(AgentError::Skill("Path traversal detected".to_string()));
    }
    Ok(canonical)
}

/// List filenames in a directory (non-recursive).
fn list_dir_entries(dir: &Path) -> Vec<String> {
    let mut entries = Vec::new();
    if let Ok(read_dir) = std::fs::read_dir(dir) {
        for entry in read_dir.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                entries.push(name.to_string());
            }
        }
    }
    entries.sort();
    entries
}

/// List resource entries in a skill directory (excluding SKILL.md and scripts/).
fn list_skill_resources(skill_dir: &Path) -> Vec<String> {
    let mut resources = Vec::new();
    if let Ok(read_dir) = std::fs::read_dir(skill_dir) {
        for entry in read_dir.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if name != "SKILL.md" && name != "scripts" {
                    resources.push(name.to_string());
                }
            }
        }
    }
    resources.sort();
    resources
}

/// Generate the three virtual tool definitions for skill management.
fn virtual_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "load_skill".to_string(),
            description: "Load full instructions for a skill. Returns the complete SKILL.md body and lists available scripts and resources.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "The name of the skill to load"
                    }
                },
                "required": ["name"]
            }),
            cache_control: None,
            read_only: false,
        },
        ToolDefinition {
            name: "run_skill_script".to_string(),
            description: "Execute a script from a skill's scripts/ directory. Returns stdout/stderr output.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "skill_name": {
                        "type": "string",
                        "description": "The name of the skill containing the script"
                    },
                    "script": {
                        "type": "string",
                        "description": "The script filename to execute (relative to scripts/ dir)"
                    },
                    "args": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional command-line arguments for the script"
                    }
                },
                "required": ["skill_name", "script"]
            }),
            cache_control: None,
            read_only: false,
        },
        ToolDefinition {
            name: "read_skill_resource".to_string(),
            description: "Read a file from a skill's directory. Can read references, assets, or any file within the skill directory.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "skill_name": {
                        "type": "string",
                        "description": "The name of the skill containing the resource"
                    },
                    "path": {
                        "type": "string",
                        "description": "Path to the file relative to the skill directory"
                    }
                },
                "required": ["skill_name", "path"]
            }),
            cache_control: None,
            read_only: false,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::types::{SkillEntry, SkillMetadata};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
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

    fn make_test_skill(dir: &TempDir) -> (SkillSet, std::path::PathBuf) {
        let skill_dir = dir.path().join("test-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: test-skill\ndescription: A test\n---\n# Instructions\nDo things.",
        )
        .unwrap();

        let mut set = SkillSet::new();
        let canonical_dir = std::fs::canonicalize(&skill_dir).unwrap();
        set.insert(SkillEntry {
            metadata: SkillMetadata {
                name: "test-skill".to_string(),
                description: "A test".to_string(),
                license: None,
                compatibility: vec![],
                metadata: Default::default(),
                allowed_tools: vec![],
            },
            body: "# Instructions\nDo things.".to_string(),
            dir_path: canonical_dir.clone(),
            skill_md_path: canonical_dir.join("SKILL.md"),
        })
        .unwrap();

        (set, skill_dir)
    }

    #[test]
    fn test_empty_skill_set_no_virtual_tools() {
        let inner = mock_inner();
        let executor = SkillAwareExecutor::new(inner, SkillSet::new(), 60);
        assert_eq!(executor.tool_definitions().len(), 1);
        assert_eq!(executor.tool_definitions()[0].name, "navigate");
    }

    #[test]
    fn test_with_skills_adds_virtual_tools() {
        let dir = TempDir::new().unwrap();
        let (skill_set, _) = make_test_skill(&dir);
        let inner = mock_inner();
        let executor = SkillAwareExecutor::new(inner, skill_set, 60);
        // 1 inner + 3 virtual
        assert_eq!(executor.tool_definitions().len(), 4);
        let names: Vec<&str> = executor
            .tool_definitions()
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert!(names.contains(&"navigate"));
        assert!(names.contains(&"load_skill"));
        assert!(names.contains(&"run_skill_script"));
        assert!(names.contains(&"read_skill_resource"));
    }

    #[tokio::test]
    async fn test_unknown_tool_delegates_to_inner() {
        let dir = TempDir::new().unwrap();
        let (skill_set, _) = make_test_skill(&dir);
        let inner = mock_inner();
        let executor = SkillAwareExecutor::new(inner, skill_set, 60);

        let result = executor
            .execute_tool("navigate", json!({"url": "test"}))
            .await
            .unwrap();
        assert_eq!(result.content, "inner executed: navigate");
    }

    #[tokio::test]
    async fn test_load_skill_success() {
        let dir = TempDir::new().unwrap();
        let (skill_set, _) = make_test_skill(&dir);
        let inner = mock_inner();
        let executor = SkillAwareExecutor::new(inner, skill_set, 60);

        let result = executor
            .execute_tool("load_skill", json!({"name": "test-skill"}))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("# Instructions"));
        assert!(result.content.contains("Do things."));
    }

    #[tokio::test]
    async fn test_load_skill_not_found() {
        let dir = TempDir::new().unwrap();
        let (skill_set, _) = make_test_skill(&dir);
        let inner = mock_inner();
        let executor = SkillAwareExecutor::new(inner, skill_set, 60);

        let result = executor
            .execute_tool("load_skill", json!({"name": "nonexistent"}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_load_skill_lists_scripts() {
        let dir = TempDir::new().unwrap();
        let (skill_set, skill_dir) = make_test_skill(&dir);

        // Add a scripts directory with a script
        let scripts_dir = skill_dir.join("scripts");
        fs::create_dir(&scripts_dir).unwrap();
        fs::write(scripts_dir.join("setup.sh"), "#!/bin/bash\necho hello").unwrap();

        let inner = mock_inner();
        let executor = SkillAwareExecutor::new(inner, skill_set, 60);

        let result = executor
            .execute_tool("load_skill", json!({"name": "test-skill"}))
            .await
            .unwrap();
        assert!(result.content.contains("Available scripts:"));
        assert!(result.content.contains("setup.sh"));
    }

    #[tokio::test]
    async fn test_load_skill_lists_resources() {
        let dir = TempDir::new().unwrap();
        let (skill_set, skill_dir) = make_test_skill(&dir);

        // Add a resource file
        fs::write(skill_dir.join("reference.md"), "# Reference\nSome docs.").unwrap();

        let inner = mock_inner();
        let executor = SkillAwareExecutor::new(inner, skill_set, 60);

        let result = executor
            .execute_tool("load_skill", json!({"name": "test-skill"}))
            .await
            .unwrap();
        assert!(result.content.contains("Available resources:"));
        assert!(result.content.contains("reference.md"));
    }

    #[tokio::test]
    async fn test_read_skill_resource_success() {
        let dir = TempDir::new().unwrap();
        let (skill_set, skill_dir) = make_test_skill(&dir);
        fs::write(skill_dir.join("data.txt"), "Hello from resource").unwrap();

        let inner = mock_inner();
        let executor = SkillAwareExecutor::new(inner, skill_set, 60);

        let result = executor
            .execute_tool(
                "read_skill_resource",
                json!({"skill_name": "test-skill", "path": "data.txt"}),
            )
            .await
            .unwrap();
        assert_eq!(result.content, "Hello from resource");
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_read_skill_resource_path_traversal() {
        let dir = TempDir::new().unwrap();
        let (skill_set, _) = make_test_skill(&dir);

        // Create a file outside the skill dir
        fs::write(dir.path().join("secret.txt"), "secret data").unwrap();

        let inner = mock_inner();
        let executor = SkillAwareExecutor::new(inner, skill_set, 60);

        let result = executor
            .execute_tool(
                "read_skill_resource",
                json!({"skill_name": "test-skill", "path": "../secret.txt"}),
            )
            .await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Path traversal detected"));
    }

    #[tokio::test]
    async fn test_read_skill_resource_not_found() {
        let dir = TempDir::new().unwrap();
        let (skill_set, _) = make_test_skill(&dir);

        let inner = mock_inner();
        let executor = SkillAwareExecutor::new(inner, skill_set, 60);

        let result = executor
            .execute_tool(
                "read_skill_resource",
                json!({"skill_name": "test-skill", "path": "nonexistent.txt"}),
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Path not found"));
    }

    #[tokio::test]
    async fn test_run_skill_script_success() {
        let dir = TempDir::new().unwrap();
        let (skill_set, skill_dir) = make_test_skill(&dir);

        let scripts_dir = skill_dir.join("scripts");
        fs::create_dir(&scripts_dir).unwrap();
        let script_path = scripts_dir.join("hello.sh");
        fs::write(&script_path, "#!/bin/bash\necho 'Hello from script'").unwrap();
        fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();

        let inner = mock_inner();
        let executor = SkillAwareExecutor::new(inner, skill_set, 60);

        let result = executor
            .execute_tool(
                "run_skill_script",
                json!({"skill_name": "test-skill", "script": "hello.sh"}),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("Hello from script"));
    }

    #[tokio::test]
    async fn test_run_skill_script_with_args() {
        let dir = TempDir::new().unwrap();
        let (skill_set, skill_dir) = make_test_skill(&dir);

        let scripts_dir = skill_dir.join("scripts");
        fs::create_dir(&scripts_dir).unwrap();
        let script_path = scripts_dir.join("echo-args.sh");
        fs::write(&script_path, "#!/bin/bash\necho \"args: $@\"").unwrap();
        fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();

        let inner = mock_inner();
        let executor = SkillAwareExecutor::new(inner, skill_set, 60);

        let result = executor
            .execute_tool(
                "run_skill_script",
                json!({"skill_name": "test-skill", "script": "echo-args.sh", "args": ["foo", "bar"]}),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("args: foo bar"));
    }

    #[tokio::test]
    async fn test_run_skill_script_path_traversal() {
        let dir = TempDir::new().unwrap();
        let (skill_set, skill_dir) = make_test_skill(&dir);

        // Create scripts dir so canonicalize works
        let scripts_dir = skill_dir.join("scripts");
        fs::create_dir(&scripts_dir).unwrap();

        let inner = mock_inner();
        let executor = SkillAwareExecutor::new(inner, skill_set, 60);

        let result = executor
            .execute_tool(
                "run_skill_script",
                json!({"skill_name": "test-skill", "script": "../../etc/passwd"}),
            )
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_run_skill_script_timeout() {
        let dir = TempDir::new().unwrap();
        let (skill_set, skill_dir) = make_test_skill(&dir);

        let scripts_dir = skill_dir.join("scripts");
        fs::create_dir(&scripts_dir).unwrap();
        let script_path = scripts_dir.join("slow.sh");
        fs::write(&script_path, "#!/bin/bash\nsleep 30").unwrap();
        fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();

        let inner = mock_inner();
        // 1 second timeout
        let executor = SkillAwareExecutor::new(inner, skill_set, 1);

        let result = executor
            .execute_tool(
                "run_skill_script",
                json!({"skill_name": "test-skill", "script": "slow.sh"}),
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("timed out"));
    }

    #[tokio::test]
    async fn test_run_skill_script_nonzero_exit() {
        let dir = TempDir::new().unwrap();
        let (skill_set, skill_dir) = make_test_skill(&dir);

        let scripts_dir = skill_dir.join("scripts");
        fs::create_dir(&scripts_dir).unwrap();
        let script_path = scripts_dir.join("fail.sh");
        fs::write(&script_path, "#!/bin/bash\necho 'error msg' >&2\nexit 1").unwrap();
        fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();

        let inner = mock_inner();
        let executor = SkillAwareExecutor::new(inner, skill_set, 60);

        let result = executor
            .execute_tool(
                "run_skill_script",
                json!({"skill_name": "test-skill", "script": "fail.sh"}),
            )
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("exited with code 1"));
    }

    #[tokio::test]
    async fn test_into_inner() {
        let inner = mock_inner();
        let executor = SkillAwareExecutor::new(inner, SkillSet::new(), 60);
        let recovered = executor.into_inner();
        assert_eq!(recovered.tool_definitions().len(), 1);
    }

    #[test]
    fn test_validate_path_within_skill_valid() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "content").unwrap();

        let result = validate_path_within_skill(dir.path(), "test.txt");
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_path_within_skill_traversal() {
        let dir = TempDir::new().unwrap();
        let parent_file = dir.path().parent().unwrap().join("outside.txt");
        // Create the parent file so canonicalize can resolve it
        if fs::write(&parent_file, "outside").is_ok() {
            let result = validate_path_within_skill(dir.path(), "../outside.txt");
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("Path traversal detected"));
            let _ = fs::remove_file(&parent_file);
        }
    }

    #[test]
    fn test_validate_path_within_skill_nonexistent() {
        let dir = TempDir::new().unwrap();
        let result = validate_path_within_skill(dir.path(), "does-not-exist.txt");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Path not found"));
    }

    #[test]
    fn test_virtual_tool_definitions_count() {
        let defs = virtual_tool_definitions();
        assert_eq!(defs.len(), 3);
        assert_eq!(defs[0].name, "load_skill");
        assert_eq!(defs[1].name, "run_skill_script");
        assert_eq!(defs[2].name, "read_skill_resource");
    }

    #[test]
    fn test_virtual_tool_definitions_have_schemas() {
        for def in virtual_tool_definitions() {
            assert!(def.input_schema.get("type").is_some());
            assert!(def.input_schema.get("properties").is_some());
            assert!(def.input_schema.get("required").is_some());
        }
    }

    #[tokio::test]
    async fn test_empty_skillset_delegates_skill_tool_names() {
        // When skill_set is empty, even tool names matching virtual tools
        // should delegate to inner
        let inner = mock_inner();
        let executor = SkillAwareExecutor::new(inner, SkillSet::new(), 60);

        let result = executor
            .execute_tool("load_skill", json!({"name": "test"}))
            .await
            .unwrap();
        // Should delegate to inner since skill_set is empty
        assert_eq!(result.content, "inner executed: load_skill");
    }

    #[tokio::test]
    async fn test_read_skill_resource_too_large() {
        let dir = TempDir::new().unwrap();
        let (skill_set, skill_dir) = make_test_skill(&dir);

        // Create a file larger than 1MB
        let large_content = "x".repeat(1_048_577);
        fs::write(skill_dir.join("large.txt"), &large_content).unwrap();

        let inner = mock_inner();
        let executor = SkillAwareExecutor::new(inner, skill_set, 60);

        let result = executor
            .execute_tool(
                "read_skill_resource",
                json!({"skill_name": "test-skill", "path": "large.txt"}),
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("too large"));
    }
}
