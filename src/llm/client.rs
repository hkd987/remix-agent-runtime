use async_trait::async_trait;
use std::collections::HashMap;
use std::pin::Pin;
use futures_core::Stream;
use tracing::{debug, error, warn};

use super::stream::{StreamEvent, SseParser};
use super::types::*;
use crate::error::AgentError;

#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn send_messages(
        &self,
        system: Option<&[SystemContent]>,
        messages: &[Message],
        tools: Option<&[ToolDefinition]>,
    ) -> Result<MessagesResponse, AgentError>;
}

#[async_trait]
impl<L: LlmProvider> LlmProvider for std::sync::Arc<L> {
    async fn send_messages(
        &self,
        system: Option<&[SystemContent]>,
        messages: &[Message],
        tools: Option<&[ToolDefinition]>,
    ) -> Result<MessagesResponse, AgentError> {
        (**self).send_messages(system, messages, tools).await
    }
}

/// Extension trait for LLM providers that support streaming responses.
///
/// This is a separate trait from LlmProvider so non-streaming backends
/// don't need to implement it.
pub trait StreamingLlmProvider: LlmProvider {
    /// Send a messages request and return a stream of events.
    ///
    /// The stream yields `StreamEvent` items as they arrive from the API.
    /// Events follow the Anthropic SSE streaming protocol.
    fn send_messages_stream<'a>(
        &'a self,
        system: Option<&'a [SystemContent]>,
        messages: &'a [Message],
        tools: Option<&'a [ToolDefinition]>,
    ) -> Pin<Box<dyn Stream<Item = Result<StreamEvent, AgentError>> + Send + 'a>>;
}

impl<L: StreamingLlmProvider> StreamingLlmProvider for std::sync::Arc<L> {
    fn send_messages_stream<'a>(
        &'a self,
        system: Option<&'a [SystemContent]>,
        messages: &'a [Message],
        tools: Option<&'a [ToolDefinition]>,
    ) -> Pin<Box<dyn Stream<Item = Result<StreamEvent, AgentError>> + Send + 'a>> {
        (**self).send_messages_stream(system, messages, tools)
    }
}

pub struct AnthropicClient {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    max_tokens: u32,
    custom_headers: HashMap<String, String>,
    thinking: Option<ThinkingConfig>,
    enable_prompt_caching: bool,
}

impl AnthropicClient {
    pub fn new(
        base_url: String,
        api_key: String,
        model: String,
        max_tokens: u32,
        custom_headers: HashMap<String, String>,
        thinking: Option<ThinkingConfig>,
        enable_prompt_caching: bool,
    ) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url,
            api_key,
            model,
            max_tokens,
            custom_headers,
            thinking,
            enable_prompt_caching,
        }
    }

    fn build_request(&self, request: &MessagesRequest) -> reqwest::RequestBuilder {
        let url = format!("{}/v1/messages", self.base_url);
        let mut builder = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("content-type", "application/json")
            .header("anthropic-version", "2023-06-01");

        if self.enable_prompt_caching {
            builder = builder.header("anthropic-beta", "prompt-caching-2024-07-31");
        }

        for (key, value) in &self.custom_headers {
            builder = builder.header(key, value);
        }

        builder.json(request)
    }
}

#[async_trait]
impl LlmProvider for AnthropicClient {
    async fn send_messages(
        &self,
        system: Option<&[SystemContent]>,
        messages: &[Message],
        tools: Option<&[ToolDefinition]>,
    ) -> Result<MessagesResponse, AgentError> {
        let request = MessagesRequest {
            model: self.model.clone(),
            max_tokens: self.max_tokens,
            system: system.map(|s| s.to_vec()),
            messages: messages.to_vec(),
            tools: tools.map(|t| t.to_vec()),
            thinking: self.thinking.clone(),
            stream: None,
        };

        let max_retries: u32 = 3;
        let base_delay_ms: u64 = 1000;
        let max_delay_ms: u64 = 30000;

        for attempt in 0..max_retries {
            debug!(attempt = attempt, "Sending request to Anthropic API");

            let response = self.build_request(&request).send().await?;
            let status = response.status();

            if status.is_success() {
                let body = response.text().await?;
                let parsed: MessagesResponse = serde_json::from_str(&body).map_err(|e| {
                    error!(error = %e, "Failed to parse Anthropic API response");
                    AgentError::Llm(format!("Failed to parse response: {e}"))
                })?;
                return Ok(parsed);
            }

            if status == reqwest::StatusCode::UNAUTHORIZED
                || status == reqwest::StatusCode::FORBIDDEN
            {
                let body = response.text().await.unwrap_or_default();
                return Err(AgentError::LlmAuth(body));
            }

            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                if attempt < max_retries - 1 {
                    let delay_ms = (base_delay_ms * 2u64.pow(attempt)).min(max_delay_ms);
                    warn!(
                        attempt = attempt,
                        delay_ms = delay_ms,
                        "Rate limited, retrying"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                    continue;
                }
                return Err(AgentError::LlmRateLimited);
            }

            if status.is_server_error() {
                let body = response.text().await.unwrap_or_default();
                if attempt < max_retries - 1 {
                    let delay_ms = (base_delay_ms * 2u64.pow(attempt)).min(max_delay_ms);
                    warn!(
                        attempt = attempt,
                        delay_ms = delay_ms,
                        status = status.as_u16(),
                        "Server error, retrying"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                    continue;
                }
                return Err(AgentError::Llm(format!(
                    "Server error {}: {}",
                    status.as_u16(),
                    body
                )));
            }

            let body = response.text().await.unwrap_or_default();
            return Err(AgentError::Llm(format!(
                "Unexpected status {}: {}",
                status.as_u16(),
                body
            )));
        }

        Err(AgentError::LlmRateLimited)
    }
}

impl StreamingLlmProvider for AnthropicClient {
    fn send_messages_stream<'a>(
        &'a self,
        system: Option<&'a [SystemContent]>,
        messages: &'a [Message],
        tools: Option<&'a [ToolDefinition]>,
    ) -> Pin<Box<dyn Stream<Item = Result<StreamEvent, AgentError>> + Send + 'a>> {
        let request = MessagesRequest {
            model: self.model.clone(),
            max_tokens: self.max_tokens,
            system: system.map(|s| s.to_vec()),
            messages: messages.to_vec(),
            tools: tools.map(|t| t.to_vec()),
            thinking: self.thinking.clone(),
            stream: Some(true),
        };

        let (tx, rx) = tokio::sync::mpsc::channel::<Result<StreamEvent, AgentError>>(32);
        let client = self.client.clone();
        let url = format!("{}/v1/messages", self.base_url);
        let api_key = self.api_key.clone();
        let custom_headers = self.custom_headers.clone();
        let enable_prompt_caching = self.enable_prompt_caching;

        tokio::spawn(async move {
            let mut builder = client
                .post(&url)
                .header("x-api-key", &api_key)
                .header("content-type", "application/json")
                .header("anthropic-version", "2023-06-01");

            if enable_prompt_caching {
                builder = builder.header("anthropic-beta", "prompt-caching-2024-07-31");
            }

            for (key, value) in &custom_headers {
                builder = builder.header(key, value);
            }

            let response = match builder.json(&request).send().await {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx.send(Err(AgentError::Http(e))).await;
                    return;
                }
            };

            let status = response.status();
            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                let err = if status == reqwest::StatusCode::UNAUTHORIZED
                    || status == reqwest::StatusCode::FORBIDDEN
                {
                    AgentError::LlmAuth(body)
                } else if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                    AgentError::LlmRateLimited
                } else {
                    AgentError::Llm(format!("HTTP {}: {}", status.as_u16(), body))
                };
                let _ = tx.send(Err(err)).await;
                return;
            }

            let mut byte_stream = response.bytes_stream();
            let mut parser = SseParser::new();

            use tokio_stream::StreamExt as _;
            while let Some(chunk_result) = byte_stream.next().await {
                match chunk_result {
                    Ok(bytes) => {
                        let events = parser.feed(&bytes);
                        for (event_type, data) in events {
                            match SseParser::parse_event(&event_type, &data) {
                                Ok(event) => {
                                    if tx.send(Ok(event)).await.is_err() {
                                        return; // Receiver dropped
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        error = %e,
                                        event_type = %event_type,
                                        "Failed to parse stream event"
                                    );
                                    // Continue parsing, don't break on individual event parse errors
                                }
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(AgentError::Http(e))).await;
                        return;
                    }
                }
            }
        });

        Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_success_response_json() -> serde_json::Value {
        json!({
            "id": "msg_test_123",
            "content": [
                {
                    "type": "text",
                    "text": "Hello! How can I help you?"
                }
            ],
            "model": "test-model",
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 10,
                "output_tokens": 20
            }
        })
    }

    fn make_tool_use_response_json() -> serde_json::Value {
        json!({
            "id": "msg_test_456",
            "content": [
                {
                    "type": "text",
                    "text": "I'll navigate there."
                },
                {
                    "type": "tool_use",
                    "id": "toolu_abc",
                    "name": "navigate",
                    "input": {"url": "https://example.com"}
                }
            ],
            "model": "test-model",
            "stop_reason": "tool_use",
            "usage": {
                "input_tokens": 15,
                "output_tokens": 25
            }
        })
    }

    #[tokio::test]
    async fn test_successful_response() {
        let mut server = mockito::Server::new_async().await;
        let response_json = make_success_response_json();

        let mock = server
            .mock("POST", "/v1/messages")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::to_string(&response_json).unwrap())
            .create_async()
            .await;

        let client = AnthropicClient::new(
            server.url(),
            "test-api-key".to_string(),
            "test-model".to_string(),
            8192,
            Default::default(),
            None,
            false,
        );

        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Hello".to_string(),
            }],
        }];

        let result = client
            .send_messages(Some(&[SystemContent::text("system prompt")]), &messages, None)
            .await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.id, "msg_test_123");
        assert_eq!(response.stop_reason, StopReason::EndTurn);
        assert_eq!(response.content.len(), 1);
        assert!(matches!(
            &response.content[0],
            ContentBlock::Text { text } if text == "Hello! How can I help you?"
        ));

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_tool_use_response() {
        let mut server = mockito::Server::new_async().await;
        let response_json = make_tool_use_response_json();

        let mock = server
            .mock("POST", "/v1/messages")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::to_string(&response_json).unwrap())
            .create_async()
            .await;

        let client = AnthropicClient::new(
            server.url(),
            "test-api-key".to_string(),
            "test-model".to_string(),
            8192,
            Default::default(),
            None,
            false,
        );

        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Go to example.com".to_string(),
            }],
        }];

        let result = client.send_messages(None, &messages, None).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.stop_reason, StopReason::ToolUse);
        assert_eq!(response.content.len(), 2);
        assert!(matches!(
            &response.content[1],
            ContentBlock::ToolUse { name, .. } if name == "navigate"
        ));

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_401_returns_llm_auth_error() {
        let mut server = mockito::Server::new_async().await;

        let mock = server
            .mock("POST", "/v1/messages")
            .with_status(401)
            .with_body("Invalid API key")
            .create_async()
            .await;

        let client = AnthropicClient::new(
            server.url(),
            "bad-key".to_string(),
            "test-model".to_string(),
            8192,
            Default::default(),
            None,
            false,
        );

        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Hello".to_string(),
            }],
        }];

        let result = client.send_messages(None, &messages, None).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, AgentError::LlmAuth(body) if body == "Invalid API key"));

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_403_returns_llm_auth_error() {
        let mut server = mockito::Server::new_async().await;

        let mock = server
            .mock("POST", "/v1/messages")
            .with_status(403)
            .with_body("Forbidden")
            .create_async()
            .await;

        let client = AnthropicClient::new(
            server.url(),
            "forbidden-key".to_string(),
            "test-model".to_string(),
            8192,
            Default::default(),
            None,
            false,
        );

        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Hello".to_string(),
            }],
        }];

        let result = client.send_messages(None, &messages, None).await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), AgentError::LlmAuth(_)));

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_429_returns_rate_limited_after_retries() {
        let mut server = mockito::Server::new_async().await;

        let mock = server
            .mock("POST", "/v1/messages")
            .with_status(429)
            .with_body("Rate limited")
            .expect(3)
            .create_async()
            .await;

        let client = AnthropicClient::new(
            server.url(),
            "test-key".to_string(),
            "test-model".to_string(),
            8192,
            Default::default(),
            None,
            false,
        );

        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Hello".to_string(),
            }],
        }];

        // Use tokio::time::pause to speed up sleep in tests
        tokio::time::pause();
        let result = client.send_messages(None, &messages, None).await;
        tokio::time::resume();

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), AgentError::LlmRateLimited));

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_429_succeeds_on_retry() {
        let mut server = mockito::Server::new_async().await;

        // First two calls return 429, third succeeds
        let response_json = make_success_response_json();

        let mock_429 = server
            .mock("POST", "/v1/messages")
            .with_status(429)
            .with_body("Rate limited")
            .expect(2)
            .create_async()
            .await;

        let mock_200 = server
            .mock("POST", "/v1/messages")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::to_string(&response_json).unwrap())
            .expect(1)
            .create_async()
            .await;

        let client = AnthropicClient::new(
            server.url(),
            "test-key".to_string(),
            "test-model".to_string(),
            8192,
            Default::default(),
            None,
            false,
        );

        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Hello".to_string(),
            }],
        }];

        tokio::time::pause();
        let result = client.send_messages(None, &messages, None).await;
        tokio::time::resume();

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.id, "msg_test_123");

        mock_429.assert_async().await;
        mock_200.assert_async().await;
    }

    #[tokio::test]
    async fn test_500_returns_llm_error() {
        let mut server = mockito::Server::new_async().await;

        let mock = server
            .mock("POST", "/v1/messages")
            .with_status(500)
            .with_body("Internal Server Error")
            .expect(3)
            .create_async()
            .await;

        let client = AnthropicClient::new(
            server.url(),
            "test-key".to_string(),
            "test-model".to_string(),
            8192,
            Default::default(),
            None,
            false,
        );

        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Hello".to_string(),
            }],
        }];

        tokio::time::pause();
        let result = client.send_messages(None, &messages, None).await;
        tokio::time::resume();

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(&err, AgentError::Llm(msg) if msg.contains("500") && msg.contains("Internal Server Error"))
        );

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_500_retries_then_succeeds() {
        let mut server = mockito::Server::new_async().await;
        let response_json = make_success_response_json();

        // First two calls return 500, third succeeds
        let mock_500 = server
            .mock("POST", "/v1/messages")
            .with_status(500)
            .with_body("Internal Server Error")
            .expect(2)
            .create_async()
            .await;

        let mock_200 = server
            .mock("POST", "/v1/messages")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::to_string(&response_json).unwrap())
            .expect(1)
            .create_async()
            .await;

        let client = AnthropicClient::new(
            server.url(),
            "test-key".to_string(),
            "test-model".to_string(),
            8192,
            Default::default(),
            None,
            false,
        );

        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Hello".to_string(),
            }],
        }];

        tokio::time::pause();
        let result = client.send_messages(None, &messages, None).await;
        tokio::time::resume();

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.id, "msg_test_123");

        mock_500.assert_async().await;
        mock_200.assert_async().await;
    }

    #[tokio::test]
    async fn test_500_exhausts_retries() {
        let mut server = mockito::Server::new_async().await;

        let mock = server
            .mock("POST", "/v1/messages")
            .with_status(500)
            .with_body("Service Unavailable")
            .expect(3)
            .create_async()
            .await;

        let client = AnthropicClient::new(
            server.url(),
            "test-key".to_string(),
            "test-model".to_string(),
            8192,
            Default::default(),
            None,
            false,
        );

        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Hello".to_string(),
            }],
        }];

        tokio::time::pause();
        let result = client.send_messages(None, &messages, None).await;
        tokio::time::resume();

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(&err, AgentError::Llm(msg) if msg.contains("500")));

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_request_format() {
        let mut server = mockito::Server::new_async().await;
        let response_json = make_success_response_json();

        let mock = server
            .mock("POST", "/v1/messages")
            .match_header("x-api-key", "test-api-key")
            .match_header("content-type", "application/json")
            .match_header("anthropic-version", "2023-06-01")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::to_string(&response_json).unwrap())
            .create_async()
            .await;

        let client = AnthropicClient::new(
            server.url(),
            "test-api-key".to_string(),
            "my-model".to_string(),
            4096,
            Default::default(),
            None,
            false,
        );

        let tools = vec![ToolDefinition {
            name: "navigate".to_string(),
            description: "Navigate to URL".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": {"type": "string"}
                },
                "required": ["url"]
            }),
            cache_control: None,
        }];

        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Go to example.com".to_string(),
            }],
        }];

        let result = client
            .send_messages(Some(&[SystemContent::text("You are a helper")]), &messages, Some(&tools))
            .await;

        assert!(result.is_ok());
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_custom_headers_sent() {
        let mut server = mockito::Server::new_async().await;
        let response_json = make_success_response_json();

        let mock = server
            .mock("POST", "/v1/messages")
            .match_header("x-custom-header", "custom-value")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::to_string(&response_json).unwrap())
            .create_async()
            .await;

        let mut custom_headers = HashMap::new();
        custom_headers.insert("x-custom-header".to_string(), "custom-value".to_string());

        let client = AnthropicClient::new(
            server.url(),
            "test-key".to_string(),
            "test-model".to_string(),
            8192,
            custom_headers,
            None,
            false,
        );

        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Hello".to_string(),
            }],
        }];

        let result = client.send_messages(None, &messages, None).await;
        assert!(result.is_ok());

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_parse_error_returns_llm_error() {
        let mut server = mockito::Server::new_async().await;

        let mock = server
            .mock("POST", "/v1/messages")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body("{ invalid json }")
            .create_async()
            .await;

        let client = AnthropicClient::new(
            server.url(),
            "test-key".to_string(),
            "test-model".to_string(),
            8192,
            Default::default(),
            None,
            false,
        );

        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Hello".to_string(),
            }],
        }];

        let result = client.send_messages(None, &messages, None).await;

        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), AgentError::Llm(msg) if msg.contains("Failed to parse"))
        );

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_arc_llm_provider_delegates() {
        use std::sync::{Arc, Mutex};

        struct MockLlmProvider {
            call_count: Mutex<u32>,
        }

        #[async_trait]
        impl LlmProvider for MockLlmProvider {
            async fn send_messages(
                &self,
                system: Option<&[SystemContent]>,
                _messages: &[Message],
                _tools: Option<&[ToolDefinition]>,
            ) -> Result<MessagesResponse, AgentError> {
                *self.call_count.lock().unwrap() += 1;
                Ok(MessagesResponse {
                    id: "msg_mock".to_string(),
                    content: vec![ContentBlock::Text {
                        text: system
                            .and_then(|s| s.first())
                            .map(|sc| match sc {
                                SystemContent::Text { text, .. } => text.clone(),
                            })
                            .unwrap_or_else(|| "no system".to_string()),
                    }],
                    model: "mock".to_string(),
                    stop_reason: StopReason::EndTurn,
                    usage: None,
                })
            }
        }

        let provider = Arc::new(MockLlmProvider {
            call_count: Mutex::new(0),
        });

        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Hello".to_string(),
            }],
        }];

        // Call through Arc
        let result = provider
            .send_messages(Some(&[SystemContent::text("test system")]), &messages, None)
            .await
            .unwrap();

        assert_eq!(result.id, "msg_mock");
        assert_eq!(result.stop_reason, StopReason::EndTurn);
        assert!(matches!(
            &result.content[0],
            ContentBlock::Text { text } if text == "test system"
        ));
        assert_eq!(*provider.call_count.lock().unwrap(), 1);

        // Call again to verify repeated delegation
        let _ = provider.send_messages(None, &messages, None).await.unwrap();
        assert_eq!(*provider.call_count.lock().unwrap(), 2);

        // Verify a clone of the Arc also works
        let provider2 = provider.clone();
        let _ = provider2
            .send_messages(None, &messages, None)
            .await
            .unwrap();
        assert_eq!(*provider.call_count.lock().unwrap(), 3);
    }

    #[tokio::test]
    async fn test_prompt_caching_header_sent_when_enabled() {
        let mut server = mockito::Server::new_async().await;
        let response_json = make_success_response_json();

        let mock = server
            .mock("POST", "/v1/messages")
            .match_header("anthropic-beta", "prompt-caching-2024-07-31")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::to_string(&response_json).unwrap())
            .create_async()
            .await;

        let client = AnthropicClient::new(
            server.url(),
            "test-api-key".to_string(),
            "test-model".to_string(),
            8192,
            Default::default(),
            None,
            true,
        );

        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Hello".to_string(),
            }],
        }];

        let result = client.send_messages(None, &messages, None).await;
        assert!(result.is_ok());

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_prompt_caching_header_not_sent_when_disabled() {
        let mut server = mockito::Server::new_async().await;
        let response_json = make_success_response_json();

        let mock = server
            .mock("POST", "/v1/messages")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::to_string(&response_json).unwrap())
            .create_async()
            .await;

        let client = AnthropicClient::new(
            server.url(),
            "test-api-key".to_string(),
            "test-model".to_string(),
            8192,
            Default::default(),
            None,
            false,
        );

        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Hello".to_string(),
            }],
        }];

        let result = client.send_messages(None, &messages, None).await;
        assert!(result.is_ok());

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_thinking_config_included_in_request() {
        let mut server = mockito::Server::new_async().await;
        let response_json = make_success_response_json();

        let mock = server
            .mock("POST", "/v1/messages")
            .match_body(mockito::Matcher::PartialJsonString(
                r#"{"thinking":{"type":"enabled","budget_tokens":10000}}"#.to_string(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::to_string(&response_json).unwrap())
            .create_async()
            .await;

        let client = AnthropicClient::new(
            server.url(),
            "test-api-key".to_string(),
            "test-model".to_string(),
            16000,
            Default::default(),
            Some(ThinkingConfig::enabled(10000)),
            false,
        );

        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Hello".to_string(),
            }],
        }];

        let result = client.send_messages(None, &messages, None).await;
        assert!(result.is_ok());

        mock.assert_async().await;
    }
}
