use serde_json::Value;
use tokio::sync::oneshot;

use crate::error::AgentError;
use crate::llm::stream::{ContentBlockStartData, Delta, MessageStartUsage, StreamEvent};
use crate::llm::types::{ContentBlock, MessagesResponse, StopReason, Usage};

/// Messages sent from TUI to the agent loop.
#[derive(Debug, Clone, PartialEq)]
pub enum UserMessage {
    /// Normal user chat message.
    Chat(String),
    /// Interrupt current agent turn (Ctrl+C).
    Interrupt,
    /// Exit the session.
    Quit,
}

/// Signals sent from the agent loop to the TUI.
#[derive(Debug)]
pub enum InteractiveSignal {
    /// Agent finished a turn, ready for user input.
    AwaitingInput,
    /// Agent needs permission to execute a tool.
    PermissionRequest {
        tool_name: String,
        tool_input: Value,
        respond: oneshot::Sender<bool>,
    },
}

/// Incremental deltas emitted by `StreamAssembler` for TUI rendering.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamDelta {
    /// Incremental text content from the model.
    TextDelta(String),
    /// Incremental thinking/reasoning text from the model.
    ThinkingDelta(String),
    /// A tool_use content block has started.
    ToolUseStart { id: String, name: String },
    /// Incremental JSON input for the current tool_use block.
    InputJsonDelta(String),
    /// Updated token usage and cost information.
    UsageUpdate {
        input_tokens: u32,
        output_tokens: u32,
        cache_read: Option<u32>,
        cache_write: Option<u32>,
        cost: Option<f64>,
    },
}

/// Tracks the state of a single content block being assembled from stream events.
#[derive(Debug)]
enum ContentBlockBuilder {
    Text(String),
    ToolUse {
        id: String,
        name: String,
        input_json: String,
    },
    Thinking {
        thinking: String,
        signature: String,
    },
    RedactedThinking(String),
}

impl ContentBlockBuilder {
    fn into_content_block(self) -> ContentBlock {
        match self {
            ContentBlockBuilder::Text(text) => ContentBlock::Text { text },
            ContentBlockBuilder::ToolUse {
                id,
                name,
                input_json,
            } => {
                // Falling back to `{}` here dispatched the tool with no arguments at
                // all, which looks to the model like the tool ignored its request.
                // Surface the malformed JSON instead.
                let input: Value = serde_json::from_str(&input_json).unwrap_or_else(|e| {
                    tracing::error!(
                        tool = %name,
                        error = %e,
                        raw = %input_json,
                        "Streamed tool input was not valid JSON"
                    );
                    serde_json::json!({
                        "__malformed_tool_input": input_json,
                        "__parse_error": e.to_string(),
                    })
                });
                ContentBlock::ToolUse { id, name, input }
            }
            ContentBlockBuilder::Thinking {
                thinking,
                signature,
            } => ContentBlock::Thinking {
                thinking,
                signature,
            },
            ContentBlockBuilder::RedactedThinking(data) => ContentBlock::RedactedThinking { data },
        }
    }
}

/// Accumulates `StreamEvent`s into a final `MessagesResponse`.
///
/// As events arrive from the streaming API, the assembler tracks content blocks,
/// usage data, and stop reason. It can produce a final response when the stream
/// completes or a partial response on interrupt.
#[derive(Debug)]
pub struct StreamAssembler {
    message_id: String,
    model: String,
    content_blocks: Vec<ContentBlockBuilder>,
    stop_reason: Option<StopReason>,
    input_usage: Option<MessageStartUsage>,
    output_tokens: u32,
    /// An error the server sent mid-stream, so `finish` can report the real cause
    /// rather than the missing stop reason it produces as a side effect.
    stream_error: Option<String>,
}

impl StreamAssembler {
    /// Create an empty assembler ready to process stream events.
    pub fn new() -> Self {
        Self {
            stream_error: None,
            message_id: String::new(),
            model: String::new(),
            content_blocks: Vec::new(),
            stop_reason: None,
            input_usage: None,
            output_tokens: 0,
        }
    }

    /// Process a single stream event, returning any deltas for TUI rendering.
    pub fn process_event(&mut self, event: &StreamEvent) -> Vec<StreamDelta> {
        let mut deltas = Vec::new();

        match event {
            StreamEvent::MessageStart { message } => {
                self.message_id = message.id.clone();
                self.model = message.model.clone();
                if let Some(ref usage) = message.usage {
                    self.input_usage = Some(usage.clone());
                    deltas.push(StreamDelta::UsageUpdate {
                        input_tokens: usage.input_tokens,
                        output_tokens: usage.output_tokens,
                        cache_read: usage.cache_read_input_tokens,
                        cache_write: usage.cache_creation_input_tokens,
                        cost: None,
                    });
                }
            }
            StreamEvent::ContentBlockStart {
                index: _,
                content_block,
            } => match content_block {
                ContentBlockStartData::Text { text } => {
                    self.content_blocks
                        .push(ContentBlockBuilder::Text(text.clone()));
                    if !text.is_empty() {
                        deltas.push(StreamDelta::TextDelta(text.clone()));
                    }
                }
                ContentBlockStartData::ToolUse { id, name } => {
                    self.content_blocks.push(ContentBlockBuilder::ToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        input_json: String::new(),
                    });
                    deltas.push(StreamDelta::ToolUseStart {
                        id: id.clone(),
                        name: name.clone(),
                    });
                }
                ContentBlockStartData::Thinking { thinking } => {
                    self.content_blocks.push(ContentBlockBuilder::Thinking {
                        thinking: thinking.clone(),
                        signature: String::new(),
                    });
                    if !thinking.is_empty() {
                        deltas.push(StreamDelta::ThinkingDelta(thinking.clone()));
                    }
                }
                ContentBlockStartData::RedactedThinking { data } => {
                    self.content_blocks
                        .push(ContentBlockBuilder::RedactedThinking(data.clone()));
                }
            },
            StreamEvent::ContentBlockDelta { index, delta } => {
                let idx = *index as usize;
                if idx < self.content_blocks.len() {
                    match delta {
                        Delta::TextDelta { text } => {
                            if let ContentBlockBuilder::Text(ref mut buf) = self.content_blocks[idx]
                            {
                                buf.push_str(text);
                            }
                            deltas.push(StreamDelta::TextDelta(text.clone()));
                        }
                        Delta::InputJsonDelta { partial_json } => {
                            if let ContentBlockBuilder::ToolUse {
                                ref mut input_json, ..
                            } = self.content_blocks[idx]
                            {
                                input_json.push_str(partial_json);
                            }
                            deltas.push(StreamDelta::InputJsonDelta(partial_json.clone()));
                        }
                        Delta::ThinkingDelta { thinking } => {
                            if let ContentBlockBuilder::Thinking {
                                thinking: ref mut buf,
                                ..
                            } = self.content_blocks[idx]
                            {
                                buf.push_str(thinking);
                            }
                            deltas.push(StreamDelta::ThinkingDelta(thinking.clone()));
                        }
                        Delta::SignatureDelta { signature } => {
                            if let ContentBlockBuilder::Thinking {
                                signature: ref mut sig,
                                ..
                            } = self.content_blocks[idx]
                            {
                                sig.push_str(signature);
                            }
                            // No delta emitted for signatures — internal bookkeeping only
                        }
                    }
                }
            }
            StreamEvent::ContentBlockStop { index: _ } => {
                // Block is complete; nothing to emit as delta.
            }
            StreamEvent::MessageDelta { delta, usage } => {
                if let Some(ref reason_str) = delta.stop_reason {
                    self.stop_reason = match reason_str.as_str() {
                        "end_turn" => Some(StopReason::EndTurn),
                        "tool_use" => Some(StopReason::ToolUse),
                        "max_tokens" => Some(StopReason::MaxTokens),
                        "stop_sequence" => Some(StopReason::StopSequence),
                        _ => None,
                    };
                }
                if let Some(ref u) = usage {
                    self.output_tokens = u.output_tokens;
                    let input_tokens = self
                        .input_usage
                        .as_ref()
                        .map(|iu| iu.input_tokens)
                        .unwrap_or(0);
                    deltas.push(StreamDelta::UsageUpdate {
                        input_tokens,
                        output_tokens: u.output_tokens,
                        cache_read: self
                            .input_usage
                            .as_ref()
                            .and_then(|iu| iu.cache_read_input_tokens),
                        cache_write: self
                            .input_usage
                            .as_ref()
                            .and_then(|iu| iu.cache_creation_input_tokens),
                        cost: u.cost,
                    });
                }
            }
            StreamEvent::MessageStop => {
                // Stream complete. Final response assembled via finish().
            }
            StreamEvent::Ping => {}
            StreamEvent::Unknown { event_type } => {
                tracing::debug!(event_type = %event_type, "Ignoring unrecognized stream event");
            }
            StreamEvent::Error { error } => {
                // Record the error so `finish` can report it. Previously this only
                // logged, and the stream then ended without a stop reason — so an
                // `overloaded_error` surfaced as the unrelated and misleading
                // "Stream ended without a stop_reason in message_delta".
                tracing::warn!(
                    error_type = %error.error_type,
                    message = %error.message,
                    "Stream error event received"
                );
                self.stream_error = Some(format!("{}: {}", error.error_type, error.message));
            }
        }

        deltas
    }

    /// Assemble the final `MessagesResponse` after the stream completes.
    ///
    /// Returns an error if the server reported one mid-stream, or if no stop reason was
    /// received (incomplete stream).
    pub fn finish(self) -> Result<MessagesResponse, AgentError> {
        // A mid-stream error is the real cause; the missing stop reason is only its
        // symptom, and reporting the symptom sent people looking in the wrong place.
        if let Some(err) = self.stream_error {
            return Err(AgentError::Llm(format!("Stream error: {err}")));
        }
        let usage = self.build_usage();
        let stop_reason = self.stop_reason.ok_or_else(|| {
            AgentError::Llm("Stream ended without a stop_reason in message_delta".to_string())
        })?;
        let content: Vec<ContentBlock> = self
            .content_blocks
            .into_iter()
            .map(|b| b.into_content_block())
            .collect();

        Ok(MessagesResponse {
            id: self.message_id,
            content,
            model: self.model,
            stop_reason,
            usage,
        })
    }

    /// Assemble a partial `MessagesResponse` from whatever data has arrived so far.
    ///
    /// Used when the stream is interrupted (e.g., user presses Ctrl+C).
    /// Defaults to `StopReason::EndTurn` if no stop reason was received.
    pub fn partial_finish(self) -> Result<MessagesResponse, AgentError> {
        let usage = self.build_usage();
        let stop_reason = self.stop_reason.unwrap_or(StopReason::EndTurn);
        let content: Vec<ContentBlock> = self
            .content_blocks
            .into_iter()
            .map(|b| b.into_content_block())
            .collect();

        Ok(MessagesResponse {
            id: self.message_id,
            content,
            model: self.model,
            stop_reason,
            usage,
        })
    }

    /// Build a `Usage` struct from tracked input/output token counts.
    fn build_usage(&self) -> Option<Usage> {
        let input_usage = self.input_usage.as_ref()?;
        Some(Usage {
            input_tokens: input_usage.input_tokens,
            output_tokens: self.output_tokens,
            cache_creation_input_tokens: input_usage.cache_creation_input_tokens,
            cache_read_input_tokens: input_usage.cache_read_input_tokens,
        })
    }

    /// Get the current model name (useful for cost calculation).
    pub fn model(&self) -> &str {
        &self.model
    }
}

impl Default for StreamAssembler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::stream::{
        ContentBlockStartData, Delta, DeltaUsage, MessageDeltaData, MessageStartData,
        MessageStartUsage, StreamErrorData,
    };

    fn make_message_start(
        id: &str,
        model: &str,
        input_tokens: u32,
        output_tokens: u32,
    ) -> StreamEvent {
        StreamEvent::MessageStart {
            message: MessageStartData {
                id: id.to_string(),
                message_type: "message".to_string(),
                role: "assistant".to_string(),
                model: model.to_string(),
                content: vec![],
                stop_reason: None,
                usage: Some(MessageStartUsage {
                    input_tokens,
                    output_tokens,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                }),
            },
        }
    }

    fn make_message_start_with_cache(
        id: &str,
        model: &str,
        input_tokens: u32,
        cache_read: u32,
        cache_write: u32,
    ) -> StreamEvent {
        StreamEvent::MessageStart {
            message: MessageStartData {
                id: id.to_string(),
                message_type: "message".to_string(),
                role: "assistant".to_string(),
                model: model.to_string(),
                content: vec![],
                stop_reason: None,
                usage: Some(MessageStartUsage {
                    input_tokens,
                    output_tokens: 0,
                    cache_creation_input_tokens: Some(cache_write),
                    cache_read_input_tokens: Some(cache_read),
                }),
            },
        }
    }

    fn make_text_block_start(index: u32) -> StreamEvent {
        StreamEvent::ContentBlockStart {
            index,
            content_block: ContentBlockStartData::Text {
                text: String::new(),
            },
        }
    }

    fn make_text_delta(index: u32, text: &str) -> StreamEvent {
        StreamEvent::ContentBlockDelta {
            index,
            delta: Delta::TextDelta {
                text: text.to_string(),
            },
        }
    }

    fn make_tool_use_start(index: u32, id: &str, name: &str) -> StreamEvent {
        StreamEvent::ContentBlockStart {
            index,
            content_block: ContentBlockStartData::ToolUse {
                id: id.to_string(),
                name: name.to_string(),
            },
        }
    }

    fn make_input_json_delta(index: u32, json: &str) -> StreamEvent {
        StreamEvent::ContentBlockDelta {
            index,
            delta: Delta::InputJsonDelta {
                partial_json: json.to_string(),
            },
        }
    }

    fn make_thinking_start(index: u32) -> StreamEvent {
        StreamEvent::ContentBlockStart {
            index,
            content_block: ContentBlockStartData::Thinking {
                thinking: String::new(),
            },
        }
    }

    fn make_thinking_delta(index: u32, thinking: &str) -> StreamEvent {
        StreamEvent::ContentBlockDelta {
            index,
            delta: Delta::ThinkingDelta {
                thinking: thinking.to_string(),
            },
        }
    }

    fn make_block_stop(index: u32) -> StreamEvent {
        StreamEvent::ContentBlockStop { index }
    }

    fn make_message_delta(stop_reason: &str, output_tokens: u32) -> StreamEvent {
        StreamEvent::MessageDelta {
            delta: MessageDeltaData {
                stop_reason: Some(stop_reason.to_string()),
            },
            usage: Some(DeltaUsage {
                output_tokens,
                input_tokens: None,
                cost: None,
            }),
        }
    }

    fn make_message_delta_with_cost(
        stop_reason: &str,
        output_tokens: u32,
        cost: f64,
    ) -> StreamEvent {
        StreamEvent::MessageDelta {
            delta: MessageDeltaData {
                stop_reason: Some(stop_reason.to_string()),
            },
            usage: Some(DeltaUsage {
                output_tokens,
                input_tokens: None,
                cost: Some(cost),
            }),
        }
    }

    // ---- StreamAssembler basic tests ----

    #[test]
    fn test_new_assembler_is_empty() {
        let asm = StreamAssembler::new();
        assert!(asm.message_id.is_empty());
        assert!(asm.model.is_empty());
        assert!(asm.content_blocks.is_empty());
        assert!(asm.stop_reason.is_none());
    }

    #[test]
    fn test_default_matches_new() {
        let asm = StreamAssembler::default();
        assert!(asm.message_id.is_empty());
        assert!(asm.content_blocks.is_empty());
    }

    // ---- MessageStart tests ----

    #[test]
    fn test_message_start_sets_id_and_model() {
        let mut asm = StreamAssembler::new();
        let event = make_message_start("msg_001", "claude-sonnet-4", 100, 0);
        let deltas = asm.process_event(&event);

        assert_eq!(asm.message_id, "msg_001");
        assert_eq!(asm.model, "claude-sonnet-4");
        assert_eq!(deltas.len(), 1);
        assert!(matches!(
            &deltas[0],
            StreamDelta::UsageUpdate {
                input_tokens: 100,
                output_tokens: 0,
                ..
            }
        ));
    }

    #[test]
    fn test_message_start_with_cache_tokens() {
        let mut asm = StreamAssembler::new();
        let event = make_message_start_with_cache("msg_002", "claude-sonnet-4", 50, 200, 100);
        let deltas = asm.process_event(&event);

        assert_eq!(deltas.len(), 1);
        match &deltas[0] {
            StreamDelta::UsageUpdate {
                input_tokens,
                cache_read,
                cache_write,
                ..
            } => {
                assert_eq!(*input_tokens, 50);
                assert_eq!(*cache_read, Some(200));
                assert_eq!(*cache_write, Some(100));
            }
            other => panic!("Expected UsageUpdate, got {:?}", other),
        }
    }

    // ---- Text content block tests ----

    #[test]
    fn test_text_block_assembly() {
        let mut asm = StreamAssembler::new();
        asm.process_event(&make_message_start("msg_003", "claude-sonnet-4", 10, 0));
        asm.process_event(&make_text_block_start(0));

        let d1 = asm.process_event(&make_text_delta(0, "Hello"));
        assert_eq!(d1, vec![StreamDelta::TextDelta("Hello".to_string())]);

        let d2 = asm.process_event(&make_text_delta(0, " world"));
        assert_eq!(d2, vec![StreamDelta::TextDelta(" world".to_string())]);

        asm.process_event(&make_block_stop(0));
        asm.process_event(&make_message_delta("end_turn", 5));
        asm.process_event(&StreamEvent::MessageStop);

        let response = asm.finish().unwrap();
        assert_eq!(response.id, "msg_003");
        assert_eq!(response.stop_reason, StopReason::EndTurn);
        assert_eq!(response.content.len(), 1);
        assert!(matches!(
            &response.content[0],
            ContentBlock::Text { text } if text == "Hello world"
        ));
    }

    // ---- ToolUse content block tests ----

    #[test]
    fn test_tool_use_block_assembly() {
        let mut asm = StreamAssembler::new();
        asm.process_event(&make_message_start("msg_004", "claude-sonnet-4", 20, 0));
        asm.process_event(&make_text_block_start(0));
        asm.process_event(&make_text_delta(0, "Let me navigate."));
        asm.process_event(&make_block_stop(0));

        let tool_deltas = asm.process_event(&make_tool_use_start(1, "toolu_abc", "navigate"));
        assert_eq!(
            tool_deltas,
            vec![StreamDelta::ToolUseStart {
                id: "toolu_abc".to_string(),
                name: "navigate".to_string(),
            }]
        );

        let json_d1 = asm.process_event(&make_input_json_delta(1, r#"{"url":"#));
        assert_eq!(
            json_d1,
            vec![StreamDelta::InputJsonDelta(r#"{"url":"#.to_string())]
        );

        let json_d2 = asm.process_event(&make_input_json_delta(1, r#""https://example.com"}"#));
        assert_eq!(
            json_d2,
            vec![StreamDelta::InputJsonDelta(
                r#""https://example.com"}"#.to_string()
            )]
        );

        asm.process_event(&make_block_stop(1));
        asm.process_event(&make_message_delta("tool_use", 15));
        asm.process_event(&StreamEvent::MessageStop);

        let response = asm.finish().unwrap();
        assert_eq!(response.stop_reason, StopReason::ToolUse);
        assert_eq!(response.content.len(), 2);
        assert!(
            matches!(&response.content[0], ContentBlock::Text { text } if text == "Let me navigate.")
        );
        match &response.content[1] {
            ContentBlock::ToolUse { id, name, input } => {
                assert_eq!(id, "toolu_abc");
                assert_eq!(name, "navigate");
                assert_eq!(input["url"], "https://example.com");
            }
            other => panic!("Expected ToolUse, got {:?}", other),
        }
    }

    // ---- Thinking content block tests ----

    #[test]
    fn test_thinking_block_assembly() {
        let mut asm = StreamAssembler::new();
        asm.process_event(&make_message_start("msg_005", "claude-sonnet-4", 30, 0));

        asm.process_event(&make_thinking_start(0));
        let d1 = asm.process_event(&make_thinking_delta(0, "Let me think"));
        assert_eq!(
            d1,
            vec![StreamDelta::ThinkingDelta("Let me think".to_string())]
        );

        let d2 = asm.process_event(&make_thinking_delta(0, " about this"));
        assert_eq!(
            d2,
            vec![StreamDelta::ThinkingDelta(" about this".to_string())]
        );

        asm.process_event(&make_block_stop(0));
        asm.process_event(&make_text_block_start(1));
        asm.process_event(&make_text_delta(1, "Here is my answer."));
        asm.process_event(&make_block_stop(1));
        asm.process_event(&make_message_delta("end_turn", 25));
        asm.process_event(&StreamEvent::MessageStop);

        let response = asm.finish().unwrap();
        assert_eq!(response.content.len(), 2);
        match &response.content[0] {
            ContentBlock::Thinking {
                thinking,
                signature,
            } => {
                assert_eq!(thinking, "Let me think about this");
                // Signature defaults to empty since streaming doesn't deliver it yet
                assert_eq!(signature, "");
            }
            other => panic!("Expected Thinking, got {:?}", other),
        }
        assert!(matches!(
            &response.content[1],
            ContentBlock::Text { text } if text == "Here is my answer."
        ));
    }

    // ---- Interleaved text + tool_use tests ----

    #[test]
    fn test_interleaved_text_and_tool_use() {
        let mut asm = StreamAssembler::new();
        asm.process_event(&make_message_start("msg_006", "claude-sonnet-4", 10, 0));

        // Text block
        asm.process_event(&make_text_block_start(0));
        asm.process_event(&make_text_delta(0, "I'll help."));
        asm.process_event(&make_block_stop(0));

        // First tool use
        asm.process_event(&make_tool_use_start(1, "toolu_1", "read_file"));
        asm.process_event(&make_input_json_delta(1, r#"{"path":"foo.txt"}"#));
        asm.process_event(&make_block_stop(1));

        // Second tool use
        asm.process_event(&make_tool_use_start(2, "toolu_2", "grep"));
        asm.process_event(&make_input_json_delta(2, r#"{"pattern":"error"}"#));
        asm.process_event(&make_block_stop(2));

        asm.process_event(&make_message_delta("tool_use", 40));
        asm.process_event(&StreamEvent::MessageStop);

        let response = asm.finish().unwrap();
        assert_eq!(response.content.len(), 3);
        assert!(matches!(&response.content[0], ContentBlock::Text { .. }));
        assert!(matches!(
            &response.content[1],
            ContentBlock::ToolUse { name, .. } if name == "read_file"
        ));
        assert!(matches!(
            &response.content[2],
            ContentBlock::ToolUse { name, .. } if name == "grep"
        ));
    }

    // ---- Partial finish (interrupt) tests ----

    #[test]
    fn test_partial_finish_defaults_to_end_turn() {
        let mut asm = StreamAssembler::new();
        asm.process_event(&make_message_start("msg_007", "claude-sonnet-4", 10, 0));
        asm.process_event(&make_text_block_start(0));
        asm.process_event(&make_text_delta(0, "Partial response"));
        // No message_delta, no message_stop — simulate interrupt

        let response = asm.partial_finish().unwrap();
        assert_eq!(response.stop_reason, StopReason::EndTurn);
        assert_eq!(response.content.len(), 1);
        assert!(matches!(
            &response.content[0],
            ContentBlock::Text { text } if text == "Partial response"
        ));
    }

    #[test]
    fn test_partial_finish_with_incomplete_tool_use() {
        let mut asm = StreamAssembler::new();
        asm.process_event(&make_message_start("msg_008", "claude-sonnet-4", 10, 0));
        asm.process_event(&make_text_block_start(0));
        asm.process_event(&make_text_delta(0, "Let me help."));
        asm.process_event(&make_block_stop(0));
        asm.process_event(&make_tool_use_start(1, "toolu_x", "bash"));
        asm.process_event(&make_input_json_delta(1, r#"{"comma"#));
        // Incomplete JSON — interrupt

        let response = asm.partial_finish().unwrap();
        assert_eq!(response.stop_reason, StopReason::EndTurn);
        assert_eq!(response.content.len(), 2);
        assert!(matches!(&response.content[0], ContentBlock::Text { .. }));
        // Incomplete JSON should fall back to empty object
        match &response.content[1] {
            ContentBlock::ToolUse { id, name, input } => {
                assert_eq!(id, "toolu_x");
                assert_eq!(name, "bash");
                // Invalid JSON falls back to empty object
                assert!(input.is_object());
            }
            other => panic!("Expected ToolUse, got {:?}", other),
        }
    }

    // ---- finish() error case ----

    #[test]
    fn test_finish_without_stop_reason_returns_error() {
        let mut asm = StreamAssembler::new();
        asm.process_event(&make_message_start("msg_009", "claude-sonnet-4", 10, 0));
        asm.process_event(&make_text_block_start(0));
        asm.process_event(&make_text_delta(0, "hi"));
        asm.process_event(&make_block_stop(0));
        // No message_delta — missing stop_reason

        let result = asm.finish();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, AgentError::Llm(msg) if msg.contains("stop_reason")));
    }

    // ---- Usage tracking tests ----

    #[test]
    fn test_usage_tracked_from_message_start_and_delta() {
        let mut asm = StreamAssembler::new();
        asm.process_event(&make_message_start("msg_010", "claude-sonnet-4", 150, 0));
        asm.process_event(&make_text_block_start(0));
        asm.process_event(&make_text_delta(0, "answer"));
        asm.process_event(&make_block_stop(0));
        asm.process_event(&make_message_delta("end_turn", 42));
        asm.process_event(&StreamEvent::MessageStop);

        let response = asm.finish().unwrap();
        let usage = response.usage.unwrap();
        assert_eq!(usage.input_tokens, 150);
        assert_eq!(usage.output_tokens, 42);
    }

    #[test]
    fn test_usage_with_cache_in_final_response() {
        let mut asm = StreamAssembler::new();
        asm.process_event(&make_message_start_with_cache(
            "msg_011",
            "claude-sonnet-4",
            80,
            500,
            200,
        ));
        asm.process_event(&make_text_block_start(0));
        asm.process_event(&make_text_delta(0, "cached answer"));
        asm.process_event(&make_block_stop(0));
        asm.process_event(&make_message_delta("end_turn", 10));
        asm.process_event(&StreamEvent::MessageStop);

        let response = asm.finish().unwrap();
        let usage = response.usage.unwrap();
        assert_eq!(usage.input_tokens, 80);
        assert_eq!(usage.output_tokens, 10);
        assert_eq!(usage.cache_read_input_tokens, Some(500));
        assert_eq!(usage.cache_creation_input_tokens, Some(200));
    }

    #[test]
    fn test_usage_none_when_no_message_start_usage() {
        let mut asm = StreamAssembler::new();
        // message_start without usage
        asm.process_event(&StreamEvent::MessageStart {
            message: MessageStartData {
                id: "msg_012".to_string(),
                message_type: "message".to_string(),
                role: "assistant".to_string(),
                model: "claude-sonnet-4".to_string(),
                content: vec![],
                stop_reason: None,
                usage: None,
            },
        });
        asm.process_event(&make_text_block_start(0));
        asm.process_event(&make_text_delta(0, "no usage"));
        asm.process_event(&make_block_stop(0));
        asm.process_event(&make_message_delta("end_turn", 5));

        let response = asm.finish().unwrap();
        assert!(response.usage.is_none());
    }

    // ---- Ping and error event tests ----

    #[test]
    fn test_ping_produces_no_deltas() {
        let mut asm = StreamAssembler::new();
        let deltas = asm.process_event(&StreamEvent::Ping);
        assert!(deltas.is_empty());
    }

    #[test]
    fn test_error_event_produces_no_deltas() {
        let mut asm = StreamAssembler::new();
        let deltas = asm.process_event(&StreamEvent::Error {
            error: StreamErrorData {
                error_type: "overloaded_error".to_string(),
                message: "Overloaded".to_string(),
            },
        });
        assert!(deltas.is_empty());
    }

    // ---- Delta emission verification ----

    #[test]
    fn test_all_delta_types_emitted_in_full_stream() {
        let mut asm = StreamAssembler::new();
        let mut all_deltas = Vec::new();

        // message_start → UsageUpdate
        all_deltas.extend(asm.process_event(&make_message_start(
            "msg_full",
            "claude-sonnet-4",
            100,
            0,
        )));

        // thinking block → ThinkingDelta
        all_deltas.extend(asm.process_event(&make_thinking_start(0)));
        all_deltas.extend(asm.process_event(&make_thinking_delta(0, "reasoning")));
        all_deltas.extend(asm.process_event(&make_block_stop(0)));

        // text block → TextDelta
        all_deltas.extend(asm.process_event(&make_text_block_start(1)));
        all_deltas.extend(asm.process_event(&make_text_delta(1, "response")));
        all_deltas.extend(asm.process_event(&make_block_stop(1)));

        // tool_use block → ToolUseStart + InputJsonDelta
        all_deltas.extend(asm.process_event(&make_tool_use_start(2, "toolu_z", "bash")));
        all_deltas.extend(asm.process_event(&make_input_json_delta(2, r#"{"cmd":"ls"}"#)));
        all_deltas.extend(asm.process_event(&make_block_stop(2)));

        // message_delta → UsageUpdate
        all_deltas.extend(asm.process_event(&make_message_delta("tool_use", 50)));
        all_deltas.extend(asm.process_event(&StreamEvent::MessageStop));

        // Verify we got all delta types
        assert!(all_deltas
            .iter()
            .any(|d| matches!(d, StreamDelta::UsageUpdate { .. })));
        assert!(all_deltas
            .iter()
            .any(|d| matches!(d, StreamDelta::ThinkingDelta(_))));
        assert!(all_deltas
            .iter()
            .any(|d| matches!(d, StreamDelta::TextDelta(_))));
        assert!(all_deltas
            .iter()
            .any(|d| matches!(d, StreamDelta::ToolUseStart { .. })));
        assert!(all_deltas
            .iter()
            .any(|d| matches!(d, StreamDelta::InputJsonDelta(_))));
    }

    // ---- Stop reason parsing ----

    #[test]
    fn test_stop_reason_end_turn() {
        let mut asm = StreamAssembler::new();
        asm.process_event(&make_message_start("m", "m", 0, 0));
        asm.process_event(&make_text_block_start(0));
        asm.process_event(&make_block_stop(0));
        asm.process_event(&make_message_delta("end_turn", 0));
        let resp = asm.finish().unwrap();
        assert_eq!(resp.stop_reason, StopReason::EndTurn);
    }

    #[test]
    fn test_stop_reason_tool_use() {
        let mut asm = StreamAssembler::new();
        asm.process_event(&make_message_start("m", "m", 0, 0));
        asm.process_event(&make_tool_use_start(0, "t", "bash"));
        asm.process_event(&make_input_json_delta(0, "{}"));
        asm.process_event(&make_block_stop(0));
        asm.process_event(&make_message_delta("tool_use", 0));
        let resp = asm.finish().unwrap();
        assert_eq!(resp.stop_reason, StopReason::ToolUse);
    }

    #[test]
    fn test_stop_reason_max_tokens() {
        let mut asm = StreamAssembler::new();
        asm.process_event(&make_message_start("m", "m", 0, 0));
        asm.process_event(&make_text_block_start(0));
        asm.process_event(&make_block_stop(0));
        asm.process_event(&make_message_delta("max_tokens", 0));
        let resp = asm.finish().unwrap();
        assert_eq!(resp.stop_reason, StopReason::MaxTokens);
    }

    #[test]
    fn test_stop_reason_stop_sequence() {
        let mut asm = StreamAssembler::new();
        asm.process_event(&make_message_start("m", "m", 0, 0));
        asm.process_event(&make_text_block_start(0));
        asm.process_event(&make_block_stop(0));
        asm.process_event(&make_message_delta("stop_sequence", 0));
        let resp = asm.finish().unwrap();
        assert_eq!(resp.stop_reason, StopReason::StopSequence);
    }

    // ---- Cost tracking ----

    #[test]
    fn test_cost_passed_through_in_usage_delta() {
        let mut asm = StreamAssembler::new();
        asm.process_event(&make_message_start("m", "m", 100, 0));
        asm.process_event(&make_text_block_start(0));
        asm.process_event(&make_block_stop(0));

        let deltas = asm.process_event(&make_message_delta_with_cost("end_turn", 50, 0.003));
        assert_eq!(deltas.len(), 1);
        match &deltas[0] {
            StreamDelta::UsageUpdate { cost, .. } => {
                assert_eq!(*cost, Some(0.003));
            }
            other => panic!("Expected UsageUpdate, got {:?}", other),
        }
    }

    // ---- Edge case: delta for out-of-bounds index ----

    #[test]
    fn test_delta_for_nonexistent_block_index_is_noop() {
        let mut asm = StreamAssembler::new();
        asm.process_event(&make_message_start("m", "m", 0, 0));
        // No content blocks started, but we get a delta for index 5
        let deltas = asm.process_event(&make_text_delta(5, "ghost"));
        // Out-of-bounds index: no buffer update and no delta emitted
        assert!(deltas.is_empty());
    }

    // ---- model() accessor ----

    #[test]
    fn test_model_accessor() {
        let mut asm = StreamAssembler::new();
        assert_eq!(asm.model(), "");
        asm.process_event(&make_message_start("m", "claude-opus-4", 0, 0));
        assert_eq!(asm.model(), "claude-opus-4");
    }

    // ---- Empty stream partial finish ----

    #[test]
    fn test_partial_finish_empty_stream() {
        let asm = StreamAssembler::new();
        let response = asm.partial_finish().unwrap();
        assert_eq!(response.stop_reason, StopReason::EndTurn);
        assert!(response.content.is_empty());
        assert!(response.id.is_empty());
    }

    // ---- MessageStop produces no deltas ----

    #[test]
    fn test_message_stop_produces_no_deltas() {
        let mut asm = StreamAssembler::new();
        let deltas = asm.process_event(&StreamEvent::MessageStop);
        assert!(deltas.is_empty());
    }

    // ---- ContentBlockStop produces no deltas ----

    #[test]
    fn test_content_block_stop_produces_no_deltas() {
        let mut asm = StreamAssembler::new();
        asm.process_event(&make_message_start("m", "m", 0, 0));
        asm.process_event(&make_text_block_start(0));
        let deltas = asm.process_event(&make_block_stop(0));
        assert!(deltas.is_empty());
    }

    // ---- Text block start with non-empty initial text ----

    #[test]
    fn test_text_block_start_with_initial_text() {
        let mut asm = StreamAssembler::new();
        asm.process_event(&make_message_start("m", "m", 0, 0));
        let deltas = asm.process_event(&StreamEvent::ContentBlockStart {
            index: 0,
            content_block: ContentBlockStartData::Text {
                text: "initial".to_string(),
            },
        });
        assert_eq!(deltas, vec![StreamDelta::TextDelta("initial".to_string())]);
    }

    // ---- Thinking block start with non-empty initial thinking ----

    #[test]
    fn test_thinking_block_start_with_initial_text() {
        let mut asm = StreamAssembler::new();
        asm.process_event(&make_message_start("m", "m", 0, 0));
        let deltas = asm.process_event(&StreamEvent::ContentBlockStart {
            index: 0,
            content_block: ContentBlockStartData::Thinking {
                thinking: "preamble".to_string(),
            },
        });
        assert_eq!(
            deltas,
            vec![StreamDelta::ThinkingDelta("preamble".to_string())]
        );
    }

    // ---- Multiple text blocks ----

    #[test]
    fn test_multiple_text_blocks() {
        let mut asm = StreamAssembler::new();
        asm.process_event(&make_message_start("m", "m", 0, 0));

        asm.process_event(&make_text_block_start(0));
        asm.process_event(&make_text_delta(0, "first"));
        asm.process_event(&make_block_stop(0));

        asm.process_event(&make_text_block_start(1));
        asm.process_event(&make_text_delta(1, "second"));
        asm.process_event(&make_block_stop(1));

        asm.process_event(&make_message_delta("end_turn", 10));
        let response = asm.finish().unwrap();
        assert_eq!(response.content.len(), 2);
        assert!(matches!(&response.content[0], ContentBlock::Text { text } if text == "first"));
        assert!(matches!(&response.content[1], ContentBlock::Text { text } if text == "second"));
    }
}
