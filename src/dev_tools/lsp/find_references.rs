use std::sync::Arc;
use tokio::sync::Mutex;

use super::manager::LspServerManager;
use super::request::{parse_locations, send_position_request, PositionArgs};
use crate::browser::mcp::ToolExecutionResult;
use crate::config::schema::LspConfig;
use crate::error::AgentError;

pub async fn execute_lsp_find_references(
    arguments: serde_json::Value,
    manager: &Arc<Mutex<LspServerManager>>,
    config: &LspConfig,
) -> Result<ToolExecutionResult, AgentError> {
    let args = PositionArgs::parse("lsp_find_references", arguments)?;
    let include_declaration = args.include_declaration.unwrap_or(true);

    let result = send_position_request(
        "textDocument/references",
        &args,
        Some(serde_json::json!({
            "context": { "includeDeclaration": include_declaration }
        })),
        manager,
        config,
    )
    .await?;

    let locations = parse_locations(&result)?;

    let content = if locations.is_empty() {
        "No references found.".to_string()
    } else {
        let mut lines = vec![format!("Found {} reference(s):", locations.len())];
        lines.extend(locations.iter().map(|l| format!("  {}", l.display())));
        lines.join("\n")
    };

    Ok(ToolExecutionResult {
        content,
        is_error: false,
    })
}

#[cfg(test)]
mod tests {
    use super::super::request::parse_locations;
    use serde_json::json;

    #[test]
    fn formats_each_reference_as_path_line_col() {
        let result = json!([
            {"uri": "file:///a.rs", "range": {"start": {"line": 0, "character": 0},
                                              "end": {"line": 0, "character": 3}}},
            {"uri": "file:///b.rs", "range": {"start": {"line": 41, "character": 8},
                                              "end": {"line": 41, "character": 11}}}
        ]);
        let locs = parse_locations(&result).unwrap();
        assert_eq!(locs.len(), 2);
        assert_eq!(locs[0].display(), "/a.rs:1:1");
        assert_eq!(locs[1].display(), "/b.rs:42:9");
    }

    #[test]
    fn no_references_yields_no_locations() {
        assert!(parse_locations(&json!([])).unwrap().is_empty());
    }
}
