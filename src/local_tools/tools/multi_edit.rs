//! Apply several edits to a single file in one call.
//!
//! `multi_edit` was already referenced by the permission layer — it is in
//! `ACCEPT_EDITS_AUTO_ALLOW` and has a passing permission test — but no such tool
//! existed, so the entry was inert and the agent had to spend one round trip per edit.
//!
//! Edits apply in order, each against the result of the previous one, and the whole
//! batch is atomic: if any edit fails to match, the file is left untouched.

use serde::Deserialize;
use serde_json::Value;

use crate::browser::mcp::ToolExecutionResult;
use crate::error::AgentError;
use crate::local_tools::sandbox::PathValidator;

#[derive(Debug, Deserialize)]
struct MultiEditArgs {
    path: String,
    edits: Vec<Edit>,
}

#[derive(Debug, Deserialize)]
struct Edit {
    old_string: String,
    new_string: String,
    /// Replace every occurrence instead of requiring a unique match.
    #[serde(default)]
    replace_all: bool,
}

pub async fn execute_multi_edit(
    args: Value,
    path_validator: &PathValidator,
) -> Result<ToolExecutionResult, AgentError> {
    let args: MultiEditArgs = serde_json::from_value(args).map_err(|e| {
        AgentError::LocalTool(format!(
            "multi_edit requires 'path' and 'edits' (a list of \
             {{old_string, new_string, replace_all?}}): {e}"
        ))
    })?;

    if args.edits.is_empty() {
        return Err(AgentError::LocalTool(
            "multi_edit requires at least one edit".to_string(),
        ));
    }

    let resolved = path_validator.resolve_path(&args.path)?;

    let original = std::fs::read_to_string(&resolved)
        .map_err(|e| AgentError::LocalTool(format!("Failed to read file: {e}")))?;

    // Apply everything to an in-memory copy first. A partially-applied batch would
    // leave the file in a state neither the model nor the user asked for.
    let mut content = original;
    let mut applied = 0usize;

    for (i, edit) in args.edits.iter().enumerate() {
        if edit.old_string.is_empty() {
            return Err(AgentError::LocalTool(format!(
                "edit {} has an empty old_string",
                i + 1
            )));
        }

        let count = content.matches(&edit.old_string).count();

        if count == 0 {
            return Err(AgentError::LocalTool(format!(
                "edit {} of {}: old_string not found in {}. No edits were applied.",
                i + 1,
                args.edits.len(),
                args.path
            )));
        }

        if count > 1 && !edit.replace_all {
            return Err(AgentError::LocalTool(format!(
                "edit {} of {}: old_string found {} times in {} (must be unique, or set \
                 replace_all). No edits were applied.",
                i + 1,
                args.edits.len(),
                count,
                args.path
            )));
        }

        content = if edit.replace_all {
            applied += count;
            content.replace(&edit.old_string, &edit.new_string)
        } else {
            applied += 1;
            content.replacen(&edit.old_string, &edit.new_string, 1)
        };
    }

    std::fs::write(&resolved, &content)
        .map_err(|e| AgentError::LocalTool(format!("Failed to write file: {e}")))?;

    Ok(ToolExecutionResult {
        content: format!(
            "Applied {} edit{} ({} replacement{}) to {}",
            args.edits.len(),
            if args.edits.len() == 1 { "" } else { "s" },
            applied,
            if applied == 1 { "" } else { "s" },
            args.path
        ),
        is_error: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    fn setup(content: &str) -> (TempDir, PathValidator, String) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("file.txt");
        std::fs::write(&path, content).unwrap();
        let validator = PathValidator::new(dir.path().to_path_buf()).unwrap();
        (dir, validator, "file.txt".to_string())
    }

    #[tokio::test]
    async fn applies_edits_in_order() {
        let (dir, validator, path) = setup("alpha beta gamma");
        let result = execute_multi_edit(
            json!({
                "path": path,
                "edits": [
                    {"old_string": "alpha", "new_string": "one"},
                    {"old_string": "beta", "new_string": "two"},
                ]
            }),
            &validator,
        )
        .await
        .unwrap();

        assert!(!result.is_error);
        let content = std::fs::read_to_string(dir.path().join("file.txt")).unwrap();
        assert_eq!(content, "one two gamma");
    }

    #[tokio::test]
    async fn later_edit_sees_earlier_result() {
        let (dir, validator, path) = setup("aaa");
        execute_multi_edit(
            json!({
                "path": path,
                "edits": [
                    {"old_string": "aaa", "new_string": "bbb"},
                    {"old_string": "bbb", "new_string": "ccc"},
                ]
            }),
            &validator,
        )
        .await
        .unwrap();

        let content = std::fs::read_to_string(dir.path().join("file.txt")).unwrap();
        assert_eq!(content, "ccc");
    }

    #[tokio::test]
    async fn batch_is_atomic_on_failure() {
        let (dir, validator, path) = setup("alpha beta");
        let err = execute_multi_edit(
            json!({
                "path": path,
                "edits": [
                    {"old_string": "alpha", "new_string": "one"},
                    {"old_string": "nonexistent", "new_string": "two"},
                ]
            }),
            &validator,
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("No edits were applied"), "{err}");
        // The first edit must not have been written.
        let content = std::fs::read_to_string(dir.path().join("file.txt")).unwrap();
        assert_eq!(content, "alpha beta");
    }

    #[tokio::test]
    async fn ambiguous_match_is_rejected_without_replace_all() {
        let (dir, validator, path) = setup("x x x");
        let err = execute_multi_edit(
            json!({
                "path": path,
                "edits": [{"old_string": "x", "new_string": "y"}]
            }),
            &validator,
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("must be unique"), "{err}");
        let content = std::fs::read_to_string(dir.path().join("file.txt")).unwrap();
        assert_eq!(content, "x x x");
    }

    #[tokio::test]
    async fn replace_all_replaces_every_occurrence() {
        let (dir, validator, path) = setup("x x x");
        let result = execute_multi_edit(
            json!({
                "path": path,
                "edits": [{"old_string": "x", "new_string": "y", "replace_all": true}]
            }),
            &validator,
        )
        .await
        .unwrap();

        assert!(
            result.content.contains("3 replacements"),
            "{}",
            result.content
        );
        let content = std::fs::read_to_string(dir.path().join("file.txt")).unwrap();
        assert_eq!(content, "y y y");
    }

    #[tokio::test]
    async fn empty_edits_list_is_rejected() {
        let (_dir, validator, path) = setup("content");
        let err = execute_multi_edit(json!({"path": path, "edits": []}), &validator)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("at least one edit"), "{err}");
    }

    #[tokio::test]
    async fn malformed_arguments_are_rejected() {
        let (_dir, validator, path) = setup("content");
        let err = execute_multi_edit(json!({"path": path}), &validator)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("multi_edit requires"), "{err}");
    }
}
