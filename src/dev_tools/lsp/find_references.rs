use std::sync::Arc;
use tokio::sync::Mutex;

use super::detection::detect_language_from_file;
use super::manager::{open_document, resolve_path, send_request, LspServerManager};
use crate::browser::mcp::ToolExecutionResult;
use crate::config::schema::LspConfig;
use crate::error::AgentError;

pub async fn execute_lsp_find_references(
    arguments: serde_json::Value,
    manager: &Arc<Mutex<LspServerManager>>,
    config: &LspConfig,
) -> Result<ToolExecutionResult, AgentError> {
    let file = arguments
        .get("file")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AgentError::ToolExecution("Missing required parameter: file".to_string()))?;
    let line = arguments
        .get("line")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| AgentError::ToolExecution("Missing required parameter: line".to_string()))?;
    let column = arguments
        .get("column")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| {
            AgentError::ToolExecution("Missing required parameter: column".to_string())
        })?;
    let include_declaration = arguments
        .get("include_declaration")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    let language = detect_language_from_file(file).ok_or_else(|| {
        AgentError::ToolExecution(format!(
            "Cannot determine language for file: {}. Supported extensions: .rs, .ts, .tsx, .js, .jsx, .py, .go",
            file
        ))
    })?;

    let abs_path = resolve_path(file)?;

    let mut mgr = manager.lock().await;
    let handle = mgr.get_or_start(language).await?;

    let file_uri = open_document(handle, &abs_path, language).await?;

    let params = serde_json::json!({
        "textDocument": { "uri": file_uri },
        "position": { "line": line, "character": column },
        "context": { "includeDeclaration": include_declaration }
    });

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(config.request_timeout_secs),
        send_request(handle, "textDocument/references", params),
    )
    .await
    .map_err(|_| AgentError::ToolExecution("LSP request timed out".to_string()))??;

    let output = format_references_result(&result);
    Ok(ToolExecutionResult {
        content: output,
        is_error: false,
    })
}

fn format_references_result(result: &serde_json::Value) -> String {
    if result.is_null() {
        return "No references found.".to_string();
    }

    if let Some(arr) = result.as_array() {
        if arr.is_empty() {
            return "No references found.".to_string();
        }

        let mut lines: Vec<String> = Vec::new();
        lines.push(format!("Found {} reference(s):", arr.len()));

        for loc in arr {
            let uri = loc.get("uri").and_then(|u| u.as_str()).unwrap_or("unknown");
            let file_path = uri.strip_prefix("file://").unwrap_or(uri);

            if let Some(range) = loc.get("range") {
                let start_line = range
                    .pointer("/start/line")
                    .and_then(|l| l.as_u64())
                    .unwrap_or(0);
                let start_col = range
                    .pointer("/start/character")
                    .and_then(|c| c.as_u64())
                    .unwrap_or(0);
                lines.push(format!(
                    "  {}:{}:{}",
                    file_path,
                    start_line + 1,
                    start_col + 1
                ));
            } else {
                lines.push(format!("  {}", file_path));
            }
        }

        lines.join("\n")
    } else {
        format!("References result: {}", result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_format_references_null() {
        assert_eq!(
            format_references_result(&json!(null)),
            "No references found."
        );
    }

    #[test]
    fn test_format_references_empty() {
        assert_eq!(format_references_result(&json!([])), "No references found.");
    }

    #[test]
    fn test_format_references_multiple() {
        let result = json!([
            {
                "uri": "file:///src/main.rs",
                "range": {
                    "start": { "line": 10, "character": 4 },
                    "end": { "line": 10, "character": 20 }
                }
            },
            {
                "uri": "file:///src/lib.rs",
                "range": {
                    "start": { "line": 5, "character": 0 },
                    "end": { "line": 5, "character": 10 }
                }
            }
        ]);
        let output = format_references_result(&result);
        assert!(output.contains("Found 2 reference(s):"));
        assert!(output.contains("/src/main.rs:11:5"));
        assert!(output.contains("/src/lib.rs:6:1"));
    }
}
