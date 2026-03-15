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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    StopSequence,
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

/// Compute total cost from token counts and model.
pub fn compute_cost(model: &str, input_tokens: u32, output_tokens: u32) -> f64 {
    let (input_rate, output_rate) = model_pricing(model);
    (input_tokens as f64 * input_rate) + (output_tokens as f64 * output_rate)
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
        };
        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["type"], "thinking");
        assert_eq!(json["thinking"], "Let me reason about this...");

        let deserialized: ContentBlock = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized, block);
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
}
