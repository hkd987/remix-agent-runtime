use std::collections::HashMap;
use std::time::Duration;

use serde_json::Value;

use crate::browser::mcp::ToolExecutionResult;
use crate::error::AgentError;

/// Maximum raw response body size before HTML-to-markdown conversion (5MB).
/// This prevents wasting CPU on converting enormous pages that will be truncated anyway.
const MAX_RAW_BODY_BYTES: usize = 5 * 1024 * 1024;

/// Fetch a webpage via HTTP GET and return its content as markdown (for HTML)
/// or raw text (for other content types).
///
/// Auto-upgrades `http://` URLs to `https://`.
pub async fn execute_web_fetch(
    args: Value,
    timeout_secs: u64,
    max_bytes: usize,
) -> Result<ToolExecutionResult, AgentError> {
    let url_str = args
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AgentError::LocalTool("web_fetch requires 'url' parameter".to_string()))?;

    let custom_headers: HashMap<String, String> = args
        .get("headers")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    let url = validate_and_normalize_url(url_str)?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .redirect(reqwest::redirect::Policy::limited(10))
        .user_agent("remix-agent-runtime")
        .build()
        .map_err(|e| AgentError::LocalTool(format!("Failed to build HTTP client: {e}")))?;

    let mut request = client.get(&url);
    for (key, value) in &custom_headers {
        request = request.header(key.as_str(), value.as_str());
    }

    let response = request.send().await.map_err(|e| {
        if e.is_timeout() {
            AgentError::LocalTool(format!("Request timed out after {timeout_secs}s: {url}"))
        } else if e.is_connect() {
            AgentError::LocalTool(format!("Connection failed: {e}"))
        } else {
            AgentError::LocalTool(format!("HTTP request failed: {e}"))
        }
    })?;

    let status = response.status();
    if !status.is_success() {
        return Ok(ToolExecutionResult {
            content: format!(
                "HTTP {status} for {url}\n\nThe server returned a non-success status code."
            ),
            is_error: false,
        });
    }

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();

    let is_html = content_type.contains("text/html") || content_type.contains("application/xhtml");

    let body_bytes = response.bytes().await.map_err(|e| {
        AgentError::LocalTool(format!("Failed to read response body: {e}"))
    })?;

    // Cap raw body size before processing
    let raw_body = if body_bytes.len() > MAX_RAW_BODY_BYTES {
        &body_bytes[..MAX_RAW_BODY_BYTES]
    } else {
        &body_bytes[..]
    };

    let body_text = String::from_utf8_lossy(raw_body).into_owned();

    let content = if is_html {
        htmd::convert(&body_text).unwrap_or(body_text)
    } else {
        body_text
    };

    let (truncated_content, was_truncated) = truncate_content(&content, max_bytes);

    let output = if was_truncated {
        format!(
            "{truncated_content}\n\n... [content truncated at {max_bytes} bytes]"
        )
    } else {
        truncated_content
    };

    Ok(ToolExecutionResult {
        content: output,
        is_error: false,
    })
}

/// Validate and normalize a URL string.
///
/// - Rejects non-http/https schemes
/// - Auto-upgrades http:// to https://
fn validate_and_normalize_url(url_str: &str) -> Result<String, AgentError> {
    // Quick check for scheme before full parsing
    let trimmed = url_str.trim();
    if trimmed.is_empty() {
        return Err(AgentError::LocalTool("URL cannot be empty".to_string()));
    }

    // Attempt to parse — if no scheme, prepend https://
    let with_scheme = if !trimmed.contains("://") {
        format!("https://{trimmed}")
    } else {
        trimmed.to_string()
    };

    let parsed = reqwest::Url::parse(&with_scheme)
        .map_err(|e| AgentError::LocalTool(format!("Invalid URL '{url_str}': {e}")))?;

    match parsed.scheme() {
        "https" => Ok(parsed.to_string()),
        "http" => {
            // Auto-upgrade to HTTPS
            let mut upgraded = parsed;
            upgraded
                .set_scheme("https")
                .map_err(|_| AgentError::LocalTool("Failed to upgrade URL to HTTPS".to_string()))?;
            Ok(upgraded.to_string())
        }
        scheme => Err(AgentError::LocalTool(format!(
            "Unsupported URL scheme '{scheme}'. Only http and https are allowed."
        ))),
    }
}

/// Truncate content to the given byte limit on a char boundary.
/// Returns (truncated_string, was_truncated).
fn truncate_content(content: &str, max_bytes: usize) -> (String, bool) {
    if content.len() <= max_bytes {
        return (content.to_string(), false);
    }

    // Find a valid char boundary at or before max_bytes
    let mut boundary = max_bytes;
    while boundary > 0 && !content.is_char_boundary(boundary) {
        boundary -= 1;
    }

    (content[..boundary].to_string(), true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_web_fetch_html() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/page")
            .with_status(200)
            .with_header("content-type", "text/html")
            .with_body("<html><body><h1>Hello</h1><p>World paragraph</p></body></html>")
            .create_async()
            .await;

        // mockito uses http, but our tool upgrades to https, so we need to
        // test with the raw URL. We'll test the upgrade separately.
        // For the mock server, directly pass the http URL and test the core logic.
        let result = fetch_with_url(
            &format!("{}/page", server.url()),
            HashMap::new(),
            30,
            102_400,
        )
        .await
        .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("Hello"));
        assert!(result.content.contains("World paragraph"));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_web_fetch_plain_text() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/text")
            .with_status(200)
            .with_header("content-type", "text/plain")
            .with_body("Just plain text content")
            .create_async()
            .await;

        let result = fetch_with_url(
            &format!("{}/text", server.url()),
            HashMap::new(),
            30,
            102_400,
        )
        .await
        .unwrap();

        assert!(!result.is_error);
        assert_eq!(result.content, "Just plain text content");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_web_fetch_json() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/api")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"key": "value", "num": 42}"#)
            .create_async()
            .await;

        let result = fetch_with_url(
            &format!("{}/api", server.url()),
            HashMap::new(),
            30,
            102_400,
        )
        .await
        .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains(r#""key": "value""#));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_web_fetch_non_200() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/missing")
            .with_status(404)
            .create_async()
            .await;

        let result = fetch_with_url(
            &format!("{}/missing", server.url()),
            HashMap::new(),
            30,
            102_400,
        )
        .await
        .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("404"));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_web_fetch_missing_url() {
        let result = execute_web_fetch(json!({}), 30, 102_400).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("requires 'url' parameter"));
    }

    #[tokio::test]
    async fn test_web_fetch_invalid_url() {
        let result = execute_web_fetch(json!({"url": "://not-valid"}), 30, 102_400).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Invalid URL"));
    }

    #[tokio::test]
    async fn test_web_fetch_non_http_scheme() {
        let result = execute_web_fetch(json!({"url": "ftp://example.com/file"}), 30, 102_400).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unsupported URL scheme"));
    }

    #[tokio::test]
    async fn test_web_fetch_truncation() {
        let mut server = mockito::Server::new_async().await;
        let large_body = "x".repeat(1000);
        let mock = server
            .mock("GET", "/large")
            .with_status(200)
            .with_header("content-type", "text/plain")
            .with_body(&large_body)
            .create_async()
            .await;

        let result = fetch_with_url(
            &format!("{}/large", server.url()),
            HashMap::new(),
            30,
            100, // very small limit
        )
        .await
        .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("content truncated at 100 bytes"));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_web_fetch_custom_headers() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/auth")
            .match_header("Authorization", "Bearer test-token")
            .with_status(200)
            .with_header("content-type", "text/plain")
            .with_body("authorized")
            .create_async()
            .await;

        let mut headers = HashMap::new();
        headers.insert(
            "Authorization".to_string(),
            "Bearer test-token".to_string(),
        );

        let result = fetch_with_url(
            &format!("{}/auth", server.url()),
            headers,
            30,
            102_400,
        )
        .await
        .unwrap();

        assert!(!result.is_error);
        assert_eq!(result.content, "authorized");
        mock.assert_async().await;
    }

    #[test]
    fn test_validate_url_https() {
        let result = validate_and_normalize_url("https://example.com");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "https://example.com/");
    }

    #[test]
    fn test_validate_url_http_upgrades_to_https() {
        let result = validate_and_normalize_url("http://example.com/page");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "https://example.com/page");
    }

    #[test]
    fn test_validate_url_no_scheme_adds_https() {
        let result = validate_and_normalize_url("example.com/page");
        assert!(result.is_ok());
        assert!(result.unwrap().starts_with("https://"));
    }

    #[test]
    fn test_validate_url_ftp_rejected() {
        let result = validate_and_normalize_url("ftp://files.example.com");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unsupported URL scheme"));
    }

    #[test]
    fn test_validate_url_empty_rejected() {
        let result = validate_and_normalize_url("");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("cannot be empty"));
    }

    #[test]
    fn test_truncate_content_no_truncation() {
        let (result, truncated) = truncate_content("hello", 100);
        assert_eq!(result, "hello");
        assert!(!truncated);
    }

    #[test]
    fn test_truncate_content_exact_boundary() {
        let (result, truncated) = truncate_content("hello", 5);
        assert_eq!(result, "hello");
        assert!(!truncated);
    }

    #[test]
    fn test_truncate_content_truncated() {
        let (result, truncated) = truncate_content("hello world", 5);
        assert_eq!(result, "hello");
        assert!(truncated);
    }

    #[test]
    fn test_truncate_content_multibyte_char_boundary() {
        // "é" is 2 bytes in UTF-8
        let content = "café";
        // "caf" = 3 bytes, "é" = 2 bytes, total = 5
        let (result, truncated) = truncate_content(content, 4);
        // Should truncate to "caf" (3 bytes) since byte 4 is in the middle of "é"
        assert_eq!(result, "caf");
        assert!(truncated);
    }

    /// Helper that bypasses URL validation (for testing with mockito's http:// URLs).
    async fn fetch_with_url(
        url: &str,
        custom_headers: HashMap<String, String>,
        timeout_secs: u64,
        max_bytes: usize,
    ) -> Result<ToolExecutionResult, AgentError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .redirect(reqwest::redirect::Policy::limited(10))
            .user_agent("remix-agent-runtime")
            .build()
            .map_err(|e| AgentError::LocalTool(format!("Failed to build HTTP client: {e}")))?;

        let mut request = client.get(url);
        for (key, value) in &custom_headers {
            request = request.header(key.as_str(), value.as_str());
        }

        let response = request.send().await.map_err(|e| {
            AgentError::LocalTool(format!("HTTP request failed: {e}"))
        })?;

        let status = response.status();
        if !status.is_success() {
            return Ok(ToolExecutionResult {
                content: format!(
                    "HTTP {status} for {url}\n\nThe server returned a non-success status code."
                ),
                is_error: false,
            });
        }

        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_lowercase();

        let is_html =
            content_type.contains("text/html") || content_type.contains("application/xhtml");

        let body_bytes = response.bytes().await.map_err(|e| {
            AgentError::LocalTool(format!("Failed to read response body: {e}"))
        })?;

        let raw_body = if body_bytes.len() > MAX_RAW_BODY_BYTES {
            &body_bytes[..MAX_RAW_BODY_BYTES]
        } else {
            &body_bytes[..]
        };

        let body_text = String::from_utf8_lossy(raw_body).into_owned();

        let content = if is_html {
            htmd::convert(&body_text).unwrap_or(body_text)
        } else {
            body_text
        };

        let (truncated_content, was_truncated) = truncate_content(&content, max_bytes);

        let output = if was_truncated {
            format!("{truncated_content}\n\n... [content truncated at {max_bytes} bytes]")
        } else {
            truncated_content
        };

        Ok(ToolExecutionResult {
            content: output,
            is_error: false,
        })
    }
}
