use std::sync::Arc;
use tokio::sync::Mutex;

use super::manager::LspServerManager;
use super::request::{parse_locations, send_position_request, PositionArgs};
use crate::browser::mcp::ToolExecutionResult;
use crate::config::schema::LspConfig;
use crate::error::AgentError;

pub async fn execute_lsp_goto_definition(
    arguments: serde_json::Value,
    manager: &Arc<Mutex<LspServerManager>>,
    config: &LspConfig,
) -> Result<ToolExecutionResult, AgentError> {
    let args = PositionArgs::parse("lsp_goto_definition", arguments)?;

    let result =
        send_position_request("textDocument/definition", &args, None, manager, config).await?;

    let locations = parse_locations(&result)?;

    let content = if locations.is_empty() {
        "No definition found.".to_string()
    } else {
        locations
            .iter()
            .map(|l| l.display())
            .collect::<Vec<_>>()
            .join("\n")
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
    fn formats_a_definition_as_path_line_col() {
        let result = json!({
            "uri": "file:///src/lib.rs",
            "range": {"start": {"line": 11, "character": 4}, "end": {"line": 11, "character": 9}}
        });
        let locs = parse_locations(&result).unwrap();
        assert_eq!(locs[0].display(), "/src/lib.rs:12:5");
    }

    #[test]
    fn no_definition_yields_no_locations() {
        assert!(parse_locations(&serde_json::Value::Null)
            .unwrap()
            .is_empty());
    }
}
