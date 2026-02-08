use serde_json::Value;

use crate::browser::mcp::ToolExecutionResult;
use crate::error::AgentError;
use crate::local_tools::sandbox::PathValidator;

pub async fn execute_edit_file(
    args: Value,
    path_validator: &PathValidator,
) -> Result<ToolExecutionResult, AgentError> {
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AgentError::LocalTool("edit_file requires 'path' parameter".to_string()))?;

    let old_string = args
        .get("old_string")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            AgentError::LocalTool("edit_file requires 'old_string' parameter".to_string())
        })?;

    let new_string = args
        .get("new_string")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            AgentError::LocalTool("edit_file requires 'new_string' parameter".to_string())
        })?;

    let resolved = path_validator.resolve_path(path)?;

    let content = std::fs::read_to_string(&resolved)
        .map_err(|e| AgentError::LocalTool(format!("Failed to read file: {e}")))?;

    let count = content.matches(old_string).count();

    if count == 0 {
        return Err(AgentError::LocalTool(format!(
            "old_string not found in {}",
            path
        )));
    }

    if count > 1 {
        return Err(AgentError::LocalTool(format!(
            "old_string found {} times in {} (must be unique)",
            count, path
        )));
    }

    let new_content = content.replacen(old_string, new_string, 1);

    std::fs::write(&resolved, &new_content)
        .map_err(|e| AgentError::LocalTool(format!("Failed to write file: {e}")))?;

    Ok(ToolExecutionResult {
        content: format!("Successfully edited {}", path),
        is_error: false,
    })
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
    async fn test_edit_basic_replacement() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("test.txt"), "hello world").unwrap();
        let validator = make_validator(&tmp);

        let result = execute_edit_file(
            json!({"path": "test.txt", "old_string": "hello", "new_string": "goodbye"}),
            &validator,
        )
        .await
        .unwrap();

        assert!(result.content.contains("Successfully edited"));
        assert!(!result.is_error);

        let updated = fs::read_to_string(tmp.path().join("test.txt")).unwrap();
        assert_eq!(updated, "goodbye world");
    }

    #[tokio::test]
    async fn test_edit_not_found() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("test.txt"), "hello world").unwrap();
        let validator = make_validator(&tmp);

        let result = execute_edit_file(
            json!({"path": "test.txt", "old_string": "xyz", "new_string": "abc"}),
            &validator,
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("old_string not found"));
    }

    #[tokio::test]
    async fn test_edit_not_unique() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("test.txt"), "aaa bbb aaa").unwrap();
        let validator = make_validator(&tmp);

        let result = execute_edit_file(
            json!({"path": "test.txt", "old_string": "aaa", "new_string": "ccc"}),
            &validator,
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("found 2 times"));
        assert!(err.contains("must be unique"));
    }

    #[tokio::test]
    async fn test_edit_outside_sandbox() {
        let tmp = TempDir::new().unwrap();
        let validator = make_validator(&tmp);

        let result = execute_edit_file(
            json!({"path": "/etc/passwd", "old_string": "root", "new_string": "toor"}),
            &validator,
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("outside sandbox"));
    }

    #[tokio::test]
    async fn test_edit_missing_path_param() {
        let tmp = TempDir::new().unwrap();
        let validator = make_validator(&tmp);

        let result =
            execute_edit_file(json!({"old_string": "a", "new_string": "b"}), &validator).await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("requires 'path' parameter"));
    }

    #[tokio::test]
    async fn test_edit_missing_old_string_param() {
        let tmp = TempDir::new().unwrap();
        let validator = make_validator(&tmp);

        let result =
            execute_edit_file(json!({"path": "test.txt", "new_string": "b"}), &validator).await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("requires 'old_string' parameter"));
    }

    #[tokio::test]
    async fn test_edit_missing_new_string_param() {
        let tmp = TempDir::new().unwrap();
        let validator = make_validator(&tmp);

        let result =
            execute_edit_file(json!({"path": "test.txt", "old_string": "a"}), &validator).await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("requires 'new_string' parameter"));
    }

    #[tokio::test]
    async fn test_edit_nonexistent_file() {
        let tmp = TempDir::new().unwrap();
        let validator = make_validator(&tmp);

        let result = execute_edit_file(
            json!({"path": "nope.txt", "old_string": "a", "new_string": "b"}),
            &validator,
        )
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_edit_multiline_replacement() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("multi.txt"), "line1\nline2\nline3").unwrap();
        let validator = make_validator(&tmp);

        let result = execute_edit_file(
            json!({"path": "multi.txt", "old_string": "line1\nline2", "new_string": "replaced"}),
            &validator,
        )
        .await
        .unwrap();

        assert!(result.content.contains("Successfully edited"));
        let updated = fs::read_to_string(tmp.path().join("multi.txt")).unwrap();
        assert_eq!(updated, "replaced\nline3");
    }

    #[tokio::test]
    async fn test_edit_preserves_other_content() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("preserve.txt"), "aaa bbb ccc").unwrap();
        let validator = make_validator(&tmp);

        execute_edit_file(
            json!({"path": "preserve.txt", "old_string": "bbb", "new_string": "XXX"}),
            &validator,
        )
        .await
        .unwrap();

        let updated = fs::read_to_string(tmp.path().join("preserve.txt")).unwrap();
        assert_eq!(updated, "aaa XXX ccc");
    }

    #[tokio::test]
    async fn test_edit_file_content_after_edit() {
        let tmp = TempDir::new().unwrap();
        let original = "fn main() {\n    println!(\"Hello\");\n}";
        fs::write(tmp.path().join("code.rs"), original).unwrap();
        let validator = make_validator(&tmp);

        execute_edit_file(
            json!({
                "path": "code.rs",
                "old_string": "println!(\"Hello\")",
                "new_string": "println!(\"Goodbye\")"
            }),
            &validator,
        )
        .await
        .unwrap();

        let updated = fs::read_to_string(tmp.path().join("code.rs")).unwrap();
        assert_eq!(updated, "fn main() {\n    println!(\"Goodbye\");\n}");
    }
}
