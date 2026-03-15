use std::sync::Arc;
use tokio::sync::Mutex;

use super::detection::detect_language_from_file;
use super::manager::{open_document, resolve_path, LspServerManager};
use crate::browser::mcp::ToolExecutionResult;
use crate::config::schema::LspConfig;
use crate::error::AgentError;

pub async fn execute_lsp_diagnostics(
    arguments: serde_json::Value,
    manager: &Arc<Mutex<LspServerManager>>,
    _config: &LspConfig,
) -> Result<ToolExecutionResult, AgentError> {
    let file = arguments.get("file").and_then(|v| v.as_str());

    let mut mgr = manager.lock().await;

    if let Some(file) = file {
        let language = detect_language_from_file(file).ok_or_else(|| {
            AgentError::ToolExecution(format!("Cannot determine language for file: {}", file))
        })?;

        let abs_path = resolve_path(file)?;

        let handle = mgr.get_or_start(language).await?;

        let file_uri = open_document(handle, &abs_path, language).await?;

        // Wait briefly for diagnostics to arrive
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        // Return cached diagnostics for this file
        let diagnostics = handle.diagnostics.get(&file_uri);
        let output = format_diagnostics(diagnostics, Some(file));
        Ok(ToolExecutionResult {
            content: output,
            is_error: false,
        })
    } else {
        // Return all cached diagnostics across all servers
        // This is a simplified implementation — in practice you'd query all active servers
        Ok(ToolExecutionResult {
            content:
                "No file specified. Provide a file path to get diagnostics for a specific file."
                    .to_string(),
            is_error: false,
        })
    }
}

fn format_diagnostics(
    diagnostics: Option<&Vec<lsp_types::Diagnostic>>,
    file: Option<&str>,
) -> String {
    match diagnostics {
        None => {
            if let Some(f) = file {
                format!("No diagnostics for {}.", f)
            } else {
                "No diagnostics.".to_string()
            }
        }
        Some(diags) if diags.is_empty() => {
            if let Some(f) = file {
                format!("No diagnostics for {}.", f)
            } else {
                "No diagnostics.".to_string()
            }
        }
        Some(diags) => {
            let mut lines = Vec::new();
            let prefix = file.unwrap_or("unknown");
            lines.push(format!("{} diagnostic(s) in {}:", diags.len(), prefix));

            for diag in diags {
                let severity = match diag.severity {
                    Some(lsp_types::DiagnosticSeverity::ERROR) => "error",
                    Some(lsp_types::DiagnosticSeverity::WARNING) => "warning",
                    Some(lsp_types::DiagnosticSeverity::INFORMATION) => "info",
                    Some(lsp_types::DiagnosticSeverity::HINT) => "hint",
                    _ => "unknown",
                };
                let line = diag.range.start.line + 1;
                let col = diag.range.start.character + 1;
                lines.push(format!(
                    "  [{}] {}:{}:{}: {}",
                    severity, prefix, line, col, diag.message
                ));
            }

            lines.join("\n")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_diagnostics_none() {
        let output = format_diagnostics(None, Some("test.rs"));
        assert_eq!(output, "No diagnostics for test.rs.");
    }

    #[test]
    fn test_format_diagnostics_empty() {
        let empty: Vec<lsp_types::Diagnostic> = vec![];
        let output = format_diagnostics(Some(&empty), Some("test.rs"));
        assert_eq!(output, "No diagnostics for test.rs.");
    }

    #[test]
    fn test_format_diagnostics_with_errors() {
        let diags = vec![lsp_types::Diagnostic {
            range: lsp_types::Range {
                start: lsp_types::Position {
                    line: 9,
                    character: 4,
                },
                end: lsp_types::Position {
                    line: 9,
                    character: 20,
                },
            },
            severity: Some(lsp_types::DiagnosticSeverity::ERROR),
            message: "expected `;`".to_string(),
            ..Default::default()
        }];
        let output = format_diagnostics(Some(&diags), Some("test.rs"));
        assert!(output.contains("[error]"));
        assert!(output.contains("test.rs:10:5"));
        assert!(output.contains("expected `;`"));
    }
}
