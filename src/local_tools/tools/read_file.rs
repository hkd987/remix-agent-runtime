use serde_json::Value;

use crate::browser::mcp::ToolExecutionResult;
use crate::error::AgentError;
use crate::local_tools::sandbox::PathValidator;

pub async fn execute_read_file(
    args: Value,
    path_validator: &PathValidator,
    max_bytes: usize,
) -> Result<ToolExecutionResult, AgentError> {
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AgentError::LocalTool("read_file requires 'path' parameter".to_string()))?;

    let offset = args
        .get("offset")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);

    let resolved = path_validator.resolve_path(path)?;

    let metadata = std::fs::metadata(&resolved)
        .map_err(|e| AgentError::LocalTool(format!("Failed to read file metadata: {e}")))?;

    if metadata.len() as usize > max_bytes {
        return Err(AgentError::LocalTool(format!(
            "File too large ({} bytes, max {})",
            metadata.len(),
            max_bytes
        )));
    }

    let content = std::fs::read_to_string(&resolved)
        .map_err(|e| AgentError::LocalTool(format!("Failed to read file: {e}")))?;

    let result = if offset.is_some() || limit.is_some() {
        let lines: Vec<&str> = content.lines().collect();
        let start = offset.unwrap_or(0);
        let end = limit
            .map(|l| (start + l).min(lines.len()))
            .unwrap_or(lines.len());

        if start >= lines.len() {
            String::new()
        } else {
            lines[start..end]
                .iter()
                .enumerate()
                .map(|(i, line)| format!("{:>6}\t{}", start + i + 1, line))
                .collect::<Vec<_>>()
                .join("\n")
        }
    } else {
        content
    };

    Ok(ToolExecutionResult {
        content: result,
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
    async fn test_read_basic_file() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("hello.txt"), "Hello, world!").unwrap();
        let validator = make_validator(&tmp);

        let result = execute_read_file(json!({"path": "hello.txt"}), &validator, 1_048_576)
            .await
            .unwrap();
        assert_eq!(result.content, "Hello, world!");
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_read_with_offset_and_limit() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("lines.txt"),
            "line1\nline2\nline3\nline4\nline5",
        )
        .unwrap();
        let validator = make_validator(&tmp);

        let result = execute_read_file(
            json!({"path": "lines.txt", "offset": 1, "limit": 2}),
            &validator,
            1_048_576,
        )
        .await
        .unwrap();
        assert!(result.content.contains("line2"));
        assert!(result.content.contains("line3"));
        assert!(!result.content.contains("line1"));
        assert!(!result.content.contains("line4"));
    }

    #[tokio::test]
    async fn test_read_with_offset_only() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("lines.txt"), "a\nb\nc\nd").unwrap();
        let validator = make_validator(&tmp);

        let result = execute_read_file(
            json!({"path": "lines.txt", "offset": 2}),
            &validator,
            1_048_576,
        )
        .await
        .unwrap();
        assert!(result.content.contains("c"));
        assert!(result.content.contains("d"));
        assert!(!result.content.contains("\ta\n"));
        assert!(!result.content.contains("\tb\n"));
    }

    #[tokio::test]
    async fn test_read_with_limit_only() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("lines.txt"), "a\nb\nc\nd").unwrap();
        let validator = make_validator(&tmp);

        let result = execute_read_file(
            json!({"path": "lines.txt", "limit": 2}),
            &validator,
            1_048_576,
        )
        .await
        .unwrap();
        assert!(result.content.contains("a"));
        assert!(result.content.contains("b"));
        assert!(!result.content.contains("\tc\n"));
    }

    #[tokio::test]
    async fn test_read_nonexistent_file() {
        let tmp = TempDir::new().unwrap();
        let validator = make_validator(&tmp);

        let result = execute_read_file(json!({"path": "nope.txt"}), &validator, 1_048_576).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("outside sandbox or parent directory does not exist")
                || err.contains("Failed to read file metadata")
        );
    }

    #[tokio::test]
    async fn test_read_too_large() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("big.txt"), "x".repeat(200)).unwrap();
        let validator = make_validator(&tmp);

        let result = execute_read_file(json!({"path": "big.txt"}), &validator, 100).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("File too large"));
    }

    #[tokio::test]
    async fn test_read_outside_sandbox() {
        let tmp = TempDir::new().unwrap();
        let validator = make_validator(&tmp);

        let result = execute_read_file(json!({"path": "/etc/passwd"}), &validator, 1_048_576).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("outside sandbox"));
    }

    #[tokio::test]
    async fn test_read_missing_path_param() {
        let tmp = TempDir::new().unwrap();
        let validator = make_validator(&tmp);

        let result = execute_read_file(json!({}), &validator, 1_048_576).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("requires 'path' parameter"));
    }

    #[tokio::test]
    async fn test_read_empty_file() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("empty.txt"), "").unwrap();
        let validator = make_validator(&tmp);

        let result = execute_read_file(json!({"path": "empty.txt"}), &validator, 1_048_576)
            .await
            .unwrap();
        assert_eq!(result.content, "");
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_read_offset_past_end() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("short.txt"), "one\ntwo").unwrap();
        let validator = make_validator(&tmp);

        let result = execute_read_file(
            json!({"path": "short.txt", "offset": 100}),
            &validator,
            1_048_576,
        )
        .await
        .unwrap();
        assert_eq!(result.content, "");
    }
}
