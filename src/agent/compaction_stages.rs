use serde::{Deserialize, Serialize};

use crate::config::schema::StageThresholds;
use crate::llm::types::{content_has_tool_use, ContentBlock, Message, Role, ToolResultContent};

/// Compaction stages in order of aggressiveness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum CompactionStage {
    ToolOutputCompression,
    ObservationMerging,
    ConversationSummary,
    LowValuePruning,
}

/// Maximum characters to keep in summarized tool results.
const SUMMARY_TRUNCATE_CHARS: usize = 100;

/// Truncate a string at a safe UTF-8 char boundary.
/// Returns true if truncation occurred.
fn truncate_text(text: &mut String, max_chars: usize) -> bool {
    if text.len() <= max_chars {
        return false;
    }
    let original_len = text.len();
    // Find a safe byte boundary at or before max_chars
    let safe_end = text
        .char_indices()
        .map(|(i, _)| i)
        .take_while(|&i| i <= max_chars)
        .last()
        .unwrap_or(0);
    *text = format!(
        "{}... [truncated from {} to {} chars]",
        &text[..safe_end],
        original_len,
        safe_end
    );
    true
}

/// Truncate a string slice at a safe UTF-8 char boundary, returning the truncated &str.
pub(crate) fn safe_truncate(s: &str, max_chars: usize) -> &str {
    if s.len() <= max_chars {
        return s;
    }
    let safe_end = s
        .char_indices()
        .map(|(i, _)| i)
        .take_while(|&i| i <= max_chars)
        .last()
        .unwrap_or(0);
    &s[..safe_end]
}

/// Determine which compaction stage should be applied based on current token usage.
/// Returns None if no compaction is needed yet.
pub fn determine_stage(
    thresholds: &StageThresholds,
    context_window_tokens: u32,
    current_tokens: u32,
) -> Option<CompactionStage> {
    let ratio = current_tokens as f32 / context_window_tokens as f32;

    if ratio >= thresholds.low_value_pruning {
        Some(CompactionStage::LowValuePruning)
    } else if ratio >= thresholds.conversation_summary {
        Some(CompactionStage::ConversationSummary)
    } else if ratio >= thresholds.observation_merging {
        Some(CompactionStage::ObservationMerging)
    } else if ratio >= thresholds.tool_output_compression {
        Some(CompactionStage::ToolOutputCompression)
    } else {
        None
    }
}

/// Stage 1: Compress large tool output results in-place.
/// Truncates ToolResult text content that exceeds `max_chars` to `max_chars` chars + truncation marker.
///
/// The most recent `preserve_recent_n` messages are left untouched. Without that guard
/// this truncates the tool result the model is about to reason over — including the one
/// produced in the previous iteration — which is exactly the context it still needs.
pub fn compress_tool_outputs(
    messages: &mut [Message],
    max_chars: usize,
    preserve_recent_n: usize,
) -> usize {
    let mut compressed_count = 0;
    let compressible_end = messages.len().saturating_sub(preserve_recent_n);
    for msg in messages.iter_mut().take(compressible_end) {
        for block in msg.content.iter_mut() {
            if let ContentBlock::ToolResult { content, .. } = block {
                match content {
                    ToolResultContent::Text(text) => {
                        if truncate_text(text, max_chars) {
                            compressed_count += 1;
                        }
                    }
                    ToolResultContent::Blocks(blocks) => {
                        for inner_block in blocks.iter_mut() {
                            if let ContentBlock::Text { text } = inner_block {
                                if truncate_text(text, max_chars) {
                                    compressed_count += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    compressed_count
}

/// Detect whether a message contains a ToolUse block (assistant) or ToolResult block (user).
fn is_tool_use_msg(msg: &Message) -> bool {
    matches!(msg.role, Role::Assistant) && content_has_tool_use(&msg.content)
}

fn is_tool_result_msg(msg: &Message) -> bool {
    matches!(msg.role, Role::User)
        && msg
            .content
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolResult { .. }))
}

fn is_all_tool_results(msg: &Message) -> bool {
    matches!(msg.role, Role::User)
        && msg
            .content
            .iter()
            .all(|b| matches!(b, ContentBlock::ToolResult { .. }))
}

/// Walk messages up to `preserve_recent_n` from the end, applying `on_pair` to
/// each consecutive ToolUse->ToolResult pair. The closure returns replacement
/// messages: empty to drop the pair, one to merge, two to keep both.
/// Non-pair messages are kept as-is.
fn process_tool_pairs<F>(messages: &[Message], preserve_recent_n: usize, on_pair: F) -> Vec<Message>
where
    F: Fn(&Message, &Message) -> Vec<Message>,
{
    if messages.len() <= preserve_recent_n {
        return messages.to_vec();
    }

    let boundary = messages.len() - preserve_recent_n;
    let mut result: Vec<Message> = Vec::new();
    let mut i = 0;

    while i < boundary {
        if i + 1 < boundary && is_tool_use_msg(&messages[i]) && is_tool_result_msg(&messages[i + 1])
        {
            let replacements = on_pair(&messages[i], &messages[i + 1]);
            result.extend(replacements);
            i += 2;
        } else {
            result.push(messages[i].clone());
            i += 1;
        }
    }

    result.extend_from_slice(&messages[boundary..]);
    result
}

/// Stage 2: Merge consecutive ToolUse -> ToolResult pairs into condensed text summaries.
/// Returns a new message list with merged observations.
/// Only merges pairs in the first `messages.len() - preserve_recent_n` messages (preserves recent messages).
pub fn merge_observations(messages: &[Message], preserve_recent_n: usize) -> Vec<Message> {
    process_tool_pairs(messages, preserve_recent_n, |tool_use, tool_result| {
        let summary = summarize_tool_pair(tool_use, tool_result);
        vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text { text: summary }],
        }]
    })
}

/// Summarize a ToolUse + ToolResult pair into a condensed text.
fn summarize_tool_pair(tool_use_msg: &Message, tool_result_msg: &Message) -> String {
    let mut parts = Vec::new();

    for block in &tool_use_msg.content {
        if let ContentBlock::ToolUse { name, .. } = block {
            parts.push(format!("Called '{name}'"));
        }
    }

    for block in &tool_result_msg.content {
        if let ContentBlock::ToolResult {
            content, is_error, ..
        } = block
        {
            let error_tag = if matches!(is_error, Some(true)) {
                " (ERROR)"
            } else {
                ""
            };
            let text = match content {
                ToolResultContent::Text(s) => {
                    if s.len() > SUMMARY_TRUNCATE_CHARS {
                        format!("{}...", safe_truncate(s, SUMMARY_TRUNCATE_CHARS))
                    } else {
                        s.clone()
                    }
                }
                ToolResultContent::Blocks(blocks) => blocks
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
            };
            parts.push(format!("-> {text}{error_tag}"));
        }
    }

    format!("[Observation] {}", parts.join("; "))
}

/// Stage 4: Score and prune low-value exchanges.
/// Returns a new message list with low-value exchanges removed.
/// An exchange is "low-value" if the tool result is trivially short and not an error.
pub fn prune_low_value(
    messages: &[Message],
    preserve_recent_n: usize,
    min_value_chars: usize,
) -> Vec<Message> {
    process_tool_pairs(messages, preserve_recent_n, |tool_use, tool_result| {
        // prune_low_value requires ALL blocks to be ToolResult (stricter check)
        if !is_all_tool_results(tool_result) {
            return vec![tool_use.clone(), tool_result.clone()];
        }
        if is_low_value_exchange(tool_result, min_value_chars) {
            vec![] // Drop low-value pair
        } else {
            vec![tool_use.clone(), tool_result.clone()] // Keep both
        }
    })
}

/// Determine if a tool result message represents a low-value exchange.
/// Low-value = all results are short text, not errors, and contain no meaningful findings.
fn is_low_value_exchange(result_msg: &Message, min_value_chars: usize) -> bool {
    for block in &result_msg.content {
        if let ContentBlock::ToolResult {
            content, is_error, ..
        } = block
        {
            // Errors are always valuable
            if matches!(is_error, Some(true)) {
                return false;
            }
            match content {
                ToolResultContent::Text(s) => {
                    if s.len() >= min_value_chars {
                        return false;
                    }
                }
                ToolResultContent::Blocks(blocks) => {
                    let total_len: usize = blocks
                        .iter()
                        .map(|b| match b {
                            ContentBlock::Text { text } => text.len(),
                            _ => min_value_chars, // non-text blocks are considered valuable
                        })
                        .sum();
                    if total_len >= min_value_chars {
                        return false;
                    }
                }
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    fn make_user_text(text: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
        }
    }

    fn make_assistant_text(text: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
        }
    }

    fn make_assistant_tool_use(tool_name: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: format!("toolu_{tool_name}"),
                name: tool_name.to_string(),
                input: json!({}),
            }],
        }
    }

    fn make_user_tool_result(tool_use_id: &str, result_text: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: tool_use_id.to_string(),
                content: ToolResultContent::Text(result_text.to_string()),
                is_error: None,
            }],
        }
    }

    fn make_user_tool_result_error(tool_use_id: &str, result_text: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: tool_use_id.to_string(),
                content: ToolResultContent::Text(result_text.to_string()),
                is_error: Some(true),
            }],
        }
    }

    fn make_user_tool_result_blocks(tool_use_id: &str, texts: &[&str]) -> Message {
        let blocks = texts
            .iter()
            .map(|t| ContentBlock::Text {
                text: t.to_string(),
            })
            .collect();
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: tool_use_id.to_string(),
                content: ToolResultContent::Blocks(blocks),
                is_error: None,
            }],
        }
    }

    /// Helper to generate a string of a given length.
    fn make_string(len: usize) -> String {
        "x".repeat(len)
    }

    // -----------------------------------------------------------------------
    // determine_stage tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_determine_stage_returns_none_below_all_thresholds() {
        let thresholds = StageThresholds::default();
        // 50% usage -> below 0.60 threshold
        let result = determine_stage(&thresholds, 1000, 500);
        assert_eq!(result, None);
    }

    #[test]
    fn test_determine_stage_returns_tool_output_compression() {
        let thresholds = StageThresholds::default();
        // 65% usage -> above 0.60, below 0.75
        let result = determine_stage(&thresholds, 1000, 650);
        assert_eq!(result, Some(CompactionStage::ToolOutputCompression));
    }

    #[test]
    fn test_determine_stage_returns_observation_merging() {
        let thresholds = StageThresholds::default();
        // 80% usage -> above 0.75, below 0.85
        let result = determine_stage(&thresholds, 1000, 800);
        assert_eq!(result, Some(CompactionStage::ObservationMerging));
    }

    #[test]
    fn test_determine_stage_returns_conversation_summary() {
        let thresholds = StageThresholds::default();
        // 90% usage -> above 0.85, below 0.95
        let result = determine_stage(&thresholds, 1000, 900);
        assert_eq!(result, Some(CompactionStage::ConversationSummary));
    }

    #[test]
    fn test_determine_stage_returns_low_value_pruning() {
        let thresholds = StageThresholds::default();
        // 96% usage -> above 0.95
        let result = determine_stage(&thresholds, 1000, 960);
        assert_eq!(result, Some(CompactionStage::LowValuePruning));
    }

    #[test]
    fn test_stage_thresholds_default() {
        let thresholds = StageThresholds::default();
        assert!((thresholds.tool_output_compression - 0.60).abs() < f32::EPSILON);
        assert!((thresholds.observation_merging - 0.75).abs() < f32::EPSILON);
        assert!((thresholds.conversation_summary - 0.85).abs() < f32::EPSILON);
        assert!((thresholds.low_value_pruning - 0.95).abs() < f32::EPSILON);
    }

    // -----------------------------------------------------------------------
    // compress_tool_outputs tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_compress_tool_outputs_truncates_long_results() {
        let long_text = make_string(600);
        let mut messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "toolu_1".to_string(),
                content: ToolResultContent::Text(long_text),
                is_error: None,
            }],
        }];

        compress_tool_outputs(&mut messages, 500, 0);

        if let ContentBlock::ToolResult { content, .. } = &messages[0].content[0] {
            if let ToolResultContent::Text(text) = content {
                assert!(text.contains("... [truncated from 600 to 500 chars]"));
                // The text should start with 500 'x' chars
                assert!(text.starts_with(&make_string(500)));
            } else {
                panic!("Expected Text content");
            }
        } else {
            panic!("Expected ToolResult block");
        }
    }

    #[test]
    fn test_compress_tool_outputs_leaves_short_results() {
        let short_text = make_string(100);
        let mut messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "toolu_1".to_string(),
                content: ToolResultContent::Text(short_text.clone()),
                is_error: None,
            }],
        }];

        let count = compress_tool_outputs(&mut messages, 500, 0);
        assert_eq!(count, 0);

        if let ContentBlock::ToolResult { content, .. } = &messages[0].content[0] {
            if let ToolResultContent::Text(text) = content {
                assert_eq!(text, &short_text);
            } else {
                panic!("Expected Text content");
            }
        } else {
            panic!("Expected ToolResult block");
        }
    }

    #[test]
    fn test_compress_tool_outputs_returns_count() {
        let long1 = make_string(600);
        let long2 = make_string(700);
        let short = make_string(100);
        let mut messages = vec![
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "toolu_1".to_string(),
                    content: ToolResultContent::Text(long1),
                    is_error: None,
                }],
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "toolu_2".to_string(),
                    content: ToolResultContent::Text(short),
                    is_error: None,
                }],
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "toolu_3".to_string(),
                    content: ToolResultContent::Text(long2),
                    is_error: None,
                }],
            },
        ];

        let count = compress_tool_outputs(&mut messages, 500, 0);
        assert_eq!(count, 2);
    }

    // -----------------------------------------------------------------------
    // merge_observations tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_merge_observations_combines_tool_pairs() {
        let messages = vec![
            make_assistant_tool_use("navigate"),
            make_user_tool_result("toolu_navigate", "Page loaded"),
            make_assistant_tool_use("click"),
            make_user_tool_result("toolu_click", "Button clicked"),
            // recent messages that will be preserved
            make_user_text("What happened?"),
        ];

        let result = merge_observations(&messages, 1);

        // The 4 tool messages should be merged into 2, plus 1 preserved = 3 total
        assert_eq!(result.len(), 3);

        // First merged message
        if let ContentBlock::Text { text } = &result[0].content[0] {
            assert!(text.contains("[Observation]"));
            assert!(text.contains("Called 'navigate'"));
            assert!(text.contains("Page loaded"));
        } else {
            panic!("Expected Text block in merged message");
        }

        // Second merged message
        if let ContentBlock::Text { text } = &result[1].content[0] {
            assert!(text.contains("[Observation]"));
            assert!(text.contains("Called 'click'"));
            assert!(text.contains("Button clicked"));
        } else {
            panic!("Expected Text block in merged message");
        }
    }

    #[test]
    fn test_merge_observations_preserves_recent() {
        let messages = vec![
            make_assistant_tool_use("navigate"),
            make_user_tool_result("toolu_navigate", "Page loaded"),
            make_assistant_text("I navigated to the page"),
            make_user_text("Thanks!"),
        ];

        // Preserve last 2 messages
        let result = merge_observations(&messages, 2);

        // First pair merged into 1, plus 2 preserved = 3 total
        assert_eq!(result.len(), 3);

        // Last two messages should be the preserved ones
        assert!(matches!(result[1].role, Role::Assistant));
        assert!(matches!(result[2].role, Role::User));
    }

    #[test]
    fn test_merge_observations_handles_non_tool_messages() {
        let messages = vec![
            make_user_text("Hello"),
            make_assistant_text("Hi there!"),
            make_user_text("How are you?"),
            make_assistant_text("I'm fine."),
        ];

        let result = merge_observations(&messages, 1);

        // No tool pairs to merge, so first 3 should pass through, last 1 preserved
        assert_eq!(result.len(), 4);
    }

    #[test]
    fn test_summarize_tool_pair_formats_correctly() {
        let tool_use_msg = make_assistant_tool_use("screenshot");
        let tool_result_msg = make_user_tool_result("toolu_screenshot", "Image captured OK");

        let summary = summarize_tool_pair(&tool_use_msg, &tool_result_msg);

        assert!(summary.starts_with("[Observation]"));
        assert!(summary.contains("Called 'screenshot'"));
        assert!(summary.contains("-> Image captured OK"));
    }

    // -----------------------------------------------------------------------
    // prune_low_value tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_prune_low_value_removes_trivial_exchanges() {
        let messages = vec![
            make_assistant_tool_use("check_status"),
            make_user_tool_result("toolu_check_status", "ok"), // 2 chars - trivial
            make_user_text("What next?"),
        ];

        // Preserve last 1, min_value_chars = 50
        let result = prune_low_value(&messages, 1, 50);

        // The trivial pair should be pruned, only preserved msg remains
        assert_eq!(result.len(), 1);
        if let ContentBlock::Text { text } = &result[0].content[0] {
            assert_eq!(text, "What next?");
        } else {
            panic!("Expected Text block");
        }
    }

    #[test]
    fn test_prune_low_value_keeps_errors() {
        let messages = vec![
            make_assistant_tool_use("navigate"),
            make_user_tool_result_error("toolu_navigate", "err"), // short but error
            make_user_text("Continue"),
        ];

        let result = prune_low_value(&messages, 1, 50);

        // Error exchange should be kept: tool_use + tool_result + preserved = 3
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_prune_low_value_keeps_substantial_results() {
        let substantial_text = make_string(200);
        let messages = vec![
            make_assistant_tool_use("read_file"),
            make_user_tool_result("toolu_read_file", &substantial_text),
            make_user_text("Analyze this"),
        ];

        let result = prune_low_value(&messages, 1, 50);

        // Substantial result should be kept: tool_use + tool_result + preserved = 3
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_prune_low_value_preserves_recent() {
        let messages = vec![
            make_assistant_tool_use("ping"),
            make_user_tool_result("toolu_ping", "ok"), // trivial
            make_assistant_tool_use("status"),
            make_user_tool_result("toolu_status", "ok"), // trivial
            // These two are in the preserve window
            make_assistant_tool_use("recent_ping"),
            make_user_tool_result("toolu_recent_ping", "ok"), // trivial but preserved
        ];

        let result = prune_low_value(&messages, 2, 50);

        // First 2 trivial pairs pruned, last 2 messages preserved
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_is_low_value_exchange_with_error() {
        let error_msg = make_user_tool_result_error("toolu_1", "fail");
        assert!(!is_low_value_exchange(&error_msg, 50));
    }

    // -----------------------------------------------------------------------
    // Additional edge-case tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_determine_stage_at_exact_boundary() {
        let thresholds = StageThresholds::default();
        // Exactly at 0.60 boundary
        let result = determine_stage(&thresholds, 100, 60);
        assert_eq!(result, Some(CompactionStage::ToolOutputCompression));
    }

    #[test]
    fn test_compress_tool_outputs_handles_blocks_variant() {
        let long_text = make_string(600);
        let mut messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "toolu_1".to_string(),
                content: ToolResultContent::Blocks(vec![ContentBlock::Text { text: long_text }]),
                is_error: None,
            }],
        }];

        let count = compress_tool_outputs(&mut messages, 500, 0);
        assert_eq!(count, 1);

        if let ContentBlock::ToolResult { content, .. } = &messages[0].content[0] {
            if let ToolResultContent::Blocks(blocks) = content {
                if let ContentBlock::Text { text } = &blocks[0] {
                    assert!(text.contains("... [truncated from 600 to 500 chars]"));
                } else {
                    panic!("Expected Text block inside Blocks");
                }
            } else {
                panic!("Expected Blocks content");
            }
        } else {
            panic!("Expected ToolResult block");
        }
    }

    #[test]
    fn test_merge_observations_with_empty_messages() {
        let messages: Vec<Message> = vec![];
        let result = merge_observations(&messages, 0);
        assert!(result.is_empty());
    }

    #[test]
    fn test_merge_observations_all_preserved() {
        let messages = vec![
            make_assistant_tool_use("navigate"),
            make_user_tool_result("toolu_navigate", "done"),
        ];

        // preserve_recent_n >= messages.len() -> all preserved as-is
        let result = merge_observations(&messages, 5);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_prune_low_value_with_blocks_content() {
        let messages = vec![
            make_assistant_tool_use("list_files"),
            make_user_tool_result_blocks("toolu_list_files", &["a", "b"]), // 2 chars total - trivial
            make_user_text("Next"),
        ];

        let result = prune_low_value(&messages, 1, 50);

        // Trivial blocks-based result should be pruned
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_summarize_tool_pair_truncates_long_result() {
        let long_result = make_string(200);
        let tool_use_msg = make_assistant_tool_use("read_file");
        let tool_result_msg = make_user_tool_result("toolu_read_file", &long_result);

        let summary = summarize_tool_pair(&tool_use_msg, &tool_result_msg);

        assert!(summary.contains("[Observation]"));
        assert!(summary.contains("Called 'read_file'"));
        // Result should be truncated to 100 chars + "..."
        assert!(summary.contains("..."));
        // The full 200-char string should NOT be in the summary
        assert!(!summary.contains(&long_result));
    }

    #[test]
    fn test_stage_thresholds_serde_roundtrip() {
        let thresholds = StageThresholds::default();
        let json = serde_json::to_string(&thresholds).unwrap();
        let deserialized: StageThresholds = serde_json::from_str(&json).unwrap();
        assert!((deserialized.tool_output_compression - 0.60).abs() < f32::EPSILON);
        assert!((deserialized.observation_merging - 0.75).abs() < f32::EPSILON);
        assert!((deserialized.conversation_summary - 0.85).abs() < f32::EPSILON);
        assert!((deserialized.low_value_pruning - 0.95).abs() < f32::EPSILON);
    }

    #[test]
    fn test_compaction_stage_ordering() {
        assert!(CompactionStage::ToolOutputCompression < CompactionStage::ObservationMerging);
        assert!(CompactionStage::ObservationMerging < CompactionStage::ConversationSummary);
        assert!(CompactionStage::ConversationSummary < CompactionStage::LowValuePruning);
    }

    #[test]
    fn test_summarize_tool_pair_with_error_result() {
        let tool_use_msg = make_assistant_tool_use("navigate");
        let tool_result_msg = make_user_tool_result_error("toolu_navigate", "404 Not Found");

        let summary = summarize_tool_pair(&tool_use_msg, &tool_result_msg);

        assert!(summary.contains("(ERROR)"));
        assert!(summary.contains("404 Not Found"));
    }

    #[test]
    fn test_is_low_value_exchange_with_valuable_blocks() {
        let valuable_msg = make_user_tool_result_blocks("toolu_1", &[&make_string(200)]);
        assert!(!is_low_value_exchange(&valuable_msg, 50));
    }

    #[test]
    fn test_compress_tool_outputs_skips_non_tool_result_blocks() {
        let mut messages = vec![make_user_text("Hello"), make_assistant_text("World")];

        let count = compress_tool_outputs(&mut messages, 500, 0);
        assert_eq!(count, 0);
    }
}
