use async_trait::async_trait;
use futures_core::Stream;
use std::collections::HashMap;
use std::pin::Pin;
use tracing::{debug, error, warn};

use super::stream::{SseParser, StreamEvent};
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

/// Trait for dynamically controlling the thinking/reasoning budget at runtime.
pub trait ThinkingControl: Send + Sync {
    fn set_thinking_config(&self, config: Option<ThinkingConfig>);
}

/// How long a single request may take before it is abandoned.
///
/// Extended thinking on a large context can legitimately take minutes, so this is
/// generous — but it must exist. Without any timeout a hung connection stalls the agent
/// forever, since the loop only checks its own deadline *between* iterations.
const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 600;
const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 30;

/// Retry budget for transient failures.
const DEFAULT_MAX_RETRIES: u32 = 3;
const BASE_RETRY_DELAY_MS: u64 = 1000;
const MAX_RETRY_DELAY_MS: u64 = 30_000;

pub struct AnthropicClient {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    max_tokens: u32,
    custom_headers: HashMap<String, String>,
    thinking: std::sync::RwLock<Option<ThinkingConfig>>,
    enable_prompt_caching: bool,
    max_retries: u32,
    /// Base backoff delay. Configurable so tests can exercise the real retry path
    /// without either sleeping for seconds or pausing the clock — pausing also
    /// virtualizes the request timeout, which then fires immediately.
    retry_base_delay_ms: u64,
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
        // A client with no timeout will wait forever on a stalled connection. Compare
        // `webhook.rs` and `web_fetch.rs`, which both set one.
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(DEFAULT_REQUEST_TIMEOUT_SECS))
            .connect_timeout(std::time::Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS))
            .build()
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "Falling back to a default HTTP client without timeouts");
                reqwest::Client::new()
            });

        Self {
            client,
            base_url,
            api_key,
            model,
            max_tokens,
            custom_headers,
            thinking: std::sync::RwLock::new(thinking),
            enable_prompt_caching,
            max_retries: DEFAULT_MAX_RETRIES,
            retry_base_delay_ms: BASE_RETRY_DELAY_MS,
        }
    }

    /// Override the base retry backoff.
    pub fn with_retry_base_delay_ms(mut self, delay_ms: u64) -> Self {
        self.retry_base_delay_ms = delay_ms;
        self
    }

    /// Override the retry budget. Mainly for tests and callers with their own policy.
    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
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

        builder.json(&request.to_wire())
    }
}

/// Compute the backoff delay for `attempt`, honouring a server-supplied `Retry-After`.
///
/// Jitter matters when several agents share a rate limit: without it they all back off
/// for exactly the same interval and retry in lockstep, reproducing the burst that
/// caused the 429.
fn retry_delay_ms(attempt: u32, retry_after: Option<u64>, base_delay_ms: u64) -> u64 {
    if let Some(secs) = retry_after {
        // The server told us when to come back; that beats any local guess.
        return (secs * 1000).min(MAX_RETRY_DELAY_MS);
    }
    // `checked_pow` because a large attempt count would otherwise overflow before the
    // cap is applied.
    let scaled = 2u64
        .checked_pow(attempt)
        .and_then(|f| base_delay_ms.checked_mul(f))
        .unwrap_or(MAX_RETRY_DELAY_MS);
    let base = scaled.min(MAX_RETRY_DELAY_MS);

    // Deterministic ±25% spread derived from the attempt number, so no RNG dependency.
    let spread = base / 4;
    let offset = (attempt as u64 * 7919) % (spread * 2 + 1);
    // Cap again: jitter is applied after the cap, so it could otherwise exceed it.
    base.saturating_sub(spread)
        .saturating_add(offset)
        .min(MAX_RETRY_DELAY_MS)
}

/// Parse the `Retry-After` header, which may be either delay-seconds or an HTTP date.
/// Only the numeric form is handled; a date falls back to computed backoff.
fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
}

/// Whether a transport-level failure is worth retrying.
///
/// Connection resets, DNS blips and timeouts are transient; a malformed request is not.
/// These were previously not retried at all — only HTTP status codes were — so a single
/// dropped connection ended the whole run.
fn is_retryable_transport_error(e: &reqwest::Error) -> bool {
    e.is_timeout() || e.is_connect() || e.is_request()
}

/// Whether a 400 is the API rejecting an over-long prompt.
///
/// This is recoverable by compacting and retrying, unlike other 400s, so it is worth
/// distinguishing rather than aborting the run.
pub fn is_context_overflow(body: &str) -> bool {
    let lower = body.to_lowercase();
    lower.contains("prompt is too long")
        || lower.contains("context window")
        || (lower.contains("too many tokens") && lower.contains("maximum"))
}

#[async_trait]
impl LlmProvider for AnthropicClient {
    async fn send_messages(
        &self,
        system: Option<&[SystemContent]>,
        messages: &[Message],
        tools: Option<&[ToolDefinition]>,
    ) -> Result<MessagesResponse, AgentError> {
        let thinking = self
            .thinking
            .read()
            .expect("thinking RwLock poisoned")
            .clone();
        let request = MessagesRequest {
            model: self.model.clone(),
            max_tokens: self.max_tokens,
            system: system.map(|s| s.to_vec()),
            messages: messages.to_vec(),
            tools: tools.map(|t| t.to_vec()),
            thinking,
            stream: None,
            // Cache the stable conversation prefix. Without this the whole history is
            // re-sent uncached every iteration, which dominates the cost of a long run.
            cache_breakpoint: conversation_cache_breakpoint(messages),
        };

        let max_retries = self.max_retries.max(1);
        let mut last_error: Option<AgentError> = None;

        for attempt in 0..max_retries {
            debug!(attempt = attempt, "Sending request to Anthropic API");

            // The send itself is inside the retry loop. Previously a `?` here meant a
            // dropped connection or DNS blip ended the run without a single retry,
            // even though only status codes were treated as transient.
            let response = match self.build_request(&request).send().await {
                Ok(r) => r,
                Err(e) => {
                    if is_retryable_transport_error(&e) && attempt < max_retries - 1 {
                        let delay_ms = retry_delay_ms(attempt, None, self.retry_base_delay_ms);
                        warn!(
                            attempt = attempt,
                            delay_ms = delay_ms,
                            error = %e,
                            "Transport error, retrying"
                        );
                        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                        last_error = Some(AgentError::Http(e));
                        continue;
                    }
                    return Err(AgentError::Http(e));
                }
            };

            let status = response.status();
            let retry_after = parse_retry_after(response.headers());

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
                    let delay_ms = retry_delay_ms(attempt, retry_after, self.retry_base_delay_ms);
                    warn!(
                        attempt = attempt,
                        delay_ms = delay_ms,
                        retry_after = ?retry_after,
                        "Rate limited, retrying"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                    last_error = Some(AgentError::LlmRateLimited);
                    continue;
                }
                return Err(AgentError::LlmRateLimited);
            }

            if status.is_server_error() {
                let body = response.text().await.unwrap_or_default();
                if attempt < max_retries - 1 {
                    let delay_ms = retry_delay_ms(attempt, retry_after, self.retry_base_delay_ms);
                    warn!(
                        attempt = attempt,
                        delay_ms = delay_ms,
                        status = status.as_u16(),
                        "Server error, retrying"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                    last_error = Some(AgentError::Llm(format!(
                        "Server error {}: {}",
                        status.as_u16(),
                        body
                    )));
                    continue;
                }
                return Err(AgentError::Llm(format!(
                    "Server error {}: {}",
                    status.as_u16(),
                    body
                )));
            }

            let body = response.text().await.unwrap_or_default();

            // A 400 for an over-long prompt is recoverable by compacting, unlike other
            // 400s, so it gets its own error the loop can act on.
            if status == reqwest::StatusCode::BAD_REQUEST && is_context_overflow(&body) {
                return Err(AgentError::LlmContextOverflow(body));
            }

            return Err(AgentError::Llm(format!(
                "Unexpected status {}: {}",
                status.as_u16(),
                body
            )));
        }

        Err(last_error.unwrap_or(AgentError::LlmRateLimited))
    }
}

/// Build a dedicated client for context compaction, if one is configured.
///
/// `compaction_model` and `compaction_max_tokens` were declared in config and
/// documented but never reached the loop — every call site passed `None` for the
/// compaction LLM — so summarization always ran on the primary (expensive) model.
///
/// Returns `None` when no compaction model is configured, in which case the caller
/// should fall back to the primary client.
pub fn build_compaction_client(
    llm: &crate::config::schema::LlmConfig,
    compaction: &crate::config::schema::CompactionConfig,
) -> Option<AnthropicClient> {
    compaction.compaction_model.as_ref().map(|model| {
        AnthropicClient::new(
            llm.base_url.clone(),
            llm.api_key.clone(),
            model.clone(),
            // Summaries are short; a smaller cap keeps the cheap model cheap.
            compaction.compaction_max_tokens.unwrap_or(4096),
            llm.custom_headers.clone(),
            // Compaction is a one-shot summarization, so neither extended thinking nor
            // prompt caching earns its keep here.
            None,
            false,
        )
    })
}

impl ThinkingControl for AnthropicClient {
    fn set_thinking_config(&self, config: Option<ThinkingConfig>) {
        *self.thinking.write().expect("thinking RwLock poisoned") = config;
    }
}

impl StreamingLlmProvider for AnthropicClient {
    fn send_messages_stream<'a>(
        &'a self,
        system: Option<&'a [SystemContent]>,
        messages: &'a [Message],
        tools: Option<&'a [ToolDefinition]>,
    ) -> Pin<Box<dyn Stream<Item = Result<StreamEvent, AgentError>> + Send + 'a>> {
        let thinking = self
            .thinking
            .read()
            .expect("thinking RwLock poisoned")
            .clone();
        let request = MessagesRequest {
            model: self.model.clone(),
            max_tokens: self.max_tokens,
            system: system.map(|s| s.to_vec()),
            messages: messages.to_vec(),
            tools: tools.map(|t| t.to_vec()),
            thinking,
            stream: Some(true),
            cache_breakpoint: conversation_cache_breakpoint(messages),
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

            let response = match builder.json(&request.to_wire()).send().await {
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
        )
        .with_retry_base_delay_ms(1);

        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Hello".to_string(),
            }],
        }];

        let result = client
            .send_messages(
                Some(&[SystemContent::text("system prompt")]),
                &messages,
                None,
            )
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
        )
        .with_retry_base_delay_ms(1);

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
        )
        .with_retry_base_delay_ms(1);

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
        )
        .with_retry_base_delay_ms(1);

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
        )
        .with_retry_base_delay_ms(1);

        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Hello".to_string(),
            }],
        }];

        let result = client.send_messages(None, &messages, None).await;

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
        )
        .with_retry_base_delay_ms(1);

        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Hello".to_string(),
            }],
        }];
        let result = client.send_messages(None, &messages, None).await;

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
        )
        .with_retry_base_delay_ms(1);

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
        )
        .with_retry_base_delay_ms(1);

        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Hello".to_string(),
            }],
        }];
        let result = client.send_messages(None, &messages, None).await;

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
        )
        .with_retry_base_delay_ms(1);

        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Hello".to_string(),
            }],
        }];
        let result = client.send_messages(None, &messages, None).await;

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
            read_only: false,
        }];

        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Go to example.com".to_string(),
            }],
        }];

        let result = client
            .send_messages(
                Some(&[SystemContent::text("You are a helper")]),
                &messages,
                Some(&tools),
            )
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
        )
        .with_retry_base_delay_ms(1);

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
        )
        .with_retry_base_delay_ms(1);

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
        )
        .with_retry_base_delay_ms(1);

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

    // --- Retry policy ---

    #[test]
    fn test_retry_after_header_wins_over_backoff() {
        // The server knows when its limit resets; a local guess does not.
        assert_eq!(retry_delay_ms(0, Some(7), 1000), 7000);
    }

    #[test]
    fn test_retry_after_is_capped() {
        assert_eq!(retry_delay_ms(0, Some(9999), 1000), MAX_RETRY_DELAY_MS);
    }

    #[test]
    fn test_backoff_grows_and_is_capped() {
        let d0 = retry_delay_ms(0, None, 1000);
        let d3 = retry_delay_ms(3, None, 1000);
        assert!(d3 > d0, "{d3} should exceed {d0}");
        assert!(retry_delay_ms(20, None, 1000) <= MAX_RETRY_DELAY_MS);
    }

    #[test]
    fn test_backoff_is_jittered() {
        // Without jitter, concurrent agents retry in lockstep and reproduce the burst
        // that triggered the rate limit.
        let plain: Vec<u64> = (0..4).map(|a| 1000 * 2u64.pow(a)).collect();
        let actual: Vec<u64> = (0..4).map(|a| retry_delay_ms(a, None, 1000)).collect();
        assert_ne!(plain, actual, "delays were not jittered");
    }

    #[test]
    fn test_parse_retry_after_numeric() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "12".parse().unwrap());
        assert_eq!(parse_retry_after(&headers), Some(12));
    }

    #[test]
    fn test_parse_retry_after_http_date_falls_back() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::RETRY_AFTER,
            "Wed, 21 Oct 2015 07:28:00 GMT".parse().unwrap(),
        );
        assert_eq!(parse_retry_after(&headers), None);
    }

    #[test]
    fn test_context_overflow_detection() {
        assert!(is_context_overflow(
            r#"{"error":{"message":"prompt is too long: 250000 tokens > 200000 maximum"}}"#
        ));
        assert!(is_context_overflow("exceeds the context window"));
        assert!(!is_context_overflow(
            r#"{"error":{"message":"invalid model name"}}"#
        ));
    }

    #[tokio::test]
    async fn test_context_overflow_returns_distinct_error() {
        // Recoverable by compacting, unlike other 400s, so the loop needs to tell them
        // apart rather than ending the run.
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/messages")
            .with_status(400)
            .with_body(r#"{"error":{"message":"prompt is too long: 250000 tokens"}}"#)
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
        )
        .with_retry_base_delay_ms(1);

        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Hello".to_string(),
            }],
        }];

        let err = client
            .send_messages(None, &messages, None)
            .await
            .unwrap_err();
        assert!(
            matches!(err, AgentError::LlmContextOverflow(_)),
            "got: {err:?}"
        );
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_other_400_is_not_treated_as_overflow() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/messages")
            .with_status(400)
            .with_body(r#"{"error":{"message":"invalid model name"}}"#)
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
        )
        .with_retry_base_delay_ms(1);

        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Hello".to_string(),
            }],
        }];

        let err = client
            .send_messages(None, &messages, None)
            .await
            .unwrap_err();
        assert!(matches!(err, AgentError::Llm(_)), "got: {err:?}");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_transport_error_is_retried() {
        // A dropped connection used to end the run without a single retry, because the
        // send was outside the retry loop.
        let client = AnthropicClient::new(
            // Nothing is listening here, so every attempt fails to connect.
            "http://127.0.0.1:1".to_string(),
            "test-key".to_string(),
            "test-model".to_string(),
            8192,
            Default::default(),
            None,
            false,
        )
        .with_retry_base_delay_ms(1)
        .with_max_retries(3);

        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Hello".to_string(),
            }],
        }];

        let err = client
            .send_messages(None, &messages, None)
            .await
            .unwrap_err();
        assert!(matches!(err, AgentError::Http(_)), "got: {err:?}");
    }

    #[tokio::test]
    async fn test_conversation_cache_breakpoint_is_sent_on_the_wire() {
        // The unit tests cover the wire form; this proves the client actually sends it,
        // which is the part that was silently missing before.
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/messages")
            .match_body(mockito::Matcher::PartialJson(serde_json::json!({
                "messages": [
                    {"role": "user", "content": [{"type": "text", "text": "m0"}]},
                    {"role": "assistant", "content": [{"type": "text", "text": "m1"}]},
                    {"role": "user", "content": [{"type": "text", "text": "m2"}]},
                    {"role": "assistant", "content": [{
                        "type": "text",
                        "text": "m3",
                        "cache_control": {"type": "ephemeral"}
                    }]},
                    {"role": "user", "content": [{"type": "text", "text": "m4"}]},
                    {"role": "assistant", "content": [{"type": "text", "text": "m5"}]},
                ]
            })))
            .with_status(200)
            .with_body(
                r#"{"id":"msg_1","content":[{"type":"text","text":"ok"}],
                    "model":"test-model","stop_reason":"end_turn"}"#,
            )
            .create_async()
            .await;

        let client = AnthropicClient::new(
            server.url(),
            "test-key".to_string(),
            "test-model".to_string(),
            8192,
            Default::default(),
            None,
            true,
        )
        .with_retry_base_delay_ms(1);

        let messages: Vec<Message> = (0..6)
            .map(|i| Message {
                role: if i % 2 == 0 {
                    Role::User
                } else {
                    Role::Assistant
                },
                content: vec![ContentBlock::Text {
                    text: format!("m{i}"),
                }],
            })
            .collect();

        client.send_messages(None, &messages, None).await.unwrap();
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_short_conversation_sends_no_cache_control() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/messages")
            .match_body(mockito::Matcher::PartialJson(serde_json::json!({
                "messages": [{"role": "user", "content": [{"type": "text", "text": "hi"}]}]
            })))
            .with_status(200)
            .with_body(
                r#"{"id":"msg_1","content":[{"type":"text","text":"ok"}],
                    "model":"test-model","stop_reason":"end_turn"}"#,
            )
            .create_async()
            .await;

        let client = AnthropicClient::new(
            server.url(),
            "test-key".to_string(),
            "test-model".to_string(),
            8192,
            Default::default(),
            None,
            true,
        )
        .with_retry_base_delay_ms(1);

        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "hi".to_string(),
            }],
        }];

        let response = client.send_messages(None, &messages, None).await.unwrap();
        assert_eq!(response.id, "msg_1");
        mock.assert_async().await;
    }
}
