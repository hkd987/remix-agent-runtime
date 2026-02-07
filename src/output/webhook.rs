use super::result::AgentResult;
use crate::error::AgentError;

/// Dispatches webhooks for agent completion and error events.
pub struct WebhookDispatcher {
    client: reqwest::Client,
    on_complete_url: Option<String>,
    on_error_url: Option<String>,
}

impl WebhookDispatcher {
    /// Create a new dispatcher with optional webhook URLs.
    pub fn new(on_complete_url: Option<String>, on_error_url: Option<String>) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("Failed to create HTTP client"),
            on_complete_url,
            on_error_url,
        }
    }

    /// Send completion webhook. Logs errors but does not propagate them
    /// (fire-and-forget semantics).
    pub async fn send_completion(&self, result: &AgentResult) {
        if let Some(ref url) = self.on_complete_url {
            if let Err(e) = self.send_webhook(url, result).await {
                tracing::error!("Failed to send completion webhook: {e}");
            }
        }
    }

    /// Send error webhook. Logs errors but does not propagate them.
    pub async fn send_error(&self, _error: &str, result: &AgentResult) {
        if let Some(ref url) = self.on_error_url {
            if let Err(e) = self.send_webhook(url, result).await {
                tracing::error!("Failed to send error webhook: {e}");
            }
        }
    }

    async fn send_webhook(&self, url: &str, body: &AgentResult) -> Result<(), AgentError> {
        self.client
            .post(url)
            .header("content-type", "application/json")
            .json(body)
            .send()
            .await
            .map_err(|e| AgentError::Webhook(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::result::{AgentResult, AgentStatus, StepRecord};
    use serde_json::json;

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

        let dispatcher = WebhookDispatcher::new(Some(format!("{}/webhook", server.url())), None);
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

        let dispatcher = WebhookDispatcher::new(None, Some(format!("{}/error-hook", server.url())));
        dispatcher.send_error("something failed", &result).await;

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_send_error_does_nothing_when_no_webhook() {
        let dispatcher = WebhookDispatcher::new(None, None);
        let result = error_result();
        dispatcher.send_error("oops", &result).await;
    }

    #[tokio::test]
    async fn test_webhook_failure_is_not_propagated() {
        let mut server = mockito::Server::new_async().await;

        let mock = server
            .mock("POST", "/webhook")
            .with_status(500)
            .create_async()
            .await;

        let dispatcher = WebhookDispatcher::new(Some(format!("{}/webhook", server.url())), None);
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

        let dispatcher = WebhookDispatcher::new(Some(format!("{}/webhook", server.url())), None);
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

        let dispatcher = WebhookDispatcher::new(
            Some(format!("{}/complete", server.url())),
            Some(format!("{}/error", server.url())),
        );

        let result = sample_result();
        dispatcher.send_completion(&result).await;
        complete_mock.assert_async().await;

        let err_result = error_result();
        dispatcher.send_error("fail", &err_result).await;
        error_mock.assert_async().await;
    }
}
