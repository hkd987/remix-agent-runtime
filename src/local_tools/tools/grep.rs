use serde_json::Value;
use std::path::Path;

use crate::browser::mcp::ToolExecutionResult;
use crate::error::AgentError;
use crate::local_tools::sandbox::PathValidator;

pub async fn execute_grep(
    args: Value,
    path_validator: &PathValidator,
) -> Result<ToolExecutionResult, AgentError> {
    let pattern = args
        .get("pattern")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AgentError::LocalTool("grep requires 'pattern' parameter".to_string()))?;

    let search_path = args.get("path").and_then(|v| v.as_str());

    let include = args.get("include").and_then(|v| v.as_str());

    let context_lines = args
        .get("context_lines")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;

    let regex = regex::Regex::new(pattern)
        .map_err(|e| AgentError::LocalTool(format!("Invalid regex pattern: {e}")))?;

    let base_path = if let Some(p) = search_path {
        path_validator.resolve_path(p)?
    } else {
        path_validator.root().to_path_buf()
    };

    let include_glob = include
        .map(glob::Pattern::new)
        .transpose()
        .map_err(|e| AgentError::LocalTool(format!("Invalid include pattern: {e}")))?;

    let mut ctx = SearchContext {
        regex: &regex,
        include: &include_glob,
        context_lines,
        root: path_validator.root(),
        results: Vec::new(),
        total_bytes: 0,
        max_bytes: 100 * 1024, // 100KB limit
    };

    search_files_recursive(&base_path, &mut ctx)?;

    if ctx.results.is_empty() {
        Ok(ToolExecutionResult {
            content: "No matches found".to_string(),
            is_error: false,
        })
    } else {
        Ok(ToolExecutionResult {
            content: ctx.results.join("\n"),
            is_error: false,
        })
    }
}

/// Groups the immutable search parameters and mutable output state.
struct SearchContext<'a> {
    regex: &'a regex::Regex,
    include: &'a Option<glob::Pattern>,
    context_lines: usize,
    root: &'a Path,
    results: Vec<String>,
    total_bytes: usize,
    max_bytes: usize,
}

fn search_files_recursive(dir: &Path, ctx: &mut SearchContext<'_>) -> Result<(), AgentError> {
    if !dir.is_dir() {
        return search_file(dir, ctx);
    }

    let entries = std::fs::read_dir(dir)
        .map_err(|e| AgentError::LocalTool(format!("Failed to read directory: {e}")))?;

    let mut sorted_entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
    sorted_entries.sort_by_key(|e| e.path());

    for entry in sorted_entries {
        if ctx.total_bytes >= ctx.max_bytes {
            ctx.results
                .push("... output truncated (100KB limit)".to_string());
            return Ok(());
        }

        let path = entry.path();
        if path.is_dir() {
            // Skip hidden directories
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with('.'))
            {
                continue;
            }
            search_files_recursive(&path, ctx)?;
        } else if path.is_file() {
            search_file(&path, ctx)?;
        }
    }

    Ok(())
}

fn search_file(file: &Path, ctx: &mut SearchContext<'_>) -> Result<(), AgentError> {
    // Check include pattern
    if let Some(ref pattern) = ctx.include {
        if let Some(name) = file.file_name().and_then(|n| n.to_str()) {
            if !pattern.matches(name) {
                return Ok(());
            }
        }
    }

    // Skip binary/unreadable files
    let content = match std::fs::read_to_string(file) {
        Ok(c) => c,
        Err(_) => return Ok(()),
    };

    let relative = file.strip_prefix(ctx.root).unwrap_or(file);
    let lines: Vec<&str> = content.lines().collect();

    // Track the last printed line index to avoid duplicating context lines
    // when matches are adjacent or overlapping.
    let mut last_printed_line: Option<usize> = None;

    for (i, line) in lines.iter().enumerate() {
        if ctx.regex.is_match(line) {
            let start = i.saturating_sub(ctx.context_lines);
            let end = (i + ctx.context_lines + 1).min(lines.len());

            // Insert a separator if there's a gap between this group and the last printed line
            if let Some(last) = last_printed_line {
                if start > last + 1 {
                    ctx.results.push("--".to_string());
                    ctx.total_bytes += 3; // "--\n"
                }
            }

            for (j, ctx_line) in lines.iter().enumerate().take(end).skip(start) {
                // Skip lines already printed by a previous match's context
                if let Some(last) = last_printed_line {
                    if j <= last {
                        continue;
                    }
                }

                let prefix = if j == i { ">" } else { " " };
                let entry = format!("{}{}:{}: {}", prefix, relative.display(), j + 1, ctx_line);
                ctx.total_bytes += entry.len();
                ctx.results.push(entry);

                if ctx.total_bytes >= ctx.max_bytes {
                    return Ok(());
                }
            }

            last_printed_line = Some(end - 1);
        }
    }

    Ok(())
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
    async fn test_grep_basic_match() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("test.txt"),
            "hello world\nfoo bar\nhello again",
        )
        .unwrap();
        let validator = make_validator(&tmp);

        let result = execute_grep(json!({"pattern": "hello"}), &validator)
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("hello world"));
        assert!(result.content.contains("hello again"));
        assert!(!result.content.contains("foo bar"));
    }

    #[tokio::test]
    async fn test_grep_regex_pattern() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("code.rs"),
            "fn main() {}\nfn helper() {}\nlet x = 5;",
        )
        .unwrap();
        let validator = make_validator(&tmp);

        let result = execute_grep(json!({"pattern": "fn \\w+\\("}), &validator)
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("fn main()"));
        assert!(result.content.contains("fn helper()"));
        assert!(!result.content.contains("let x"));
    }

    #[tokio::test]
    async fn test_grep_with_include() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("file.rs"), "hello rust").unwrap();
        fs::write(tmp.path().join("file.txt"), "hello text").unwrap();
        let validator = make_validator(&tmp);

        let result = execute_grep(json!({"pattern": "hello", "include": "*.rs"}), &validator)
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("hello rust"));
        assert!(!result.content.contains("hello text"));
    }

    #[tokio::test]
    async fn test_grep_with_context_lines() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("ctx.txt"),
            "line1\nline2\nMATCH\nline4\nline5",
        )
        .unwrap();
        let validator = make_validator(&tmp);

        let result = execute_grep(json!({"pattern": "MATCH", "context_lines": 1}), &validator)
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("line2"));
        assert!(result.content.contains("MATCH"));
        assert!(result.content.contains("line4"));
    }

    #[tokio::test]
    async fn test_grep_no_matches() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("nope.txt"), "nothing here").unwrap();
        let validator = make_validator(&tmp);

        let result = execute_grep(json!({"pattern": "NONEXISTENT"}), &validator)
            .await
            .unwrap();
        assert!(!result.is_error);
        assert_eq!(result.content, "No matches found");
    }

    #[tokio::test]
    async fn test_grep_invalid_regex() {
        let tmp = TempDir::new().unwrap();
        let validator = make_validator(&tmp);

        let result = execute_grep(json!({"pattern": "[invalid"}), &validator).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Invalid regex pattern"));
    }

    #[tokio::test]
    async fn test_grep_specific_file() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("target.txt"), "find me here").unwrap();
        fs::write(tmp.path().join("other.txt"), "find me too").unwrap();
        let validator = make_validator(&tmp);

        let result = execute_grep(
            json!({"pattern": "find me", "path": "target.txt"}),
            &validator,
        )
        .await
        .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("find me here"));
        assert!(!result.content.contains("find me too"));
    }

    #[tokio::test]
    async fn test_grep_missing_pattern_param() {
        let tmp = TempDir::new().unwrap();
        let validator = make_validator(&tmp);

        let result = execute_grep(json!({}), &validator).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("requires 'pattern' parameter"));
    }

    #[tokio::test]
    async fn test_grep_nested_directories() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("sub/deep")).unwrap();
        fs::write(tmp.path().join("top.txt"), "needle top").unwrap();
        fs::write(tmp.path().join("sub/mid.txt"), "needle mid").unwrap();
        fs::write(tmp.path().join("sub/deep/bottom.txt"), "needle bottom").unwrap();
        let validator = make_validator(&tmp);

        let result = execute_grep(json!({"pattern": "needle"}), &validator)
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("needle top"));
        assert!(result.content.contains("needle mid"));
        assert!(result.content.contains("needle bottom"));
    }

    #[tokio::test]
    async fn test_grep_context_no_duplicate_lines_adjacent_matches() {
        let tmp = TempDir::new().unwrap();
        // Lines 1-5, matches on lines 2 and 4 with context_lines=1
        // Without dedup: line1,>line2,line3,line3,>line4,line5
        // With dedup:    line1,>line2,line3,>line4,line5
        fs::write(
            tmp.path().join("adj.txt"),
            "line1\nMATCH_A\nline3\nMATCH_B\nline5",
        )
        .unwrap();
        let validator = make_validator(&tmp);

        let result = execute_grep(json!({"pattern": "MATCH", "context_lines": 1}), &validator)
            .await
            .unwrap();
        assert!(!result.is_error);

        // Count occurrences of each line — none should appear more than once
        let lines: Vec<&str> = result.content.lines().collect();
        for line in &lines {
            if line.starts_with("--") {
                continue;
            }
            let count = lines.iter().filter(|l| *l == line).count();
            assert_eq!(
                count, 1,
                "Line '{}' appears {} times, expected 1",
                line, count
            );
        }

        // Verify all expected lines are present
        assert!(result.content.contains("line1"));
        assert!(result.content.contains("MATCH_A"));
        assert!(result.content.contains("line3"));
        assert!(result.content.contains("MATCH_B"));
        assert!(result.content.contains("line5"));
    }

    #[tokio::test]
    async fn test_grep_context_no_duplicate_lines_overlapping_context() {
        let tmp = TempDir::new().unwrap();
        // Two matches right next to each other with context_lines=2
        fs::write(
            tmp.path().join("overlap.txt"),
            "a\nb\nMATCH1\nd\nMATCH2\nf\ng",
        )
        .unwrap();
        let validator = make_validator(&tmp);

        let result = execute_grep(json!({"pattern": "MATCH", "context_lines": 2}), &validator)
            .await
            .unwrap();
        assert!(!result.is_error);

        let lines: Vec<&str> = result.content.lines().collect();
        // Filter out separators for duplicate check
        let content_lines: Vec<&&str> = lines.iter().filter(|l| !l.starts_with("--")).collect();
        for line in &content_lines {
            let count = content_lines.iter().filter(|l| *l == line).count();
            assert_eq!(
                count, 1,
                "Line '{}' appears {} times, expected 1",
                line, count
            );
        }
    }

    #[tokio::test]
    async fn test_grep_context_separator_between_groups() {
        let tmp = TempDir::new().unwrap();
        // Two matches far apart — should have "--" separator between groups
        fs::write(
            tmp.path().join("sep.txt"),
            "a\nMATCH1\nc\nd\ne\nf\ng\nMATCH2\ni",
        )
        .unwrap();
        let validator = make_validator(&tmp);

        let result = execute_grep(json!({"pattern": "MATCH", "context_lines": 1}), &validator)
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(
            result.content.contains("--"),
            "Should have separator between non-adjacent match groups"
        );
    }

    #[tokio::test]
    async fn test_grep_context_no_separator_adjacent_groups() {
        let tmp = TempDir::new().unwrap();
        // Matches close enough that context overlaps — no separator
        fs::write(
            tmp.path().join("nosep.txt"),
            "a\nMATCH1\nc\nMATCH2\ne",
        )
        .unwrap();
        let validator = make_validator(&tmp);

        let result = execute_grep(json!({"pattern": "MATCH", "context_lines": 1}), &validator)
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(
            !result.content.contains("--"),
            "Should NOT have separator when context groups are adjacent/overlapping"
        );
    }

    #[tokio::test]
    async fn test_grep_skips_binary_files() {
        let tmp = TempDir::new().unwrap();
        // Write a file with null bytes (binary)
        let mut binary_content = vec![0u8; 100];
        binary_content.extend_from_slice(b"needle");
        fs::write(tmp.path().join("binary.bin"), &binary_content).unwrap();
        fs::write(tmp.path().join("text.txt"), "needle in text").unwrap();
        let validator = make_validator(&tmp);

        let result = execute_grep(json!({"pattern": "needle"}), &validator)
            .await
            .unwrap();
        assert!(!result.is_error);
        // Binary file should be skipped (read_to_string fails on null bytes)
        assert!(result.content.contains("needle in text"));
    }
}
