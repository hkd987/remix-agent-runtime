use serde_json::Value;

use crate::browser::mcp::ToolExecutionResult;
use crate::error::AgentError;
use crate::local_tools::sandbox::PathValidator;

pub async fn execute_glob(
    args: Value,
    path_validator: &PathValidator,
) -> Result<ToolExecutionResult, AgentError> {
    let args: super::params::GlobArgs = super::params::parse("glob", args)?;
    let pattern = args.pattern.as_str();

    let base_path = match args.path.as_deref() {
        Some(p) => path_validator.resolve_path(p)?,
        None => path_validator.root().to_path_buf(),
    };

    // Build full glob pattern
    let full_pattern = base_path.join(pattern);
    let full_pattern_str = full_pattern.to_string_lossy();

    let paths = glob::glob(&full_pattern_str)
        .map_err(|e| AgentError::LocalTool(format!("Invalid glob pattern: {e}")))?;

    let mut results = Vec::new();
    let root = path_validator.root();

    for entry in paths {
        match entry {
            Ok(path) => {
                // Verify path is within sandbox
                if let Ok(canonical) = path.canonicalize() {
                    if canonical.starts_with(root) {
                        let relative = canonical.strip_prefix(root).unwrap_or(&canonical);
                        results.push(relative.display().to_string());
                    }
                }
            }
            Err(_) => continue,
        }
    }

    results.sort();

    if results.is_empty() {
        Ok(ToolExecutionResult {
            content: "No matching files found".to_string(),
            is_error: false,
        })
    } else {
        Ok(ToolExecutionResult {
            content: results.join("\n"),
            is_error: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use tempfile::TempDir;

    fn make_validator(tmp: &TempDir) -> PathValidator {
        PathValidator::new(tmp.path().to_path_buf()).unwrap()
    }

    #[tokio::test]
    async fn test_glob_basic_pattern() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("hello.txt"), "content").unwrap();
        fs::write(tmp.path().join("world.txt"), "content").unwrap();
        fs::write(tmp.path().join("data.rs"), "content").unwrap();
        let validator = make_validator(&tmp);

        let result = execute_glob(json!({"pattern": "*.txt"}), &validator)
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("hello.txt"));
        assert!(result.content.contains("world.txt"));
        assert!(!result.content.contains("data.rs"));
    }

    #[tokio::test]
    async fn test_glob_recursive() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("sub/deep")).unwrap();
        fs::write(tmp.path().join("top.rs"), "content").unwrap();
        fs::write(tmp.path().join("sub/mid.rs"), "content").unwrap();
        fs::write(tmp.path().join("sub/deep/bottom.rs"), "content").unwrap();
        let validator = make_validator(&tmp);

        let result = execute_glob(json!({"pattern": "**/*.rs"}), &validator)
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("top.rs"));
        assert!(result.content.contains("mid.rs"));
        assert!(result.content.contains("bottom.rs"));
    }

    #[tokio::test]
    async fn test_glob_no_matches() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("file.txt"), "content").unwrap();
        let validator = make_validator(&tmp);

        let result = execute_glob(json!({"pattern": "*.nonexistent"}), &validator)
            .await
            .unwrap();
        assert!(!result.is_error);
        assert_eq!(result.content, "No matching files found");
    }

    #[tokio::test]
    async fn test_glob_with_base_path() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("subdir")).unwrap();
        fs::write(tmp.path().join("subdir/file.txt"), "content").unwrap();
        fs::write(tmp.path().join("root.txt"), "content").unwrap();
        let validator = make_validator(&tmp);

        let result = execute_glob(json!({"pattern": "*.txt", "path": "subdir"}), &validator)
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("file.txt"));
        assert!(!result.content.contains("root.txt"));
    }

    #[tokio::test]
    async fn test_glob_missing_pattern_param() {
        let tmp = TempDir::new().unwrap();
        let validator = make_validator(&tmp);

        let result = execute_glob(json!({}), &validator).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("missing field `pattern`"));
    }

    #[tokio::test]
    async fn test_glob_sorts_results() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("charlie.txt"), "c").unwrap();
        fs::write(tmp.path().join("alpha.txt"), "a").unwrap();
        fs::write(tmp.path().join("bravo.txt"), "b").unwrap();
        let validator = make_validator(&tmp);

        let result = execute_glob(json!({"pattern": "*.txt"}), &validator)
            .await
            .unwrap();
        assert!(!result.is_error);
        let lines: Vec<&str> = result.content.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("alpha.txt"));
        assert!(lines[1].contains("bravo.txt"));
        assert!(lines[2].contains("charlie.txt"));
    }

    #[tokio::test]
    async fn test_glob_relative_paths() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("rel.txt"), "content").unwrap();
        let validator = make_validator(&tmp);

        let result = execute_glob(json!({"pattern": "*.txt"}), &validator)
            .await
            .unwrap();
        assert!(!result.is_error);
        // Should be relative path (just filename), not absolute
        assert!(!result.content.starts_with('/'));
        assert!(result.content.contains("rel.txt"));
    }

    #[tokio::test]
    async fn test_glob_nested_directories() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("a/b")).unwrap();
        fs::create_dir_all(tmp.path().join("c")).unwrap();
        fs::write(tmp.path().join("a/one.txt"), "1").unwrap();
        fs::write(tmp.path().join("a/b/two.txt"), "2").unwrap();
        fs::write(tmp.path().join("c/three.txt"), "3").unwrap();
        let validator = make_validator(&tmp);

        let result = execute_glob(json!({"pattern": "**/*.txt"}), &validator)
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("one.txt"));
        assert!(result.content.contains("two.txt"));
        assert!(result.content.contains("three.txt"));
    }

    #[tokio::test]
    async fn test_glob_multiple_extensions() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("code.rs"), "rust").unwrap();
        fs::write(tmp.path().join("data.json"), "json").unwrap();
        fs::write(tmp.path().join("notes.txt"), "text").unwrap();
        let validator = make_validator(&tmp);

        let result_rs = execute_glob(json!({"pattern": "*.rs"}), &validator)
            .await
            .unwrap();
        assert!(result_rs.content.contains("code.rs"));
        assert!(!result_rs.content.contains("data.json"));
        assert!(!result_rs.content.contains("notes.txt"));

        let result_json = execute_glob(json!({"pattern": "*.json"}), &validator)
            .await
            .unwrap();
        assert!(result_json.content.contains("data.json"));
        assert!(!result_json.content.contains("code.rs"));
    }
}
