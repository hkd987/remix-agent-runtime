use std::collections::HashMap;
use std::time::Duration;

use serde_json::Value;
use tokio_stream::StreamExt;

use crate::browser::mcp::ToolExecutionResult;
use crate::error::AgentError;

use super::output_filter::floor_char_boundary;

/// Maximum raw response body size (5MB). Streaming stops once this limit is reached,
/// preventing unbounded memory usage on large responses.
const MAX_RAW_BODY_BYTES: usize = 5 * 1024 * 1024;

const USER_AGENT: &str = "remix-agent-runtime";
const MAX_REDIRECTS: usize = 10;

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

    let custom_headers = parse_headers(&args)?;
    let url = validate_and_normalize_url(url_str)?;

    fetch_url(&url, &custom_headers, timeout_secs, max_bytes).await
}

/// Core fetch logic, separated from argument parsing for testability.
async fn fetch_url(
    url: &str,
    headers: &HashMap<String, String>,
    timeout_secs: u64,
    max_bytes: usize,
) -> Result<ToolExecutionResult, AgentError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| AgentError::LocalTool(format!("Failed to build HTTP client: {e}")))?;

    let mut request = client.get(url);
    for (key, value) in headers {
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

    // Stream body with a size cap to avoid unbounded memory usage
    let body_bytes = read_body_capped(response, MAX_RAW_BODY_BYTES).await?;

    let body_text = String::from_utf8_lossy(&body_bytes);

    let content = if is_html {
        htmd::convert(&body_text).unwrap_or_else(|_| body_text.into_owned())
    } else {
        body_text.into_owned()
    };

    if content.is_empty() {
        return Ok(ToolExecutionResult {
            content: format!("(empty response body from {url})"),
            is_error: false,
        });
    }

    let output = truncate_content(content, max_bytes);

    Ok(ToolExecutionResult {
        content: output,
        is_error: false,
    })
}

/// Parse and validate the `headers` parameter from tool arguments.
fn parse_headers(args: &Value) -> Result<HashMap<String, String>, AgentError> {
    match args.get("headers") {
        None => Ok(HashMap::new()),
        Some(v) => serde_json::from_value::<HashMap<String, String>>(v.clone()).map_err(|e| {
            AgentError::LocalTool(format!(
                "Invalid 'headers' parameter: expected an object with string values. {e}"
            ))
        }),
    }
}

/// Stream the response body up to `max_bytes`, stopping early once the cap is reached.
async fn read_body_capped(
    response: reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, AgentError> {
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| {
            AgentError::LocalTool(format!("Failed to read response body: {e}"))
        })?;
        body.extend_from_slice(&chunk);
        if body.len() >= max_bytes {
            body.truncate(max_bytes);
            break;
        }
    }

    Ok(body)
}

/// Validate and normalize a URL string.
///
/// - Rejects non-http/https schemes
/// - Auto-upgrades http:// to https://
fn validate_and_normalize_url(url_str: &str) -> Result<String, AgentError> {
    let trimmed = url_str.trim();
    if trimmed.is_empty() {
        return Err(AgentError::LocalTool("URL cannot be empty".to_string()));
    }

    // If no scheme, prepend https://
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
/// Appends a truncation notice if content was trimmed.
fn truncate_content(content: String, max_bytes: usize) -> String {
    if content.len() <= max_bytes {
        return content;
    }

    let boundary = floor_char_boundary(&content, max_bytes);
    let mut result = content;
    result.truncate(boundary);
    result.push_str(&format!(
        "\n\n... [content truncated at {max_bytes} bytes]"
    ));
    result
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

        let result = fetch_url(
            &format!("{}/page", server.url()),
            &HashMap::new(),
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

        let result = fetch_url(
            &format!("{}/text", server.url()),
            &HashMap::new(),
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

        let result = fetch_url(
            &format!("{}/api", server.url()),
            &HashMap::new(),
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

        let result = fetch_url(
            &format!("{}/missing", server.url()),
            &HashMap::new(),
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
    async fn test_web_fetch_empty_body() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/empty")
            .with_status(200)
            .with_header("content-type", "text/plain")
            .with_body("")
            .create_async()
            .await;

        let result = fetch_url(
            &format!("{}/empty", server.url()),
            &HashMap::new(),
            30,
            102_400,
        )
        .await
        .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("empty response body"));
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
        let result =
            execute_web_fetch(json!({"url": "ftp://example.com/file"}), 30, 102_400).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unsupported URL scheme"));
    }

    #[tokio::test]
    async fn test_web_fetch_malformed_headers() {
        let result =
            execute_web_fetch(json!({"url": "https://example.com", "headers": "bad"}), 30, 102_400)
                .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Invalid 'headers' parameter"));
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

        let result = fetch_url(
            &format!("{}/large", server.url()),
            &HashMap::new(),
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

        let result = fetch_url(
            &format!("{}/auth", server.url()),
            &headers,
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
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Unsupported URL scheme"));
    }

    #[test]
    fn test_validate_url_empty_rejected() {
        let result = validate_and_normalize_url("");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("cannot be empty"));
    }

    #[test]
    fn test_truncate_content_no_truncation() {
        let result = truncate_content("hello".to_string(), 100);
        assert_eq!(result, "hello");
    }

    #[test]
    fn test_truncate_content_exact_boundary() {
        let result = truncate_content("hello".to_string(), 5);
        assert_eq!(result, "hello");
    }

    #[test]
    fn test_truncate_content_truncated() {
        let result = truncate_content("hello world".to_string(), 5);
        assert!(result.starts_with("hello"));
        assert!(result.contains("content truncated"));
    }

    #[test]
    fn test_truncate_content_multibyte_char_boundary() {
        // "é" is 2 bytes in UTF-8
        let content = "café".to_string();
        // "caf" = 3 bytes, "é" = 2 bytes, total = 5
        let result = truncate_content(content, 4);
        // Should truncate to "caf" (3 bytes) since byte 4 is in the middle of "é"
        assert!(result.starts_with("caf"));
        assert!(result.contains("content truncated"));
    }

    #[test]
    fn test_parse_headers_missing() {
        let result = parse_headers(&json!({"url": "https://example.com"})).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_headers_valid() {
        let result = parse_headers(&json!({"headers": {"Accept": "text/html"}})).unwrap();
        assert_eq!(result.get("Accept").unwrap(), "text/html");
    }

    #[test]
    fn test_parse_headers_malformed() {
        let result = parse_headers(&json!({"headers": "not-an-object"}));
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid 'headers' parameter"));
    }
}
