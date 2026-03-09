use crate::config::schema::CompactionConfig;
use crate::llm::types::{ContentBlock, Message, Role, ToolResultContent};

/// Result of a compaction operation.
#[derive(Debug, Clone)]
pub struct CompactionResult {
    pub summary_text: String,
    pub messages_removed: usize,
    pub tokens_before: u32,
    pub tokens_after_estimate: u32,
}

/// Check if compaction should be triggered based on accumulated token usage.
pub fn should_compact(config: &CompactionConfig, total_input_tokens: u32) -> bool {
    if !config.enabled {
        return false;
    }
    let threshold = (config.context_window_tokens as f32 * config.trigger_threshold) as u32;
    total_input_tokens >= threshold
}

/// Build messages to send to the LLM for summarization.
/// Takes the messages to compact and returns a new message list with the compaction prompt.
pub fn build_compaction_request(messages_to_compact: &[Message]) -> Vec<Message> {
    let mut conversation_text = String::new();
    for msg in messages_to_compact {
        let role_str = match msg.role {
            Role::User => "User",
            Role::Assistant => "Assistant",
        };
        for block in &msg.content {
            match block {
                ContentBlock::Text { text } => {
                    conversation_text.push_str(&format!("[{role_str}]: {text}\n\n"));
                }
                ContentBlock::ToolUse { name, input, .. } => {
                    conversation_text.push_str(&format!(
                        "[{role_str} used tool '{name}']: {}\n\n",
                        serde_json::to_string(input).unwrap_or_default()
                    ));
                }
                ContentBlock::Image { .. } => {
                    conversation_text.push_str(&format!("[{role_str} sent image]\n\n"));
                }
                ContentBlock::Thinking { thinking } => {
                    conversation_text.push_str(&format!(
                        "[{role_str} thinking]: {}\n\n",
                        if thinking.len() > 200 {
                            format!("{}...", &thinking[..200])
                        } else {
                            thinking.clone()
                        }
                    ));
                }
                ContentBlock::ToolResult {
                    content, is_error, ..
                } => {
                    let error_marker = if is_error == &Some(true) {
                        " (ERROR)"
                    } else {
                        ""
                    };
                    let content_str = match content {
                        ToolResultContent::Text(s) => s.clone(),
                        ToolResultContent::Blocks(blocks) => {
                            blocks
                                .iter()
                                .filter_map(|b| {
                                    if let ContentBlock::Text { text } = b {
                                        Some(text.as_str())
                                    } else {
                                        None
                                    }
                                })
                                .collect::<Vec<_>>()
                                .join("\n")
                        }
                    };
                    let truncated = if content_str.len() > 500 {
                        format!("{}... [truncated]", &content_str[..500])
                    } else {
                        content_str
                    };
                    conversation_text
                        .push_str(&format!("[Tool Result{error_marker}]: {truncated}\n\n"));
                }
            }
        }
    }

    vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: format!("Please summarize the following conversation:\n\n{conversation_text}"),
        }],
    }]
}

/// Create a summary message from the LLM's summary response.
/// Wraps the summary in `<summary>` XML tags for clear identification.
pub fn create_summary_message(summary_text: &str) -> Message {
    Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: format!("<summary>\n{summary_text}\n</summary>"),
        }],
    }
}

/// Perform the compaction: split messages into "to compact" and "to preserve",
/// return the indices for slicing.
/// Returns `(compact_end_index, preserve_start_index)`.
/// `Messages[0..compact_end]` will be compacted.
/// `Messages[preserve_start..]` will be preserved.
pub fn compute_compaction_split(messages_len: usize, preserve_recent_n: usize) -> (usize, usize) {
    if messages_len <= preserve_recent_n {
        // Nothing to compact
        return (0, 0);
    }
    let preserve_start = messages_len - preserve_recent_n;
    (preserve_start, preserve_start)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_config(enabled: bool, threshold: f32, context_window: u32) -> CompactionConfig {
        CompactionConfig {
            enabled,
            trigger_threshold: threshold,
            context_window_tokens: context_window,
            preserve_recent_n: 4,
        }
    }

    // --- should_compact tests ---

    #[test]
    fn test_should_compact_returns_false_when_disabled() {
        let config = make_config(false, 0.95, 200_000);
        assert!(!should_compact(&config, 999_999));
    }

    #[test]
    fn test_should_compact_returns_false_when_below_threshold() {
        let config = make_config(true, 0.95, 200_000);
        // threshold = 200_000 * 0.95 = 190_000
        assert!(!should_compact(&config, 100_000));
    }

    #[test]
    fn test_should_compact_returns_true_when_above_threshold() {
        let config = make_config(true, 0.95, 200_000);
        // threshold = 190_000
        assert!(should_compact(&config, 195_000));
    }

    #[test]
    fn test_should_compact_at_exact_threshold() {
        let config = make_config(true, 0.50, 100_000);
        // threshold = 50_000
        assert!(should_compact(&config, 50_000));
    }

    #[test]
    fn test_should_compact_with_zero_tokens() {
        let config = make_config(true, 0.95, 200_000);
        assert!(!should_compact(&config, 0));
    }

    #[test]
    fn test_should_compact_just_below_threshold() {
        let config = make_config(true, 0.50, 100_000);
        // threshold = 50_000
        assert!(!should_compact(&config, 49_999));
    }

    // --- build_compaction_request tests ---

    #[test]
    fn test_build_compaction_request_with_text_messages() {
        let messages = vec![
            Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "Hello".to_string(),
                }],
            },
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::Text {
                    text: "Hi there!".to_string(),
                }],
            },
        ];
        let result = build_compaction_request(&messages);
        assert_eq!(result.len(), 1);
        assert!(matches!(result[0].role, Role::User));
        match &result[0].content[0] {
            ContentBlock::Text { text } => {
                assert!(text.contains("[User]: Hello"));
                assert!(text.contains("[Assistant]: Hi there!"));
                assert!(text.contains("Please summarize"));
            }
            _ => panic!("Expected Text content block"),
        }
    }

    #[test]
    fn test_build_compaction_request_with_tool_use() {
        let messages = vec![Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "toolu_01".to_string(),
                name: "navigate".to_string(),
                input: json!({"url": "https://example.com"}),
            }],
        }];
        let result = build_compaction_request(&messages);
        match &result[0].content[0] {
            ContentBlock::Text { text } => {
                assert!(text.contains("[Assistant used tool 'navigate']"));
                assert!(text.contains("https://example.com"));
            }
            _ => panic!("Expected Text content block"),
        }
    }

    #[test]
    fn test_build_compaction_request_with_tool_result() {
        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "toolu_01".to_string(),
                content: ToolResultContent::Text("Page loaded successfully".to_string()),
                is_error: None,
            }],
        }];
        let result = build_compaction_request(&messages);
        match &result[0].content[0] {
            ContentBlock::Text { text } => {
                assert!(text.contains("[Tool Result]: Page loaded successfully"));
                assert!(!text.contains("(ERROR)"));
            }
            _ => panic!("Expected Text content block"),
        }
    }

    #[test]
    fn test_build_compaction_request_with_error_tool_result() {
        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "toolu_01".to_string(),
                content: ToolResultContent::Text("Connection refused".to_string()),
                is_error: Some(true),
            }],
        }];
        let result = build_compaction_request(&messages);
        match &result[0].content[0] {
            ContentBlock::Text { text } => {
                assert!(text.contains("[Tool Result (ERROR)]: Connection refused"));
            }
            _ => panic!("Expected Text content block"),
        }
    }

    #[test]
    fn test_build_compaction_request_truncates_long_tool_results() {
        let long_content = "x".repeat(600);
        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "toolu_01".to_string(),
                content: ToolResultContent::Text(long_content),
                is_error: None,
            }],
        }];
        let result = build_compaction_request(&messages);
        match &result[0].content[0] {
            ContentBlock::Text { text } => {
                assert!(text.contains("... [truncated]"));
                // The truncated portion should be 500 chars of 'x'
                assert!(text.contains(&"x".repeat(500)));
                // But not 600
                assert!(!text.contains(&"x".repeat(600)));
            }
            _ => panic!("Expected Text content block"),
        }
    }

    #[test]
    fn test_build_compaction_request_with_empty_messages() {
        let messages: Vec<Message> = vec![];
        let result = build_compaction_request(&messages);
        assert_eq!(result.len(), 1);
        match &result[0].content[0] {
            ContentBlock::Text { text } => {
                assert!(text.contains("Please summarize"));
            }
            _ => panic!("Expected Text content block"),
        }
    }

    #[test]
    fn test_build_compaction_request_with_mixed_content_blocks() {
        let messages = vec![
            Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "Navigate to example.com".to_string(),
                }],
            },
            Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::Text {
                        text: "I'll navigate there.".to_string(),
                    },
                    ContentBlock::ToolUse {
                        id: "toolu_01".to_string(),
                        name: "navigate".to_string(),
                        input: json!({"url": "https://example.com"}),
                    },
                ],
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "toolu_01".to_string(),
                    content: ToolResultContent::Text("OK".to_string()),
                    is_error: None,
                }],
            },
        ];
        let result = build_compaction_request(&messages);
        match &result[0].content[0] {
            ContentBlock::Text { text } => {
                assert!(text.contains("[User]: Navigate to example.com"));
                assert!(text.contains("[Assistant]: I'll navigate there."));
                assert!(text.contains("[Assistant used tool 'navigate']"));
                assert!(text.contains("[Tool Result]: OK"));
            }
            _ => panic!("Expected Text content block"),
        }
    }

    // --- create_summary_message tests ---

    #[test]
    fn test_create_summary_message_wraps_in_summary_tags() {
        let msg = create_summary_message("The user navigated to example.com.");
        assert!(matches!(msg.role, Role::User));
        assert_eq!(msg.content.len(), 1);
        match &msg.content[0] {
            ContentBlock::Text { text } => {
                assert!(text.starts_with("<summary>"));
                assert!(text.ends_with("</summary>"));
                assert!(text.contains("The user navigated to example.com."));
            }
            _ => panic!("Expected Text content block"),
        }
    }

    #[test]
    fn test_create_summary_message_with_empty_summary() {
        let msg = create_summary_message("");
        match &msg.content[0] {
            ContentBlock::Text { text } => {
                assert_eq!(text, "<summary>\n\n</summary>");
            }
            _ => panic!("Expected Text content block"),
        }
    }

    // --- compute_compaction_split tests ---

    #[test]
    fn test_compute_compaction_split_normal() {
        // 10 messages, preserve 4 => compact first 6
        let (compact_end, preserve_start) = compute_compaction_split(10, 4);
        assert_eq!(compact_end, 6);
        assert_eq!(preserve_start, 6);
    }

    #[test]
    fn test_compute_compaction_split_messages_equal_preserve() {
        // 4 messages, preserve 4 => nothing to compact
        let (compact_end, preserve_start) = compute_compaction_split(4, 4);
        assert_eq!(compact_end, 0);
        assert_eq!(preserve_start, 0);
    }

    #[test]
    fn test_compute_compaction_split_messages_less_than_preserve() {
        // 2 messages, preserve 4 => nothing to compact
        let (compact_end, preserve_start) = compute_compaction_split(2, 4);
        assert_eq!(compact_end, 0);
        assert_eq!(preserve_start, 0);
    }

    #[test]
    fn test_compute_compaction_split_preserve_zero() {
        // 5 messages, preserve 0 => compact all
        let (compact_end, preserve_start) = compute_compaction_split(5, 0);
        assert_eq!(compact_end, 5);
        assert_eq!(preserve_start, 5);
    }

    #[test]
    fn test_compute_compaction_split_single_message_no_preserve() {
        let (compact_end, preserve_start) = compute_compaction_split(1, 0);
        assert_eq!(compact_end, 1);
        assert_eq!(preserve_start, 1);
    }

    #[test]
    fn test_compute_compaction_split_zero_messages() {
        let (compact_end, preserve_start) = compute_compaction_split(0, 4);
        assert_eq!(compact_end, 0);
        assert_eq!(preserve_start, 0);
    }

    // --- CompactionResult tests ---

    #[test]
    fn test_compaction_result_clone() {
        let result = CompactionResult {
            summary_text: "Summary here".to_string(),
            messages_removed: 8,
            tokens_before: 150_000,
            tokens_after_estimate: 5_000,
        };
        let cloned = result.clone();
        assert_eq!(cloned.summary_text, "Summary here");
        assert_eq!(cloned.messages_removed, 8);
        assert_eq!(cloned.tokens_before, 150_000);
        assert_eq!(cloned.tokens_after_estimate, 5_000);
    }
}
