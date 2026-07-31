use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CacheControl {
    #[serde(rename = "type")]
    pub cache_type: String,
}

impl CacheControl {
    pub fn ephemeral() -> Self {
        Self {
            cache_type: "ephemeral".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SystemContent {
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
}

impl SystemContent {
    pub fn text(text: impl Into<String>) -> Self {
        SystemContent::Text {
            text: text.into(),
            cache_control: None,
        }
    }

    pub fn text_cached(text: impl Into<String>) -> Self {
        SystemContent::Text {
            text: text.into(),
            cache_control: Some(CacheControl::ephemeral()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum ToolResultContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImageSource {
    #[serde(rename = "type")]
    pub source_type: String,
    pub media_type: String,
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Image {
        source: ImageSource,
    },
    Thinking {
        thinking: String,
        signature: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: ToolResultContent,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
    /// Redacted thinking blocks returned by the API when extended thinking
    /// content is filtered. We deserialize and preserve them but skip
    /// serialization of the opaque `data` field when not present.
    RedactedThinking {
        data: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThinkingConfig {
    #[serde(rename = "type")]
    pub thinking_type: String,
    pub budget_tokens: u32,
}

impl ThinkingConfig {
    pub fn enabled(budget_tokens: u32) -> Self {
        Self {
            thinking_type: "enabled".to_string(),
            budget_tokens,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
    /// Whether this tool only reads state (not sent to API, used for plan mode filtering).
    #[serde(skip)]
    pub read_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessagesRequest {
    pub model: String,
    pub max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<Vec<SystemContent>>,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    /// Index into `messages` whose final content block carries a cache breakpoint.
    ///
    /// Held here rather than on `ContentBlock` so the breakpoint is applied once, at
    /// the serialization boundary, instead of adding a `cache_control` field to two
    /// enum variants that are constructed at ~180 sites across the tree.
    #[serde(skip)]
    pub cache_breakpoint: Option<usize>,
}

/// A content block with a cache breakpoint attached.
///
/// `flatten` splices `cache_control` into the block's own object, producing exactly the
/// shape the API expects, without any hand-built JSON.
#[derive(Serialize)]
struct CachedBlock<'a> {
    #[serde(flatten)]
    inner: &'a ContentBlock,
    cache_control: CacheControl,
}

#[derive(Serialize)]
#[serde(untagged)]
enum MaybeCachedBlock<'a> {
    Plain(&'a ContentBlock),
    Cached(CachedBlock<'a>),
}

#[derive(Serialize)]
struct SerializableMessage<'a> {
    role: &'a Role,
    content: Vec<MaybeCachedBlock<'a>>,
}

/// The wire form of a request, with the cache breakpoint applied.
#[derive(Serialize)]
pub struct WireRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<&'a Vec<SystemContent>>,
    messages: Vec<SerializableMessage<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<&'a Vec<ToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<&'a ThinkingConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

impl MessagesRequest {
    /// Build the wire form, splicing in the cache breakpoint if one is set.
    pub fn to_wire(&self) -> WireRequest<'_> {
        let messages = self
            .messages
            .iter()
            .enumerate()
            .map(|(i, msg)| {
                let mark_last = self.cache_breakpoint == Some(i);
                let last_idx = msg.content.len().saturating_sub(1);
                let content = msg
                    .content
                    .iter()
                    .enumerate()
                    .map(|(j, block)| {
                        if mark_last && j == last_idx && block_accepts_cache_control(block) {
                            MaybeCachedBlock::Cached(CachedBlock {
                                inner: block,
                                cache_control: CacheControl::ephemeral(),
                            })
                        } else {
                            MaybeCachedBlock::Plain(block)
                        }
                    })
                    .collect();
                SerializableMessage {
                    role: &msg.role,
                    content,
                }
            })
            .collect();

        WireRequest {
            model: &self.model,
            max_tokens: self.max_tokens,
            system: self.system.as_ref(),
            messages,
            tools: self.tools.as_ref(),
            thinking: self.thinking.as_ref(),
            stream: self.stream,
        }
    }
}

/// Whether the API accepts `cache_control` on this block type.
///
/// Thinking and redacted-thinking blocks reject it, and marking one fails the whole
/// request — so a breakpoint that lands on a thinking block is skipped rather than sent.
fn block_accepts_cache_control(block: &ContentBlock) -> bool {
    matches!(
        block,
        ContentBlock::Text { .. } | ContentBlock::ToolResult { .. } | ContentBlock::Image { .. }
    )
}

/// Choose the message index that should carry the conversation cache breakpoint.
///
/// Without one, the entire growing history is re-sent uncached on every iteration,
/// which on a long run is the dominant cost. The breakpoint goes on the last message of
/// the stable prefix — the most recent exchange is excluded because it changes every
/// turn and would invalidate the entry immediately.
///
/// Returns `None` for conversations too short to be worth caching.
pub fn conversation_cache_breakpoint(messages: &[Message]) -> Option<usize> {
    /// The API does not cache prefixes below its own minimum, so very short
    /// conversations gain nothing and would just burn a breakpoint.
    const MIN_MESSAGES: usize = 5;
    /// Leave the most recent exchange (assistant turn + tool results) outside.
    const UNCACHED_TAIL: usize = 2;

    if messages.len() < MIN_MESSAGES {
        return None;
    }
    let idx = messages.len() - UNCACHED_TAIL - 1;
    // A breakpoint is useless on a block type that cannot carry one.
    messages
        .get(idx)
        .and_then(|m| m.content.last())
        .filter(|b| block_accepts_cache_control(b))
        .map(|_| idx)
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    StopSequence,
    /// A stop reason this build does not know about, such as `refusal` or `pause_turn`.
    ///
    /// Without this arm the field fails to deserialize and the whole response surfaces
    /// as a misleading "Failed to parse response" error, so a new API value looks like
    /// a client bug. The loop treats it as the end of the turn.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessagesResponse {
    pub id: String,
    pub content: Vec<ContentBlock>,
    pub model: String,
    pub stop_reason: StopReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

/// Returns `true` if any block in the slice is a `ToolUse` variant.
pub fn content_has_tool_use(content: &[ContentBlock]) -> bool {
    content
        .iter()
        .any(|b| matches!(b, ContentBlock::ToolUse { .. }))
}

/// Per-token pricing (input, output) in USD. Returns (input_cost_per_token, output_cost_per_token).
pub fn model_pricing(model: &str) -> (f64, f64) {
    // Claude pricing per million tokens
    let (input_per_m, output_per_m) = if model.starts_with("claude-opus") {
        (15.0, 75.0)
    } else if model.starts_with("claude-sonnet") {
        (3.0, 15.0)
    } else if model.starts_with("claude-haiku") {
        (0.25, 1.25)
    } else {
        // Default to Sonnet pricing for unknown models
        (3.0, 15.0)
    };
    (input_per_m / 1_000_000.0, output_per_m / 1_000_000.0)
}

/// Cache pricing multipliers relative to the base input rate.
///
/// Writing to the cache costs more than an uncached token; reading from it costs far
/// less. Ignoring both makes cached reads look like full-price input, which understates
/// the benefit of caching and overstates spend.
const CACHE_WRITE_MULTIPLIER: f64 = 1.25;
const CACHE_READ_MULTIPLIER: f64 = 0.1;

/// Compute total cost from token counts and model.
pub fn compute_cost(model: &str, input_tokens: u32, output_tokens: u32) -> f64 {
    let (input_rate, output_rate) = model_pricing(model);
    (input_tokens as f64 * input_rate) + (output_tokens as f64 * output_rate)
}

/// Compute total cost including cache reads and writes, which the API bills separately
/// from `input_tokens`.
pub fn compute_cost_with_cache(model: &str, usage: &Usage) -> f64 {
    let (input_rate, output_rate) = model_pricing(model);
    let base =
        (usage.input_tokens as f64 * input_rate) + (usage.output_tokens as f64 * output_rate);
    let write =
        usage.cache_creation_input_tokens.unwrap_or(0) as f64 * input_rate * CACHE_WRITE_MULTIPLIER;
    let read =
        usage.cache_read_input_tokens.unwrap_or(0) as f64 * input_rate * CACHE_READ_MULTIPLIER;
    base + write + read
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_role_serialization() {
        let user_json = serde_json::to_string(&Role::User).unwrap();
        assert_eq!(user_json, "\"user\"");

        let assistant_json = serde_json::to_string(&Role::Assistant).unwrap();
        assert_eq!(assistant_json, "\"assistant\"");
    }

    #[test]
    fn test_role_deserialization() {
        let user: Role = serde_json::from_str("\"user\"").unwrap();
        assert_eq!(user, Role::User);

        let assistant: Role = serde_json::from_str("\"assistant\"").unwrap();
        assert_eq!(assistant, Role::Assistant);
    }

    #[test]
    fn test_content_block_text_roundtrip() {
        let block = ContentBlock::Text {
            text: "Hello, world!".to_string(),
        };
        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["type"], "text");
        assert_eq!(json["text"], "Hello, world!");

        let deserialized: ContentBlock = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized, block);
    }

    #[test]
    fn test_content_block_tool_use_roundtrip() {
        let block = ContentBlock::ToolUse {
            id: "toolu_01A".to_string(),
            name: "navigate".to_string(),
            input: json!({"url": "https://example.com"}),
        };
        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["type"], "tool_use");
        assert_eq!(json["id"], "toolu_01A");
        assert_eq!(json["name"], "navigate");
        assert_eq!(json["input"]["url"], "https://example.com");

        let deserialized: ContentBlock = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized, block);
    }

    #[test]
    fn test_content_block_tool_result_roundtrip() {
        let block = ContentBlock::ToolResult {
            tool_use_id: "toolu_01A".to_string(),
            content: ToolResultContent::Text("Page loaded successfully".to_string()),
            is_error: None,
        };
        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["type"], "tool_result");
        assert_eq!(json["tool_use_id"], "toolu_01A");
        assert_eq!(json["content"], "Page loaded successfully");
        assert!(json.get("is_error").is_none());

        let deserialized: ContentBlock = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized, block);
    }

    #[test]
    fn test_content_block_tool_result_with_error() {
        let block = ContentBlock::ToolResult {
            tool_use_id: "toolu_01B".to_string(),
            content: ToolResultContent::Text("Navigation failed".to_string()),
            is_error: Some(true),
        };
        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["is_error"], true);

        let deserialized: ContentBlock = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized, block);
    }

    #[test]
    fn test_content_block_text_from_api_json() {
        let api_json = json!({
            "type": "text",
            "text": "Here is the result of the navigation."
        });
        let block: ContentBlock = serde_json::from_value(api_json).unwrap();
        assert_eq!(
            block,
            ContentBlock::Text {
                text: "Here is the result of the navigation.".to_string()
            }
        );
    }

    #[test]
    fn test_content_block_tool_use_from_api_json() {
        let api_json = json!({
            "type": "tool_use",
            "id": "toolu_abc123",
            "name": "click",
            "input": {"selector": "#submit-btn"}
        });
        let block: ContentBlock = serde_json::from_value(api_json).unwrap();
        assert_eq!(
            block,
            ContentBlock::ToolUse {
                id: "toolu_abc123".to_string(),
                name: "click".to_string(),
                input: json!({"selector": "#submit-btn"}),
            }
        );
    }

    #[test]
    fn test_messages_request_serialization() {
        let request = MessagesRequest {
            model: "claude-sonnet-4-5-20250929".to_string(),
            max_tokens: 8192,
            system: Some(vec![SystemContent::text(
                "You are a browser automation agent.",
            )]),
            messages: vec![Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "Navigate to example.com".to_string(),
                }],
            }],
            tools: None,
            thinking: None,
            stream: None,
            cache_breakpoint: None,
        };

        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["model"], "claude-sonnet-4-5-20250929");
        assert_eq!(json["max_tokens"], 8192);
        assert_eq!(json["system"][0]["type"], "text");
        assert_eq!(
            json["system"][0]["text"],
            "You are a browser automation agent."
        );
        assert_eq!(json["messages"][0]["role"], "user");
        assert_eq!(
            json["messages"][0]["content"][0]["text"],
            "Navigate to example.com"
        );
        assert!(json.get("tools").is_none());
        assert!(json.get("thinking").is_none());
        assert!(json.get("stream").is_none());
    }

    #[test]
    fn test_messages_request_with_tools() {
        let request = MessagesRequest {
            model: "claude-sonnet-4-5-20250929".to_string(),
            max_tokens: 4096,
            system: None,
            messages: vec![],
            tools: Some(vec![ToolDefinition {
                name: "navigate".to_string(),
                description: "Navigate to a URL".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "url": {"type": "string"}
                    },
                    "required": ["url"]
                }),
                cache_control: None,
                read_only: false,
            }]),
            thinking: None,
            stream: None,
            cache_breakpoint: None,
        };

        let json = serde_json::to_value(&request).unwrap();
        assert!(json.get("system").is_none());
        assert_eq!(json["tools"][0]["name"], "navigate");
        assert_eq!(json["tools"][0]["description"], "Navigate to a URL");
    }

    #[test]
    fn test_messages_request_roundtrip() {
        let request = MessagesRequest {
            model: "claude-sonnet-4-5-20250929".to_string(),
            max_tokens: 8192,
            system: Some(vec![SystemContent::text("System prompt")]),
            messages: vec![
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
            ],
            tools: None,
            thinking: None,
            stream: None,
            cache_breakpoint: None,
        };

        let json_str = serde_json::to_string(&request).unwrap();
        let deserialized: MessagesRequest = serde_json::from_str(&json_str).unwrap();
        assert_eq!(deserialized.model, request.model);
        assert_eq!(deserialized.max_tokens, request.max_tokens);
        assert_eq!(deserialized.messages.len(), 2);
    }

    #[test]
    fn test_stop_reason_variants() {
        let end_turn: StopReason = serde_json::from_str("\"end_turn\"").unwrap();
        assert_eq!(end_turn, StopReason::EndTurn);

        let tool_use: StopReason = serde_json::from_str("\"tool_use\"").unwrap();
        assert_eq!(tool_use, StopReason::ToolUse);

        let max_tokens: StopReason = serde_json::from_str("\"max_tokens\"").unwrap();
        assert_eq!(max_tokens, StopReason::MaxTokens);

        let stop_seq: StopReason = serde_json::from_str("\"stop_sequence\"").unwrap();
        assert_eq!(stop_seq, StopReason::StopSequence);
    }

    #[test]
    fn test_stop_reason_serialization() {
        assert_eq!(
            serde_json::to_string(&StopReason::EndTurn).unwrap(),
            "\"end_turn\""
        );
        assert_eq!(
            serde_json::to_string(&StopReason::ToolUse).unwrap(),
            "\"tool_use\""
        );
        assert_eq!(
            serde_json::to_string(&StopReason::MaxTokens).unwrap(),
            "\"max_tokens\""
        );
        assert_eq!(
            serde_json::to_string(&StopReason::StopSequence).unwrap(),
            "\"stop_sequence\""
        );
    }

    #[test]
    fn test_usage_roundtrip() {
        let usage = Usage {
            input_tokens: 100,
            output_tokens: 250,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        };
        let json = serde_json::to_value(&usage).unwrap();
        assert_eq!(json["input_tokens"], 100);
        assert_eq!(json["output_tokens"], 250);
        assert!(json.get("cache_creation_input_tokens").is_none());
        assert!(json.get("cache_read_input_tokens").is_none());

        let deserialized: Usage = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.input_tokens, 100);
        assert_eq!(deserialized.output_tokens, 250);
    }

    #[test]
    fn test_messages_response_deserialization() {
        let api_response = json!({
            "id": "msg_01XFDUDYJgAACzvnptvVoYEL",
            "type": "message",
            "role": "assistant",
            "content": [
                {
                    "type": "text",
                    "text": "I'll navigate to example.com for you."
                }
            ],
            "model": "claude-sonnet-4-5-20250929",
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 25,
                "output_tokens": 15
            }
        });

        let response: MessagesResponse = serde_json::from_value(api_response).unwrap();
        assert_eq!(response.id, "msg_01XFDUDYJgAACzvnptvVoYEL");
        assert_eq!(response.model, "claude-sonnet-4-5-20250929");
        assert_eq!(response.stop_reason, StopReason::EndTurn);
        assert_eq!(response.content.len(), 1);
        assert!(matches!(
            &response.content[0],
            ContentBlock::Text { text } if text == "I'll navigate to example.com for you."
        ));
        let usage = response.usage.unwrap();
        assert_eq!(usage.input_tokens, 25);
        assert_eq!(usage.output_tokens, 15);
    }

    #[test]
    fn test_messages_response_with_tool_use() {
        let api_response = json!({
            "id": "msg_02ABC",
            "content": [
                {
                    "type": "text",
                    "text": "I'll use the navigate tool."
                },
                {
                    "type": "tool_use",
                    "id": "toolu_01XYZ",
                    "name": "navigate",
                    "input": {"url": "https://example.com"}
                }
            ],
            "model": "claude-sonnet-4-5-20250929",
            "stop_reason": "tool_use",
            "usage": {
                "input_tokens": 50,
                "output_tokens": 30
            }
        });

        let response: MessagesResponse = serde_json::from_value(api_response).unwrap();
        assert_eq!(response.stop_reason, StopReason::ToolUse);
        assert_eq!(response.content.len(), 2);
        assert!(matches!(&response.content[0], ContentBlock::Text { .. }));
        assert!(matches!(
            &response.content[1],
            ContentBlock::ToolUse { name, .. } if name == "navigate"
        ));
    }

    #[test]
    fn test_messages_response_without_usage() {
        let api_response = json!({
            "id": "msg_03DEF",
            "content": [
                {
                    "type": "text",
                    "text": "Done."
                }
            ],
            "model": "claude-sonnet-4-5-20250929",
            "stop_reason": "end_turn"
        });

        let response: MessagesResponse = serde_json::from_value(api_response).unwrap();
        assert!(response.usage.is_none());
    }

    #[test]
    fn test_message_with_multiple_content_blocks() {
        let msg = Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Text {
                    text: "Let me help.".to_string(),
                },
                ContentBlock::ToolUse {
                    id: "toolu_01".to_string(),
                    name: "click".to_string(),
                    input: json!({"selector": "button"}),
                },
            ],
        };

        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["role"], "assistant");
        assert_eq!(json["content"].as_array().unwrap().len(), 2);

        let deserialized: Message = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.content.len(), 2);
    }

    #[test]
    fn test_tool_definition_roundtrip() {
        let tool = ToolDefinition {
            name: "screenshot".to_string(),
            description: "Take a screenshot of the current page".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
            cache_control: None,
            read_only: false,
        };

        let json_str = serde_json::to_string(&tool).unwrap();
        let deserialized: ToolDefinition = serde_json::from_str(&json_str).unwrap();
        assert_eq!(deserialized.name, "screenshot");
        assert_eq!(
            deserialized.description,
            "Take a screenshot of the current page"
        );
    }

    #[test]
    fn test_model_pricing_opus() {
        let (input, output) = model_pricing("claude-opus-4-20250514");
        assert!((input - 15.0 / 1_000_000.0).abs() < f64::EPSILON);
        assert!((output - 75.0 / 1_000_000.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_model_pricing_sonnet() {
        let (input, output) = model_pricing("claude-sonnet-4-20250514");
        assert!((input - 3.0 / 1_000_000.0).abs() < f64::EPSILON);
        assert!((output - 15.0 / 1_000_000.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_model_pricing_haiku() {
        let (input, output) = model_pricing("claude-haiku-3-20240307");
        assert!((input - 0.25 / 1_000_000.0).abs() < f64::EPSILON);
        assert!((output - 1.25 / 1_000_000.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_model_pricing_unknown_defaults_to_sonnet() {
        let (input, output) = model_pricing("some-unknown-model");
        let (sonnet_input, sonnet_output) = model_pricing("claude-sonnet-4-20250514");
        assert!((input - sonnet_input).abs() < f64::EPSILON);
        assert!((output - sonnet_output).abs() < f64::EPSILON);
    }

    #[test]
    fn test_compute_cost_sonnet() {
        // 1000 input tokens at $3/M = $0.003, 500 output tokens at $15/M = $0.0075
        let cost = compute_cost("claude-sonnet-4-20250514", 1000, 500);
        let expected = 0.003 + 0.0075;
        assert!((cost - expected).abs() < 1e-10);
    }

    #[test]
    fn test_compute_cost_opus() {
        // 1_000_000 input tokens at $15/M = $15, 1_000_000 output at $75/M = $75
        let cost = compute_cost("claude-opus-4-20250514", 1_000_000, 1_000_000);
        let expected = 15.0 + 75.0;
        assert!((cost - expected).abs() < 1e-10);
    }

    #[test]
    fn test_compute_cost_zero_tokens() {
        let cost = compute_cost("claude-sonnet-4-20250514", 0, 0);
        assert!((cost - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_compute_cost_haiku() {
        // 10000 input at $0.25/M = $0.0025, 5000 output at $1.25/M = $0.00625
        let cost = compute_cost("claude-haiku-3-20240307", 10000, 5000);
        let expected = 0.0025 + 0.00625;
        assert!((cost - expected).abs() < 1e-10);
    }

    // --- New tests for added types ---

    #[test]
    fn test_image_source_serde_roundtrip() {
        let source = ImageSource {
            source_type: "base64".to_string(),
            media_type: "image/png".to_string(),
            data: "iVBORw0KGgo=".to_string(),
        };
        let json = serde_json::to_value(&source).unwrap();
        assert_eq!(json["type"], "base64");
        assert_eq!(json["media_type"], "image/png");
        assert_eq!(json["data"], "iVBORw0KGgo=");

        let deserialized: ImageSource = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized, source);
    }

    #[test]
    fn test_content_block_image_roundtrip() {
        let block = ContentBlock::Image {
            source: ImageSource {
                source_type: "base64".to_string(),
                media_type: "image/jpeg".to_string(),
                data: "abc123".to_string(),
            },
        };
        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["type"], "image");
        assert_eq!(json["source"]["type"], "base64");
        assert_eq!(json["source"]["media_type"], "image/jpeg");

        let deserialized: ContentBlock = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized, block);
    }

    #[test]
    fn test_content_block_thinking_roundtrip() {
        let block = ContentBlock::Thinking {
            thinking: "Let me reason about this...".to_string(),
            signature: "abc123sig".to_string(),
        };
        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["type"], "thinking");
        assert_eq!(json["thinking"], "Let me reason about this...");
        assert_eq!(json["signature"], "abc123sig");

        let deserialized: ContentBlock = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized, block);
    }

    #[test]
    fn test_content_block_thinking_signature_preserved_in_serde() {
        // Simulate a thinking block as returned by the Anthropic API
        let api_json = json!({
            "type": "thinking",
            "thinking": "Step 1: analyze the problem...",
            "signature": "EqoBCkgIAxgCIkDX2m6FqDKrheMdHGEr"
        });
        let block: ContentBlock = serde_json::from_value(api_json).unwrap();
        match &block {
            ContentBlock::Thinking {
                thinking,
                signature,
            } => {
                assert_eq!(thinking, "Step 1: analyze the problem...");
                assert_eq!(signature, "EqoBCkgIAxgCIkDX2m6FqDKrheMdHGEr");
            }
            _ => panic!("Expected Thinking variant"),
        }

        // Verify the signature is preserved when re-serialized (sent back to API)
        let re_serialized = serde_json::to_value(&block).unwrap();
        assert_eq!(
            re_serialized["signature"],
            "EqoBCkgIAxgCIkDX2m6FqDKrheMdHGEr"
        );
    }

    #[test]
    fn test_tool_result_content_text_serializes_to_string() {
        let content = ToolResultContent::Text("hello".to_string());
        let json = serde_json::to_value(&content).unwrap();
        assert_eq!(json, json!("hello"));
    }

    #[test]
    fn test_tool_result_content_blocks_serializes_to_array() {
        let content = ToolResultContent::Blocks(vec![
            ContentBlock::Text {
                text: "result text".to_string(),
            },
            ContentBlock::Image {
                source: ImageSource {
                    source_type: "base64".to_string(),
                    media_type: "image/png".to_string(),
                    data: "data123".to_string(),
                },
            },
        ]);
        let json = serde_json::to_value(&content).unwrap();
        assert!(json.is_array());
        assert_eq!(json.as_array().unwrap().len(), 2);
        assert_eq!(json[0]["type"], "text");
        assert_eq!(json[1]["type"], "image");
    }

    #[test]
    fn test_tool_result_content_deserialize_from_string() {
        let content: ToolResultContent = serde_json::from_str("\"hello world\"").unwrap();
        assert_eq!(content, ToolResultContent::Text("hello world".to_string()));
    }

    #[test]
    fn test_tool_result_content_deserialize_from_array() {
        let json = json!([{"type": "text", "text": "block content"}]);
        let content: ToolResultContent = serde_json::from_value(json).unwrap();
        match content {
            ToolResultContent::Blocks(blocks) => {
                assert_eq!(blocks.len(), 1);
                assert!(
                    matches!(&blocks[0], ContentBlock::Text { text } if text == "block content")
                );
            }
            _ => panic!("Expected Blocks variant"),
        }
    }

    #[test]
    fn test_thinking_config_serde_roundtrip() {
        let config = ThinkingConfig {
            thinking_type: "enabled".to_string(),
            budget_tokens: 10000,
        };
        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["type"], "enabled");
        assert_eq!(json["budget_tokens"], 10000);

        let deserialized: ThinkingConfig = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized, config);
    }

    #[test]
    fn test_thinking_config_enabled_constructor() {
        let config = ThinkingConfig::enabled(5000);
        assert_eq!(config.thinking_type, "enabled");
        assert_eq!(config.budget_tokens, 5000);
    }

    #[test]
    fn test_system_content_text_roundtrip() {
        let content = SystemContent::text("You are an agent.");
        let json = serde_json::to_value(&content).unwrap();
        assert_eq!(json["type"], "text");
        assert_eq!(json["text"], "You are an agent.");
        assert!(json.get("cache_control").is_none());

        let deserialized: SystemContent = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized, content);
    }

    #[test]
    fn test_system_content_text_cached_roundtrip() {
        let content = SystemContent::text_cached("Cached system prompt");
        let json = serde_json::to_value(&content).unwrap();
        assert_eq!(json["type"], "text");
        assert_eq!(json["text"], "Cached system prompt");
        assert_eq!(json["cache_control"]["type"], "ephemeral");

        let deserialized: SystemContent = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized, content);
    }

    #[test]
    fn test_system_content_text_constructor() {
        let content = SystemContent::text("hello");
        match &content {
            SystemContent::Text {
                text,
                cache_control,
            } => {
                assert_eq!(text, "hello");
                assert!(cache_control.is_none());
            }
        }
    }

    #[test]
    fn test_system_content_text_cached_constructor() {
        let content = SystemContent::text_cached("cached");
        match &content {
            SystemContent::Text {
                text,
                cache_control,
            } => {
                assert_eq!(text, "cached");
                assert!(cache_control.is_some());
                assert_eq!(cache_control.as_ref().unwrap().cache_type, "ephemeral");
            }
        }
    }

    #[test]
    fn test_cache_control_ephemeral_constructor() {
        let cc = CacheControl::ephemeral();
        assert_eq!(cc.cache_type, "ephemeral");
    }

    #[test]
    fn test_content_has_tool_use_with_tool_use() {
        let content = vec![
            ContentBlock::Text {
                text: "hello".to_string(),
            },
            ContentBlock::ToolUse {
                id: "toolu_1".to_string(),
                name: "navigate".to_string(),
                input: json!({}),
            },
        ];
        assert!(content_has_tool_use(&content));
    }

    #[test]
    fn test_content_has_tool_use_without_tool_use() {
        let content = vec![ContentBlock::Text {
            text: "hello".to_string(),
        }];
        assert!(!content_has_tool_use(&content));
    }

    #[test]
    fn test_content_has_tool_use_empty() {
        assert!(!content_has_tool_use(&[]));
    }

    #[test]
    fn test_cache_control_serde_roundtrip() {
        let cc = CacheControl::ephemeral();
        let json = serde_json::to_value(&cc).unwrap();
        assert_eq!(json["type"], "ephemeral");

        let deserialized: CacheControl = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized, cc);
    }

    #[test]
    fn test_messages_request_with_thinking() {
        let request = MessagesRequest {
            model: "claude-sonnet-4-5-20250929".to_string(),
            max_tokens: 16000,
            system: None,
            messages: vec![],
            tools: None,
            thinking: Some(ThinkingConfig::enabled(10000)),
            stream: None,
            cache_breakpoint: None,
        };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["thinking"]["type"], "enabled");
        assert_eq!(json["thinking"]["budget_tokens"], 10000);
        assert!(json.get("stream").is_none());
    }

    #[test]
    fn test_messages_request_with_stream() {
        let request = MessagesRequest {
            model: "claude-sonnet-4-5-20250929".to_string(),
            max_tokens: 4096,
            system: None,
            messages: vec![],
            tools: None,
            thinking: None,
            stream: Some(true),
            cache_breakpoint: None,
        };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["stream"], true);
        assert!(json.get("thinking").is_none());
    }

    #[test]
    fn test_usage_with_cache_tokens() {
        let usage_json = json!({
            "input_tokens": 100,
            "output_tokens": 50,
            "cache_creation_input_tokens": 2000,
            "cache_read_input_tokens": 500
        });
        let usage: Usage = serde_json::from_value(usage_json).unwrap();
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.cache_creation_input_tokens, Some(2000));
        assert_eq!(usage.cache_read_input_tokens, Some(500));

        let json = serde_json::to_value(&usage).unwrap();
        assert_eq!(json["cache_creation_input_tokens"], 2000);
        assert_eq!(json["cache_read_input_tokens"], 500);
    }

    #[test]
    fn test_tool_definition_with_cache_control() {
        let tool = ToolDefinition {
            name: "navigate".to_string(),
            description: "Navigate to a URL".to_string(),
            input_schema: json!({"type": "object", "properties": {}}),
            cache_control: Some(CacheControl::ephemeral()),
            read_only: false,
        };
        let json = serde_json::to_value(&tool).unwrap();
        assert_eq!(json["cache_control"]["type"], "ephemeral");

        let deserialized: ToolDefinition =
            serde_json::from_str(&serde_json::to_string(&tool).unwrap()).unwrap();
        assert_eq!(deserialized.name, "navigate");
        assert!(deserialized.cache_control.is_some());
    }

    // --- Conversation cache breakpoint ---
    //
    // Without a breakpoint the whole growing history is re-sent uncached every
    // iteration, which on a long run is the dominant cost.

    fn msg(role: Role, text: &str) -> Message {
        Message {
            role,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
        }
    }

    fn conversation(n: usize) -> Vec<Message> {
        (0..n)
            .map(|i| {
                let role = if i % 2 == 0 {
                    Role::User
                } else {
                    Role::Assistant
                };
                msg(role, &format!("message {i}"))
            })
            .collect()
    }

    #[test]
    fn test_no_breakpoint_for_short_conversations() {
        for n in 0..5 {
            assert_eq!(
                conversation_cache_breakpoint(&conversation(n)),
                None,
                "a {n}-message conversation should not burn a breakpoint"
            );
        }
    }

    #[test]
    fn test_breakpoint_excludes_the_most_recent_exchange() {
        let msgs = conversation(9);
        // The last two messages change every turn; caching through them would
        // invalidate the entry immediately.
        assert_eq!(conversation_cache_breakpoint(&msgs), Some(6));
    }

    #[test]
    fn test_breakpoint_advances_as_the_conversation_grows() {
        let a = conversation_cache_breakpoint(&conversation(7)).unwrap();
        let b = conversation_cache_breakpoint(&conversation(9)).unwrap();
        assert!(b > a, "breakpoint should move forward: {a} then {b}");
    }

    #[test]
    fn test_breakpoint_skips_blocks_that_cannot_carry_it() {
        // Thinking blocks reject cache_control, and marking one fails the whole request.
        let mut msgs = conversation(9);
        msgs[6].content = vec![ContentBlock::Thinking {
            thinking: "hmm".to_string(),
            signature: "sig".to_string(),
        }];
        assert_eq!(conversation_cache_breakpoint(&msgs), None);
    }

    fn wire_json(messages: Vec<Message>, breakpoint: Option<usize>) -> Value {
        let request = MessagesRequest {
            model: "test-model".to_string(),
            max_tokens: 100,
            system: None,
            messages,
            tools: None,
            thinking: None,
            stream: None,
            cache_breakpoint: breakpoint,
        };
        serde_json::to_value(request.to_wire()).unwrap()
    }

    #[test]
    fn test_wire_form_splices_cache_control_into_the_marked_block() {
        let json = wire_json(conversation(9), Some(6));
        let block = &json["messages"][6]["content"][0];
        assert_eq!(block["type"], "text", "block shape was altered: {block}");
        assert_eq!(block["text"], "message 6");
        assert_eq!(block["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn test_wire_form_leaves_other_blocks_untouched() {
        let json = wire_json(conversation(9), Some(6));
        for i in [0usize, 5, 7, 8] {
            let block = &json["messages"][i]["content"][0];
            assert!(
                block.get("cache_control").is_none(),
                "message {i} should not carry a breakpoint: {block}"
            );
        }
    }

    #[test]
    fn test_wire_form_without_breakpoint_matches_plain_serialization() {
        // The wire form must be a faithful rendering when no breakpoint is set.
        let msgs = conversation(9);
        let with_wire = wire_json(msgs.clone(), None);
        let plain = serde_json::json!({
            "model": "test-model",
            "max_tokens": 100,
            "messages": msgs,
        });
        assert_eq!(with_wire, plain);
    }

    #[test]
    fn test_wire_form_marks_only_the_last_block_of_the_message() {
        let mut msgs = conversation(9);
        msgs[6].content = vec![
            ContentBlock::Text {
                text: "first".to_string(),
            },
            ContentBlock::Text {
                text: "last".to_string(),
            },
        ];
        let json = wire_json(msgs, Some(6));
        assert!(json["messages"][6]["content"][0]
            .get("cache_control")
            .is_none());
        assert_eq!(
            json["messages"][6]["content"][1]["cache_control"]["type"],
            "ephemeral"
        );
    }

    #[test]
    fn test_wire_form_handles_tool_result_blocks() {
        let msgs = vec![
            msg(Role::User, "task"),
            msg(Role::Assistant, "working"),
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "t1".to_string(),
                    content: ToolResultContent::Text("ok".to_string()),
                    is_error: None,
                }],
            },
            msg(Role::Assistant, "more"),
            msg(Role::User, "next"),
        ];
        let json = wire_json(msgs, Some(2));
        let block = &json["messages"][2]["content"][0];
        assert_eq!(block["type"], "tool_result");
        assert_eq!(block["tool_use_id"], "t1");
        assert_eq!(block["cache_control"]["type"], "ephemeral");
    }
}
