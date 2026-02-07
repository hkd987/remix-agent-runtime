use super::result::AgentResult;
use crate::config::schema::WebhookConfig;
use crate::error::AgentError;

/// Stored webhook endpoint with URL and format.
struct WebhookEndpoint {
    url: String,
    format: String,
}

impl From<&WebhookConfig> for WebhookEndpoint {
    fn from(config: &WebhookConfig) -> Self {
        Self {
            url: config.url.clone(),
            format: config.format.clone(),
        }
    }
}

/// Dispatches webhooks for agent completion and error events.
pub struct WebhookDispatcher {
    client: reqwest::Client,
    on_complete: Option<WebhookEndpoint>,
    on_error: Option<WebhookEndpoint>,
}

impl WebhookDispatcher {
    /// Create a new dispatcher with optional webhook configs.
    pub fn new(on_complete: Option<&WebhookConfig>, on_error: Option<&WebhookConfig>) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("Failed to create HTTP client"),
            on_complete: on_complete.map(WebhookEndpoint::from),
            on_error: on_error.map(WebhookEndpoint::from),
        }
    }

    /// Send completion webhook. Logs errors but does not propagate them
    /// (fire-and-forget semantics).
    pub async fn send_completion(&self, result: &AgentResult) {
        if let Some(ref endpoint) = self.on_complete {
            if let Err(e) = self.send_webhook(endpoint, result).await {
                tracing::error!("Failed to send completion webhook: {e}");
            }
        }
    }

    /// Send error webhook. Logs errors but does not propagate them.
    pub async fn send_error(&self, result: &AgentResult) {
        if let Some(ref endpoint) = self.on_error {
            if let Err(e) = self.send_webhook(endpoint, result).await {
                tracing::error!("Failed to send error webhook: {e}");
            }
        }
    }

    async fn send_webhook(
        &self,
        endpoint: &WebhookEndpoint,
        body: &AgentResult,
    ) -> Result<(), AgentError> {
        let json_body: serde_json::Value = if endpoint.format == "slack" {
            let status = serde_json::to_value(&body.status)
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_else(|| "unknown".to_string());
            let detail = body.result.as_deref().unwrap_or("no result");
            serde_json::json!({ "text": format!("Agent {status}: {detail}") })
        } else {
            serde_json::to_value(body).map_err(|e| AgentError::Webhook(e.to_string()))?
        };

        let response = self
            .client
            .post(&endpoint.url)
            .header("content-type", "application/json")
            .json(&json_body)
            .send()
            .await
            .map_err(|e| AgentError::Webhook(e.to_string()))?;

        if !response.status().is_success() {
            tracing::warn!(
                status = %response.status(),
                url = %endpoint.url,
                "Webhook returned non-success status"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::result::{AgentResult, AgentStatus, StepRecord};
    use serde_json::json;

    fn json_config(url: String) -> WebhookConfig {
        WebhookConfig {
            url,
            format: "json".to_string(),
        }
    }

    fn slack_config(url: String) -> WebhookConfig {
        WebhookConfig {
            url,
            format: "slack".to_string(),
        }
    }

    fn sample_result() -> AgentResult {
        AgentResult {
            status: AgentStatus::Success,
            result: Some("done".to_string()),
            steps: vec![StepRecord {
                iteration: 1,
                tool: "navigate".to_string(),
                input: json!({ "url": "https://example.com" }),
                output: json!({ "success": true }),
                duration_ms: 1200,
                is_error: None,
            }],
            total_iterations: 1,
            total_duration_ms: 1200,
            error: None,
            total_input_tokens: None,
            total_output_tokens: None,
        }
    }

    fn error_result() -> AgentResult {
        AgentResult::error("something failed".to_string(), vec![], 500)
    }

    #[tokio::test]
    async fn test_send_completion_posts_correct_json_body() {
        let mut server = mockito::Server::new_async().await;
        let result = sample_result();
        let expected_body = serde_json::to_string(&result).unwrap();

        let mock = server
            .mock("POST", "/webhook")
            .with_status(200)
            .match_header("content-type", "application/json")
            .match_body(mockito::Matcher::Json(
                serde_json::from_str(&expected_body).unwrap(),
            ))
            .create_async()
            .await;

        let cfg = json_config(format!("{}/webhook", server.url()));
        let dispatcher = WebhookDispatcher::new(Some(&cfg), None);
        dispatcher.send_completion(&result).await;

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_send_completion_does_nothing_when_no_webhook() {
        let dispatcher = WebhookDispatcher::new(None, None);
        let result = sample_result();
        // Should not panic or do anything
        dispatcher.send_completion(&result).await;
    }

    #[tokio::test]
    async fn test_send_error_posts_correct_body() {
        let mut server = mockito::Server::new_async().await;
        let result = error_result();
        let expected_body = serde_json::to_string(&result).unwrap();

        let mock = server
            .mock("POST", "/error-hook")
            .with_status(200)
            .match_header("content-type", "application/json")
            .match_body(mockito::Matcher::Json(
                serde_json::from_str(&expected_body).unwrap(),
            ))
            .create_async()
            .await;

        let cfg = json_config(format!("{}/error-hook", server.url()));
        let dispatcher = WebhookDispatcher::new(None, Some(&cfg));
        dispatcher.send_error(&result).await;

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_send_error_does_nothing_when_no_webhook() {
        let dispatcher = WebhookDispatcher::new(None, None);
        let result = error_result();
        dispatcher.send_error(&result).await;
    }

    #[tokio::test]
    async fn test_webhook_failure_is_not_propagated() {
        let mut server = mockito::Server::new_async().await;

        let mock = server
            .mock("POST", "/webhook")
            .with_status(500)
            .create_async()
            .await;

        let cfg = json_config(format!("{}/webhook", server.url()));
        let dispatcher = WebhookDispatcher::new(Some(&cfg), None);
        let result = sample_result();

        // Should not panic or return an error -- fire-and-forget
        dispatcher.send_completion(&result).await;

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_correct_content_type_header() {
        let mut server = mockito::Server::new_async().await;

        let mock = server
            .mock("POST", "/webhook")
            .with_status(200)
            .match_header("content-type", "application/json")
            .create_async()
            .await;

        let cfg = json_config(format!("{}/webhook", server.url()));
        let dispatcher = WebhookDispatcher::new(Some(&cfg), None);
        dispatcher.send_completion(&sample_result()).await;

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_both_webhooks_configured() {
        let mut server = mockito::Server::new_async().await;

        let complete_mock = server
            .mock("POST", "/complete")
            .with_status(200)
            .create_async()
            .await;

        let error_mock = server
            .mock("POST", "/error")
            .with_status(200)
            .create_async()
            .await;

        let complete_cfg = json_config(format!("{}/complete", server.url()));
        let error_cfg = json_config(format!("{}/error", server.url()));
        let dispatcher = WebhookDispatcher::new(Some(&complete_cfg), Some(&error_cfg));

        let result = sample_result();
        dispatcher.send_completion(&result).await;
        complete_mock.assert_async().await;

        let err_result = error_result();
        dispatcher.send_error(&err_result).await;
        error_mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_webhook_logs_warning_on_4xx() {
        let mut server = mockito::Server::new_async().await;

        let mock = server
            .mock("POST", "/webhook")
            .with_status(400)
            .create_async()
            .await;

        let cfg = json_config(format!("{}/webhook", server.url()));
        let dispatcher = WebhookDispatcher::new(Some(&cfg), None);
        let result = sample_result();

        // Should not panic -- fire-and-forget semantics preserved even on 4xx
        dispatcher.send_completion(&result).await;

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_slack_format_sends_text_wrapper() {
        let mut server = mockito::Server::new_async().await;

        let mock = server
            .mock("POST", "/webhook")
            .with_status(200)
            .match_header("content-type", "application/json")
            .match_body(mockito::Matcher::Json(
                json!({ "text": "Agent success: done" }),
            ))
            .create_async()
            .await;

        let cfg = slack_config(format!("{}/webhook", server.url()));
        let dispatcher = WebhookDispatcher::new(Some(&cfg), None);
        dispatcher.send_completion(&sample_result()).await;

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_json_format_sends_raw_body() {
        let mut server = mockito::Server::new_async().await;
        let result = sample_result();
        let expected_body = serde_json::to_string(&result).unwrap();

        let mock = server
            .mock("POST", "/webhook")
            .with_status(200)
            .match_body(mockito::Matcher::Json(
                serde_json::from_str(&expected_body).unwrap(),
            ))
            .create_async()
            .await;

        let cfg = json_config(format!("{}/webhook", server.url()));
        let dispatcher = WebhookDispatcher::new(Some(&cfg), None);
        dispatcher.send_completion(&result).await;

        mock.assert_async().await;
    }
}
