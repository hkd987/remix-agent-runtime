use serde::{Deserialize, Serialize};

use crate::error::AgentError;

/// A parsed Server-Sent Event from the Anthropic streaming API.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    /// An event type this build does not recognize. Carried rather than silently
    /// folded into `Ping` so it is visible in logs and in tests.
    Unknown { event_type: String },
    /// `message_start` - contains the initial Message object
    MessageStart { message: MessageStartData },
    /// `content_block_start` - a new content block is beginning
    ContentBlockStart {
        index: u32,
        content_block: ContentBlockStartData,
    },
    /// `content_block_delta` - incremental update to a content block
    ContentBlockDelta { index: u32, delta: Delta },
    /// `content_block_stop` - a content block is complete
    ContentBlockStop { index: u32 },
    /// `message_delta` - update to the top-level message (e.g. stop_reason)
    MessageDelta {
        delta: MessageDeltaData,
        usage: Option<DeltaUsage>,
    },
    /// `message_stop` - the message is complete
    MessageStop,
    /// `ping` - keep-alive event
    Ping,
    /// `error` - an error occurred during streaming
    Error { error: StreamErrorData },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageStartData {
    pub id: String,
    #[serde(rename = "type")]
    pub message_type: String,
    pub role: String,
    pub model: String,
    #[serde(default)]
    pub content: Vec<serde_json::Value>,
    pub stop_reason: Option<String>,
    pub usage: Option<MessageStartUsage>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageStartUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    #[serde(default)]
    pub cache_creation_input_tokens: Option<u32>,
    #[serde(default)]
    pub cache_read_input_tokens: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlockStartData {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String },
    #[serde(rename = "thinking")]
    Thinking { thinking: String },
    #[serde(rename = "redacted_thinking")]
    RedactedThinking { data: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Delta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
    #[serde(rename = "thinking_delta")]
    ThinkingDelta { thinking: String },
    #[serde(rename = "signature_delta")]
    SignatureDelta { signature: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageDeltaData {
    pub stop_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeltaUsage {
    pub output_tokens: u32,
    #[serde(default)]
    pub input_tokens: Option<u32>,
    #[serde(default)]
    pub cost: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamErrorData {
    #[serde(rename = "type")]
    pub error_type: String,
    pub message: String,
}

/// Incremental SSE parser that accumulates bytes and emits complete events.
///
/// Handles the Anthropic streaming format where each event is:
/// ```text
/// event: <event_type>
/// data: <json_data>
/// ```
pub struct SseParser {
    buffer: String,
}

impl SseParser {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
        }
    }

    /// Feed raw bytes into the parser and return any complete (event_type, data) pairs.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<(String, String)> {
        let text = String::from_utf8_lossy(bytes);
        self.buffer.push_str(&text);

        let mut events = Vec::new();
        // SSE events are separated by double newlines
        while let Some(pos) = self.buffer.find("\n\n") {
            let block = self.buffer[..pos].to_string();
            self.buffer = self.buffer[pos + 2..].to_string();

            let mut event_type = String::new();
            let mut data_lines = Vec::new();

            for line in block.lines() {
                if let Some(value) = line.strip_prefix("event: ") {
                    event_type = value.trim().to_string();
                } else if let Some(value) = line.strip_prefix("data: ") {
                    data_lines.push(value.to_string());
                } else if let Some(value) = line.strip_prefix("event:") {
                    event_type = value.trim().to_string();
                } else if let Some(value) = line.strip_prefix("data:") {
                    data_lines.push(value.trim_start().to_string());
                }
            }

            if !event_type.is_empty() {
                let data = data_lines.join("\n");
                events.push((event_type, data));
            }
        }

        events
    }

    /// Parse an (event_type, data) pair into a `StreamEvent`.
    pub fn parse_event(event_type: &str, data: &str) -> Result<StreamEvent, AgentError> {
        match event_type {
            "message_start" => {
                #[derive(Deserialize)]
                struct Wrapper {
                    message: MessageStartData,
                }
                let wrapper: Wrapper = serde_json::from_str(data)
                    .map_err(|e| AgentError::Llm(format!("Failed to parse message_start: {e}")))?;
                Ok(StreamEvent::MessageStart {
                    message: wrapper.message,
                })
            }
            "content_block_start" => {
                #[derive(Deserialize)]
                struct Wrapper {
                    index: u32,
                    content_block: ContentBlockStartData,
                }
                let wrapper: Wrapper = serde_json::from_str(data).map_err(|e| {
                    AgentError::Llm(format!("Failed to parse content_block_start: {e}"))
                })?;
                Ok(StreamEvent::ContentBlockStart {
                    index: wrapper.index,
                    content_block: wrapper.content_block,
                })
            }
            "content_block_delta" => {
                #[derive(Deserialize)]
                struct Wrapper {
                    index: u32,
                    delta: Delta,
                }
                let wrapper: Wrapper = serde_json::from_str(data).map_err(|e| {
                    AgentError::Llm(format!("Failed to parse content_block_delta: {e}"))
                })?;
                Ok(StreamEvent::ContentBlockDelta {
                    index: wrapper.index,
                    delta: wrapper.delta,
                })
            }
            "content_block_stop" => {
                #[derive(Deserialize)]
                struct Wrapper {
                    index: u32,
                }
                let wrapper: Wrapper = serde_json::from_str(data).map_err(|e| {
                    AgentError::Llm(format!("Failed to parse content_block_stop: {e}"))
                })?;
                Ok(StreamEvent::ContentBlockStop {
                    index: wrapper.index,
                })
            }
            "message_delta" => {
                #[derive(Deserialize)]
                struct Wrapper {
                    delta: MessageDeltaData,
                    usage: Option<DeltaUsage>,
                }
                let wrapper: Wrapper = serde_json::from_str(data)
                    .map_err(|e| AgentError::Llm(format!("Failed to parse message_delta: {e}")))?;
                Ok(StreamEvent::MessageDelta {
                    delta: wrapper.delta,
                    usage: wrapper.usage,
                })
            }
            "message_stop" => Ok(StreamEvent::MessageStop),
            "ping" => Ok(StreamEvent::Ping),
            "error" => {
                #[derive(Deserialize)]
                struct Wrapper {
                    error: StreamErrorData,
                }
                let wrapper: Wrapper = serde_json::from_str(data)
                    .map_err(|e| AgentError::Llm(format!("Failed to parse error event: {e}")))?;
                Ok(StreamEvent::Error {
                    error: wrapper.error,
                })
            }
            other => {
                // Unknown event types are tolerated — some providers emit their own —
                // but mapping them to `Ping` made them indistinguishable from a
                // keep-alive, so a new protocol event would vanish without trace.
                tracing::debug!(event_type = %other, "Ignoring unrecognized SSE event type");
                Ok(StreamEvent::Unknown {
                    event_type: other.to_string(),
                })
            }
        }
    }
}

impl Default for SseParser {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sse_parser_basic_event() {
        let mut parser = SseParser::new();
        let input = b"event: ping\ndata: {}\n\n";
        let events = parser.feed(input);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "ping");
        assert_eq!(events[0].1, "{}");
    }

    #[test]
    fn test_sse_parser_multiple_events() {
        let mut parser = SseParser::new();
        let input = b"event: ping\ndata: {}\n\nevent: message_stop\ndata: {}\n\n";
        let events = parser.feed(input);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].0, "ping");
        assert_eq!(events[1].0, "message_stop");
    }

    #[test]
    fn test_sse_parser_partial_event() {
        let mut parser = SseParser::new();
        // First chunk: incomplete event
        let events = parser.feed(b"event: ping\n");
        assert_eq!(events.len(), 0);
        // Second chunk: completes the event
        let events = parser.feed(b"data: {}\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "ping");
    }

    #[test]
    fn test_sse_parser_chunked_bytes() {
        let mut parser = SseParser::new();
        let events = parser.feed(b"event: content_block_del");
        assert_eq!(events.len(), 0);
        let events = parser.feed(b"ta\ndata: {\"index\":0,\"delta\":");
        assert_eq!(events.len(), 0);
        let events = parser.feed(b"{\"type\":\"text_delta\",\"text\":\"Hi\"}}\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "content_block_delta");
    }

    #[test]
    fn test_parse_ping_event() {
        let event = SseParser::parse_event("ping", "{}").unwrap();
        assert_eq!(event, StreamEvent::Ping);
    }

    #[test]
    fn test_parse_message_stop_event() {
        let event = SseParser::parse_event("message_stop", "{}").unwrap();
        assert_eq!(event, StreamEvent::MessageStop);
    }

    #[test]
    fn test_parse_message_start_event() {
        let data = r#"{"type":"message_start","message":{"id":"msg_123","type":"message","role":"assistant","model":"claude-3","content":[],"stop_reason":null,"usage":{"input_tokens":10,"output_tokens":0}}}"#;
        let event = SseParser::parse_event("message_start", data).unwrap();
        match event {
            StreamEvent::MessageStart { message } => {
                assert_eq!(message.id, "msg_123");
                assert_eq!(message.role, "assistant");
                assert_eq!(message.model, "claude-3");
            }
            other => panic!("Expected MessageStart, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_content_block_start_text() {
        let data =
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#;
        let event = SseParser::parse_event("content_block_start", data).unwrap();
        match event {
            StreamEvent::ContentBlockStart {
                index,
                content_block,
            } => {
                assert_eq!(index, 0);
                assert!(
                    matches!(content_block, ContentBlockStartData::Text { text } if text.is_empty())
                );
            }
            other => panic!("Expected ContentBlockStart, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_content_block_start_tool_use() {
        let data = r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_abc","name":"navigate"}}"#;
        let event = SseParser::parse_event("content_block_start", data).unwrap();
        match event {
            StreamEvent::ContentBlockStart {
                index,
                content_block,
            } => {
                assert_eq!(index, 1);
                assert!(
                    matches!(content_block, ContentBlockStartData::ToolUse { id, name } if id == "toolu_abc" && name == "navigate")
                );
            }
            other => panic!("Expected ContentBlockStart, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_content_block_delta_text() {
        let data = r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#;
        let event = SseParser::parse_event("content_block_delta", data).unwrap();
        match event {
            StreamEvent::ContentBlockDelta { index, delta } => {
                assert_eq!(index, 0);
                assert!(matches!(delta, Delta::TextDelta { text } if text == "Hello"));
            }
            other => panic!("Expected ContentBlockDelta, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_content_block_delta_input_json() {
        let data = r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"url\":\"https://"}}"#;
        let event = SseParser::parse_event("content_block_delta", data).unwrap();
        match event {
            StreamEvent::ContentBlockDelta { index, delta } => {
                assert_eq!(index, 1);
                assert!(
                    matches!(delta, Delta::InputJsonDelta { partial_json } if partial_json == r#"{"url":"https://"#)
                );
            }
            other => panic!("Expected ContentBlockDelta, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_content_block_stop() {
        let data = r#"{"type":"content_block_stop","index":0}"#;
        let event = SseParser::parse_event("content_block_stop", data).unwrap();
        assert_eq!(event, StreamEvent::ContentBlockStop { index: 0 });
    }

    #[test]
    fn test_parse_message_delta() {
        let data = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":42}}"#;
        let event = SseParser::parse_event("message_delta", data).unwrap();
        match event {
            StreamEvent::MessageDelta { delta, usage } => {
                assert_eq!(delta.stop_reason, Some("end_turn".to_string()));
                let u = usage.unwrap();
                assert_eq!(u.output_tokens, 42);
                assert_eq!(u.input_tokens, None);
                assert_eq!(u.cost, None);
            }
            other => panic!("Expected MessageDelta, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_message_delta_with_full_usage() {
        let data = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":150,"input_tokens":500,"cost":0.003}}"#;
        let event = SseParser::parse_event("message_delta", data).unwrap();
        match event {
            StreamEvent::MessageDelta { delta, usage } => {
                assert_eq!(delta.stop_reason, Some("end_turn".to_string()));
                let u = usage.unwrap();
                assert_eq!(u.output_tokens, 150);
                assert_eq!(u.input_tokens, Some(500));
                assert_eq!(u.cost, Some(0.003));
            }
            other => panic!("Expected MessageDelta, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_error_event() {
        let data = r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        let event = SseParser::parse_event("error", data).unwrap();
        match event {
            StreamEvent::Error { error } => {
                assert_eq!(error.error_type, "overloaded_error");
                assert_eq!(error.message, "Overloaded");
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_unknown_event_type() {
        // Unknown events are tolerated but kept distinguishable from a keep-alive, so a
        // new protocol event shows up in logs instead of vanishing as a Ping.
        let result = SseParser::parse_event("unknown_type", "{}");
        assert_eq!(
            result.unwrap(),
            StreamEvent::Unknown {
                event_type: "unknown_type".to_string()
            }
        );
    }

    #[test]
    fn test_sse_parser_default() {
        let parser = SseParser::default();
        assert!(parser.buffer.is_empty());
    }
}
