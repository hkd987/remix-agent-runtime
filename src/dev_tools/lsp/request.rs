//! Shared request plumbing for the position-based LSP tools.
//!
//! `goto_definition` and `find_references` each carried a byte-identical block for
//! extracting `file`/`line`/`column`, and another for language detection, path
//! resolution, document opening and the timeout wrapper. They also navigated responses
//! with `.pointer("/start/line").unwrap_or(0)` chains despite `lsp-types` being a
//! declared dependency — so a malformed or unexpected response silently formatted as
//! line 1, column 1 rather than reporting that it could not be read.

use std::sync::Arc;

use serde::Deserialize;
use tokio::sync::Mutex;

use super::detection::detect_language_from_file;
use super::manager::{open_document, resolve_path, send_request, LspServerManager};
use crate::config::schema::LspConfig;
use crate::error::AgentError;

/// Arguments common to every position-based LSP tool.
#[derive(Debug, Deserialize)]
pub struct PositionArgs {
    pub file: String,
    pub line: u32,
    pub column: u32,
    /// Only meaningful for `find_references`; ignored elsewhere.
    #[serde(default)]
    pub include_declaration: Option<bool>,
}

impl PositionArgs {
    pub fn parse(tool: &str, arguments: serde_json::Value) -> Result<Self, AgentError> {
        serde_json::from_value(arguments).map_err(|e| {
            AgentError::ToolExecution(format!(
                "Invalid arguments for {tool}: {e}. Expected file (string), line (integer, \
                 0-indexed) and column (integer, 0-indexed)."
            ))
        })
    }
}

/// Send a position-based request to the language server for `args.file`.
///
/// Handles language detection, path resolution, opening the document and the timeout,
/// all of which were duplicated per tool.
pub async fn send_position_request(
    method: &str,
    args: &PositionArgs,
    extra_params: Option<serde_json::Value>,
    manager: &Arc<Mutex<LspServerManager>>,
    config: &LspConfig,
) -> Result<serde_json::Value, AgentError> {
    let language = detect_language_from_file(&args.file).ok_or_else(|| {
        AgentError::ToolExecution(format!(
            "Cannot determine language for file: {}. Supported extensions: .rs, .ts, .tsx, \
             .js, .jsx, .py, .go",
            args.file
        ))
    })?;

    let abs_path = resolve_path(&args.file)?;

    let mut mgr = manager.lock().await;
    let handle = mgr.get_or_start(language).await?;
    let file_uri = open_document(handle, &abs_path, language).await?;

    let mut params = serde_json::json!({
        "textDocument": { "uri": file_uri },
        "position": { "line": args.line, "character": args.column }
    });
    if let (Some(extra), Some(obj)) = (extra_params, params.as_object_mut()) {
        if let Some(extra_obj) = extra.as_object() {
            for (k, v) in extra_obj {
                obj.insert(k.clone(), v.clone());
            }
        }
    }

    tokio::time::timeout(
        std::time::Duration::from_secs(config.request_timeout_secs),
        send_request(handle, method, params),
    )
    .await
    .map_err(|_| AgentError::ToolExecution("LSP request timed out".to_string()))?
}

/// Interpret an LSP response as a list of locations.
///
/// Servers may answer a definition request with a single `Location`, an array of them,
/// or an array of `LocationLink`s; `lsp_types::GotoDefinitionResponse` covers all three.
/// Returning `Err` on an unreadable payload is the point — the previous `.pointer()`
/// chains turned one into a confident "line 1, column 1".
pub fn parse_locations(result: &serde_json::Value) -> Result<Vec<ResolvedLocation>, AgentError> {
    if result.is_null() {
        return Ok(Vec::new());
    }

    let response: lsp_types::GotoDefinitionResponse = serde_json::from_value(result.clone())
        .map_err(|e| {
            AgentError::ToolExecution(format!("Could not interpret the LSP response: {e}"))
        })?;

    Ok(match response {
        lsp_types::GotoDefinitionResponse::Scalar(loc) => vec![ResolvedLocation::from(&loc)],
        lsp_types::GotoDefinitionResponse::Array(locs) => {
            locs.iter().map(ResolvedLocation::from).collect()
        }
        lsp_types::GotoDefinitionResponse::Link(links) => links
            .iter()
            .map(|l| ResolvedLocation {
                path: uri_to_path(l.target_uri.as_str()),
                line: l.target_range.start.line,
                column: l.target_range.start.character,
            })
            .collect(),
    })
}

/// A source position, already converted to the 1-indexed form humans and editors use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedLocation {
    pub path: String,
    /// 0-indexed, as LSP reports it.
    pub line: u32,
    /// 0-indexed, as LSP reports it.
    pub column: u32,
}

impl From<&lsp_types::Location> for ResolvedLocation {
    fn from(loc: &lsp_types::Location) -> Self {
        Self {
            path: uri_to_path(loc.uri.as_str()),
            line: loc.range.start.line,
            column: loc.range.start.character,
        }
    }
}

impl ResolvedLocation {
    /// `path:line:col`, converted to the 1-indexed convention editors use.
    pub fn display(&self) -> String {
        format!("{}:{}:{}", self.path, self.line + 1, self.column + 1)
    }
}

fn uri_to_path(uri: &str) -> String {
    uri.strip_prefix("file://").unwrap_or(uri).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_position_arguments() {
        let args = PositionArgs::parse(
            "lsp_goto_definition",
            json!({"file": "a.rs", "line": 3, "column": 7}),
        )
        .unwrap();
        assert_eq!(args.file, "a.rs");
        assert_eq!(args.line, 3);
        assert_eq!(args.column, 7);
        assert_eq!(args.include_declaration, None);
    }

    #[test]
    fn rejects_missing_arguments_with_a_useful_message() {
        let err = PositionArgs::parse("lsp_goto_definition", json!({"file": "a.rs"})).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("Invalid arguments for lsp_goto_definition"),
            "{msg}"
        );
        assert!(msg.contains("0-indexed"), "{msg}");
    }

    #[test]
    fn null_response_is_no_locations() {
        assert!(parse_locations(&serde_json::Value::Null)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn parses_a_single_location() {
        let result = json!({
            "uri": "file:///src/main.rs",
            "range": {"start": {"line": 41, "character": 8}, "end": {"line": 41, "character": 20}}
        });
        let locs = parse_locations(&result).unwrap();
        assert_eq!(locs.len(), 1);
        assert_eq!(locs[0].path, "/src/main.rs");
        // Reported 0-indexed, displayed 1-indexed.
        assert_eq!(locs[0].line, 41);
        assert_eq!(locs[0].display(), "/src/main.rs:42:9");
    }

    #[test]
    fn parses_an_array_of_locations() {
        let result = json!([
            {"uri": "file:///a.rs", "range": {"start": {"line": 0, "character": 0},
                                              "end": {"line": 0, "character": 1}}},
            {"uri": "file:///b.rs", "range": {"start": {"line": 9, "character": 4},
                                              "end": {"line": 9, "character": 8}}}
        ]);
        let locs = parse_locations(&result).unwrap();
        assert_eq!(locs.len(), 2);
        assert_eq!(locs[1].display(), "/b.rs:10:5");
    }

    #[test]
    fn parses_location_links() {
        // rust-analyzer answers definition requests with LocationLink.
        let result = json!([{
            "targetUri": "file:///lib.rs",
            "targetRange": {"start": {"line": 2, "character": 0},
                            "end": {"line": 5, "character": 1}},
            "targetSelectionRange": {"start": {"line": 2, "character": 3},
                                     "end": {"line": 2, "character": 10}}
        }]);
        let locs = parse_locations(&result).unwrap();
        assert_eq!(locs.len(), 1);
        assert_eq!(locs[0].display(), "/lib.rs:3:1");
    }

    #[test]
    fn malformed_response_is_reported_not_guessed() {
        // The previous `.pointer("/start/line").unwrap_or(0)` chains turned this into a
        // confident "line 1, column 1".
        let result = json!({"uri": "file:///a.rs", "range": {"start": {"line": "not a number"}}});
        let err = parse_locations(&result).unwrap_err();
        assert!(
            err.to_string()
                .contains("Could not interpret the LSP response"),
            "{err}"
        );
    }

    #[test]
    fn empty_array_is_no_locations() {
        assert!(parse_locations(&json!([])).unwrap().is_empty());
    }
}
