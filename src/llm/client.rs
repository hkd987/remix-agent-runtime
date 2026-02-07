use async_trait::async_trait;
use std::collections::HashMap;
use tracing::{debug, error, warn};

use super::types::*;
use crate::error::AgentError;

#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn send_messages(
        &self,
        system: Option<&str>,
        messages: &[Message],
        tools: Option<&[ToolDefinition]>,
    ) -> Result<MessagesResponse, AgentError>;
}

pub struct AnthropicClient {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    max_tokens: u32,
    custom_headers: HashMap<String, String>,
}

impl AnthropicClient {
    pub fn new(
        base_url: String,
        api_key: String,
        model: String,
        max_tokens: u32,
        custom_headers: HashMap<String, String>,
    ) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url,
            api_key,
            model,
            max_tokens,
            custom_headers,
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
        system: Option<&str>,
        messages: &[Message],
        tools: Option<&[ToolDefinition]>,
    ) -> Result<MessagesResponse, AgentError> {
        let request = MessagesRequest {
            model: self.model.clone(),
            max_tokens: self.max_tokens,
            system: system.map(String::from),
            messages: messages.to_vec(),
            tools: tools.map(|t| t.to_vec()),
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
        );

        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Hello".to_string(),
            }],
        }];

        let result = client
            .send_messages(Some("system prompt"), &messages, None)
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
            .create_async()
            .await;

        let client = AnthropicClient::new(
            server.url(),
            "test-key".to_string(),
            "test-model".to_string(),
            8192,
            Default::default(),
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
        assert!(
            matches!(&err, AgentError::Llm(msg) if msg.contains("500") && msg.contains("Internal Server Error"))
        );

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
        }];

        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Go to example.com".to_string(),
            }],
        }];

        let result = client
            .send_messages(Some("You are a helper"), &messages, Some(&tools))
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
}
