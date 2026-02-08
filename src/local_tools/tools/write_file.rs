use serde_json::Value;

use crate::browser::mcp::ToolExecutionResult;
use crate::error::AgentError;
use crate::local_tools::sandbox::PathValidator;

pub async fn execute_write_file(
    args: Value,
    path_validator: &PathValidator,
    max_bytes: usize,
) -> Result<ToolExecutionResult, AgentError> {
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AgentError::LocalTool("write_file requires 'path' parameter".to_string()))?;

    let content = args
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            AgentError::LocalTool("write_file requires 'content' parameter".to_string())
        })?;

    if content.len() > max_bytes {
        return Err(AgentError::LocalTool(format!(
            "Content too large ({} bytes, max {})",
            content.len(),
            max_bytes
        )));
    }

    let resolved = path_validator.resolve_path(path)?;

    // Create parent dirs if needed
    if let Some(parent) = resolved.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AgentError::LocalTool(format!("Failed to create directories: {e}")))?;
    }

    std::fs::write(&resolved, content)
        .map_err(|e| AgentError::LocalTool(format!("Failed to write file: {e}")))?;

    Ok(ToolExecutionResult {
        content: format!("Successfully wrote {} bytes to {}", content.len(), path),
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
    async fn test_write_basic_file() {
        let tmp = TempDir::new().unwrap();
        let validator = make_validator(&tmp);

        let result = execute_write_file(
            json!({"path": "test.txt", "content": "hello"}),
            &validator,
            10_485_760,
        )
        .await
        .unwrap();

        assert!(result.content.contains("Successfully wrote"));
        assert!(result.content.contains("5 bytes"));
        assert!(!result.is_error);

        let written = fs::read_to_string(tmp.path().join("test.txt")).unwrap();
        assert_eq!(written, "hello");
    }

    #[tokio::test]
    async fn test_write_creates_parent_dirs() {
        let tmp = TempDir::new().unwrap();
        // Create the subdir first so PathValidator can resolve the parent
        fs::create_dir_all(tmp.path().join("sub")).unwrap();
        let validator = make_validator(&tmp);

        let result = execute_write_file(
            json!({"path": "sub/nested.txt", "content": "deep"}),
            &validator,
            10_485_760,
        )
        .await
        .unwrap();

        assert!(result.content.contains("Successfully wrote"));
        let written = fs::read_to_string(tmp.path().join("sub/nested.txt")).unwrap();
        assert_eq!(written, "deep");
    }

    #[tokio::test]
    async fn test_write_too_large() {
        let tmp = TempDir::new().unwrap();
        let validator = make_validator(&tmp);

        let result = execute_write_file(
            json!({"path": "big.txt", "content": "x".repeat(200)}),
            &validator,
            100,
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Content too large"));
    }

    #[tokio::test]
    async fn test_write_outside_sandbox() {
        let tmp = TempDir::new().unwrap();
        let validator = make_validator(&tmp);

        let result = execute_write_file(
            json!({"path": "/etc/evil.txt", "content": "pwned"}),
            &validator,
            10_485_760,
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("outside sandbox"));
    }

    #[tokio::test]
    async fn test_write_missing_path_param() {
        let tmp = TempDir::new().unwrap();
        let validator = make_validator(&tmp);

        let result = execute_write_file(json!({"content": "hello"}), &validator, 10_485_760).await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("requires 'path' parameter"));
    }

    #[tokio::test]
    async fn test_write_missing_content_param() {
        let tmp = TempDir::new().unwrap();
        let validator = make_validator(&tmp);

        let result = execute_write_file(json!({"path": "test.txt"}), &validator, 10_485_760).await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("requires 'content' parameter"));
    }

    #[tokio::test]
    async fn test_write_overwrites_existing() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("exist.txt"), "old content").unwrap();
        let validator = make_validator(&tmp);

        execute_write_file(
            json!({"path": "exist.txt", "content": "new content"}),
            &validator,
            10_485_760,
        )
        .await
        .unwrap();

        let written = fs::read_to_string(tmp.path().join("exist.txt")).unwrap();
        assert_eq!(written, "new content");
    }

    #[tokio::test]
    async fn test_write_empty_content() {
        let tmp = TempDir::new().unwrap();
        let validator = make_validator(&tmp);

        let result = execute_write_file(
            json!({"path": "empty.txt", "content": ""}),
            &validator,
            10_485_760,
        )
        .await
        .unwrap();

        assert!(result.content.contains("0 bytes"));
        let written = fs::read_to_string(tmp.path().join("empty.txt")).unwrap();
        assert_eq!(written, "");
    }

    #[tokio::test]
    async fn test_write_path_display_in_output() {
        let tmp = TempDir::new().unwrap();
        let validator = make_validator(&tmp);

        let result = execute_write_file(
            json!({"path": "myfile.txt", "content": "data"}),
            &validator,
            10_485_760,
        )
        .await
        .unwrap();

        assert!(result.content.contains("myfile.txt"));
    }

    #[tokio::test]
    async fn test_write_new_file_in_sandbox() {
        let tmp = TempDir::new().unwrap();
        let validator = make_validator(&tmp);

        let result = execute_write_file(
            json!({"path": "brand_new.txt", "content": "fresh"}),
            &validator,
            10_485_760,
        )
        .await;

        assert!(result.is_ok());
        assert!(tmp.path().join("brand_new.txt").exists());
    }
}
