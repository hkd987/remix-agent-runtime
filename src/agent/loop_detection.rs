use std::collections::HashMap;
use std::fmt::Write;
use std::hash::{Hash, Hasher};

use crate::config::schema::LoopDetectionConfig;
use crate::output::result::StepRecord;

/// Compute a deterministic hash for a serde_json::Value by writing
/// a canonical form (sorted object keys) directly into a hasher.
fn canonical_hash(value: &serde_json::Value) -> u64 {
    let mut buf = String::with_capacity(128);
    write_canonical(value, &mut buf);
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    buf.hash(&mut hasher);
    hasher.finish()
}

/// Write a canonical JSON representation with sorted object keys into a buffer.
fn write_canonical(value: &serde_json::Value, buf: &mut String) {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            buf.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    buf.push(',');
                }
                // Keys are always strings — write escaped form
                let _ = write!(buf, "\"{}\":", k);
                write_canonical(&map[k], buf);
            }
            buf.push('}');
        }
        serde_json::Value::Array(arr) => {
            buf.push('[');
            for (i, v) in arr.iter().enumerate() {
                if i > 0 {
                    buf.push(',');
                }
                write_canonical(v, buf);
            }
            buf.push(']');
        }
        other => {
            if let Ok(s) = serde_json::to_string(other) {
                buf.push_str(&s);
            }
        }
    }
}

/// Detect if the agent is stuck in a loop by looking at recent step history.
///
/// Returns `Some(warning_message)` if a loop is detected, `None` otherwise.
pub fn detect_loop(steps: &[StepRecord], config: &LoopDetectionConfig) -> Option<String> {
    if steps.is_empty() || config.max_repeats == 0 {
        return None;
    }

    let window_start = steps.len().saturating_sub(config.window_size as usize);
    let window = &steps[window_start..];

    // Count fingerprint occurrences using (tool_name, input_hash) tuples
    let mut counts: HashMap<(&str, u64), u32> = HashMap::new();
    for step in window {
        let key = (step.tool.as_str(), canonical_hash(&step.input));
        *counts.entry(key).or_insert(0) += 1;
    }

    // Find the most repeated fingerprint that exceeds the threshold
    let most_repeated = counts
        .iter()
        .filter(|(_, &count)| count >= config.max_repeats)
        .max_by_key(|(_, &count)| count);

    most_repeated.map(|(&(tool, _), count)| {
        config.message.clone().unwrap_or_else(|| {
            format!(
                "[LOOP DETECTED] You have called '{}' with the same input {} times in the last {} steps. \
                 Step back, re-read the error output or task requirements, and try a fundamentally different approach.",
                tool, count, window.len()
            )
        })
    })
}

/// Returns true if the tool name represents a file-writing operation.
fn is_write_tool(tool: &str) -> bool {
    matches!(tool, "write_file" | "edit_file")
}

/// Detect if the agent is stuck re-testing without modifying its solution.
/// Scans backward from the most recent step. Counts consecutive failing steps
/// with no write_file/edit_file interleaved. If count >= threshold, returns warning.
pub fn detect_semantic_loop(steps: &[StepRecord], config: &LoopDetectionConfig) -> Option<String> {
    if steps.is_empty() || config.max_failures_without_write == 0 {
        return None;
    }

    let window_start = steps.len().saturating_sub(config.window_size as usize);
    let window = &steps[window_start..];

    let mut failure_count: u32 = 0;
    for step in window.iter().rev() {
        if is_write_tool(&step.tool) {
            break;
        }
        if step.is_error == Some(true) {
            failure_count += 1;
        }
    }

    if failure_count >= config.max_failures_without_write {
        Some(config.semantic_loop_message.clone().unwrap_or_else(|| {
            format!(
                "[SEMANTIC LOOP DETECTED] You have run {} failing commands without modifying any files. \
                 Stop re-testing and instead edit your code to fix the underlying issue. \
                 Read the error output carefully and make a targeted change with write_file or edit_file.",
                failure_count
            )
        }))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_step(tool: &str, input: serde_json::Value) -> StepRecord {
        StepRecord {
            iteration: 1,
            tool: tool.to_string(),
            input,
            output: json!({}),
            duration_ms: 100,
            is_error: None,
        }
    }

    fn make_error_step(tool: &str, input: serde_json::Value) -> StepRecord {
        StepRecord {
            iteration: 1,
            tool: tool.to_string(),
            input,
            output: json!({}),
            duration_ms: 100,
            is_error: Some(true),
        }
    }

    #[test]
    fn test_no_loop_with_empty_steps() {
        let config = LoopDetectionConfig::default();
        assert!(detect_loop(&[], &config).is_none());
    }

    #[test]
    fn test_no_loop_with_varied_calls() {
        let config = LoopDetectionConfig::default();
        let steps = vec![
            make_step("bash", json!({"command": "ls"})),
            make_step("bash", json!({"command": "pwd"})),
            make_step("read_file", json!({"path": "/a.txt"})),
            make_step("bash", json!({"command": "cat b.txt"})),
        ];
        assert!(detect_loop(&steps, &config).is_none());
    }

    #[test]
    fn test_detects_loop_with_repeated_calls() {
        let config = LoopDetectionConfig {
            max_repeats: 3,
            window_size: 10,
            message: None,
            max_failures_without_write: 4,
            semantic_loop_message: None,
        };
        let steps = vec![
            make_step("bash", json!({"command": "cargo test"})),
            make_step(
                "edit_file",
                json!({"path": "/src/main.rs", "content": "fix"}),
            ),
            make_step("bash", json!({"command": "cargo test"})),
            make_step(
                "edit_file",
                json!({"path": "/src/main.rs", "content": "fix"}),
            ),
            make_step("bash", json!({"command": "cargo test"})),
        ];
        let result = detect_loop(&steps, &config);
        assert!(result.is_some());
        let msg = result.unwrap();
        assert!(msg.contains("LOOP DETECTED"));
        assert!(msg.contains("bash"));
        assert!(msg.contains("3"));
    }

    #[test]
    fn test_no_loop_below_threshold() {
        let config = LoopDetectionConfig {
            max_repeats: 3,
            window_size: 10,
            message: None,
            ..Default::default()
        };
        let steps = vec![
            make_step("bash", json!({"command": "cargo test"})),
            make_step("bash", json!({"command": "cargo test"})),
        ];
        assert!(detect_loop(&steps, &config).is_none());
    }

    #[test]
    fn test_window_size_limits_lookback() {
        let config = LoopDetectionConfig {
            max_repeats: 3,
            window_size: 3,
            message: None,
            ..Default::default()
        };
        // The first 3 repeats are outside the window
        let steps = vec![
            make_step("bash", json!({"command": "cargo test"})),
            make_step("bash", json!({"command": "cargo test"})),
            make_step("bash", json!({"command": "cargo test"})),
            make_step("read_file", json!({"path": "/a.txt"})),
            make_step("bash", json!({"command": "ls"})),
            make_step("read_file", json!({"path": "/b.txt"})),
        ];
        assert!(detect_loop(&steps, &config).is_none());
    }

    #[test]
    fn test_custom_message() {
        let config = LoopDetectionConfig {
            max_repeats: 2,
            window_size: 10,
            message: Some("You're stuck! Try something else.".to_string()),
            ..Default::default()
        };
        let steps = vec![
            make_step("bash", json!({"command": "make"})),
            make_step("bash", json!({"command": "make"})),
        ];
        let result = detect_loop(&steps, &config);
        assert_eq!(
            result,
            Some("You're stuck! Try something else.".to_string())
        );
    }

    #[test]
    fn test_different_inputs_not_counted_together() {
        let config = LoopDetectionConfig {
            max_repeats: 3,
            window_size: 10,
            message: None,
            ..Default::default()
        };
        let steps = vec![
            make_step("bash", json!({"command": "cargo test -- test_a"})),
            make_step("bash", json!({"command": "cargo test -- test_b"})),
            make_step("bash", json!({"command": "cargo test -- test_c"})),
        ];
        assert!(detect_loop(&steps, &config).is_none());
    }

    #[test]
    fn test_canonical_hash_key_order_independence() {
        let v1 = json!({"b": 2, "a": 1});
        let v2 = json!({"a": 1, "b": 2});
        assert_eq!(canonical_hash(&v1), canonical_hash(&v2));
    }

    #[test]
    fn test_canonical_hash_nested_objects() {
        let v1 = json!({"outer": {"b": 2, "a": 1}});
        let v2 = json!({"outer": {"a": 1, "b": 2}});
        assert_eq!(canonical_hash(&v1), canonical_hash(&v2));
    }

    #[test]
    fn test_zero_max_repeats_disables_detection() {
        let config = LoopDetectionConfig {
            max_repeats: 0,
            window_size: 10,
            message: None,
            ..Default::default()
        };
        let steps = vec![
            make_step("bash", json!({"command": "ls"})),
            make_step("bash", json!({"command": "ls"})),
            make_step("bash", json!({"command": "ls"})),
        ];
        assert!(detect_loop(&steps, &config).is_none());
    }

    #[test]
    fn test_edit_file_repeated_same_path_detected() {
        let config = LoopDetectionConfig {
            max_repeats: 3,
            window_size: 10,
            message: None,
            ..Default::default()
        };
        let input = json!({"path": "/src/main.rs", "old_string": "foo", "new_string": "bar"});
        let steps = vec![
            make_step("edit_file", input.clone()),
            make_step("edit_file", input.clone()),
            make_step("edit_file", input),
        ];
        let result = detect_loop(&steps, &config);
        assert!(result.is_some());
        assert!(result.unwrap().contains("edit_file"));
    }

    #[test]
    fn test_config_defaults() {
        let config = LoopDetectionConfig::default();
        assert_eq!(config.max_repeats, 3);
        assert_eq!(config.window_size, 10);
        assert!(config.message.is_none());
        assert_eq!(config.max_failures_without_write, 4);
        assert!(config.semantic_loop_message.is_none());
    }

    // --- Semantic loop detection tests ---

    #[test]
    fn test_semantic_loop_no_steps() {
        let config = LoopDetectionConfig::default();
        assert!(detect_semantic_loop(&[], &config).is_none());
    }

    #[test]
    fn test_semantic_loop_disabled_with_zero() {
        let config = LoopDetectionConfig {
            max_failures_without_write: 0,
            ..Default::default()
        };
        let steps = vec![
            make_error_step("bash", json!({"command": "cargo test"})),
            make_error_step("bash", json!({"command": "cargo test"})),
            make_error_step("bash", json!({"command": "cargo test"})),
            make_error_step("bash", json!({"command": "cargo test"})),
        ];
        assert!(detect_semantic_loop(&steps, &config).is_none());
    }

    #[test]
    fn test_semantic_loop_detected() {
        let config = LoopDetectionConfig {
            max_failures_without_write: 4,
            ..Default::default()
        };
        let steps = vec![
            make_error_step("bash", json!({"command": "cargo test"})),
            make_error_step("bash", json!({"command": "cargo test -- test_a"})),
            make_error_step("bash", json!({"command": "cargo build"})),
            make_error_step("bash", json!({"command": "cargo test"})),
        ];
        let result = detect_semantic_loop(&steps, &config);
        assert!(result.is_some());
        let msg = result.unwrap();
        assert!(msg.contains("SEMANTIC LOOP DETECTED"));
        assert!(msg.contains("4"));
    }

    #[test]
    fn test_semantic_loop_broken_by_write_file() {
        let config = LoopDetectionConfig {
            max_failures_without_write: 3,
            ..Default::default()
        };
        let steps = vec![
            make_error_step("bash", json!({"command": "cargo test"})),
            make_error_step("bash", json!({"command": "cargo test"})),
            make_step(
                "write_file",
                json!({"path": "/src/main.rs", "content": "fix"}),
            ),
            make_error_step("bash", json!({"command": "cargo test"})),
            make_error_step("bash", json!({"command": "cargo test"})),
        ];
        assert!(detect_semantic_loop(&steps, &config).is_none());
    }

    #[test]
    fn test_semantic_loop_broken_by_edit_file() {
        let config = LoopDetectionConfig {
            max_failures_without_write: 3,
            ..Default::default()
        };
        let steps = vec![
            make_error_step("bash", json!({"command": "cargo test"})),
            make_error_step("bash", json!({"command": "cargo test"})),
            make_step(
                "edit_file",
                json!({"path": "/src/main.rs", "old": "a", "new": "b"}),
            ),
            make_error_step("bash", json!({"command": "cargo test"})),
            make_error_step("bash", json!({"command": "cargo test"})),
        ];
        assert!(detect_semantic_loop(&steps, &config).is_none());
    }

    #[test]
    fn test_semantic_loop_mixed_success_failure() {
        let config = LoopDetectionConfig {
            max_failures_without_write: 4,
            ..Default::default()
        };
        // 3 failures interspersed with 2 successes = only 3 failures counted
        let steps = vec![
            make_error_step("bash", json!({"command": "cargo test"})),
            make_step("bash", json!({"command": "ls"})),
            make_error_step("bash", json!({"command": "cargo test"})),
            make_step("read_file", json!({"path": "/a.txt"})),
            make_error_step("bash", json!({"command": "cargo test"})),
        ];
        assert!(detect_semantic_loop(&steps, &config).is_none());
    }

    #[test]
    fn test_semantic_loop_custom_message() {
        let config = LoopDetectionConfig {
            max_failures_without_write: 2,
            semantic_loop_message: Some("Fix your code!".to_string()),
            ..Default::default()
        };
        let steps = vec![
            make_error_step("bash", json!({"command": "cargo test"})),
            make_error_step("bash", json!({"command": "cargo test"})),
        ];
        let result = detect_semantic_loop(&steps, &config);
        assert_eq!(result, Some("Fix your code!".to_string()));
    }

    #[test]
    fn test_semantic_loop_respects_window() {
        let config = LoopDetectionConfig {
            max_failures_without_write: 3,
            window_size: 3,
            ..Default::default()
        };
        // 4 failures total, but window only sees last 3
        // The first failure is outside the window
        let steps = vec![
            make_error_step("bash", json!({"command": "cargo test"})),
            make_error_step("bash", json!({"command": "cargo test"})),
            make_step("bash", json!({"command": "ls"})),
            make_error_step("bash", json!({"command": "cargo test"})),
        ];
        // Window sees: [error(cargo test), success(ls), error(cargo test)]
        // Scanning backward: 1 failure, then 0 (success), then 1 failure = only 1 consecutive from end
        let result = detect_semantic_loop(&steps, &config);
        assert!(result.is_none());
    }

    #[test]
    fn test_semantic_loop_all_success() {
        let config = LoopDetectionConfig {
            max_failures_without_write: 2,
            ..Default::default()
        };
        let steps = vec![
            make_step("bash", json!({"command": "cargo test"})),
            make_step("bash", json!({"command": "cargo test"})),
            make_step("bash", json!({"command": "cargo test"})),
        ];
        assert!(detect_semantic_loop(&steps, &config).is_none());
    }

    #[test]
    fn test_is_write_tool() {
        assert!(is_write_tool("write_file"));
        assert!(is_write_tool("edit_file"));
        assert!(!is_write_tool("bash"));
        assert!(!is_write_tool("read_file"));
        assert!(!is_write_tool("grep"));
        assert!(!is_write_tool("navigate"));
    }
}
