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
    let args: super::params::WebFetchArgs = super::params::parse("web_fetch", args)?;
    let custom_headers = args.headers.unwrap_or_default();
    let url = validate_and_normalize_url(&args.url)?;

    fetch_url(&url, &custom_headers, timeout_secs, max_bytes).await
}

/// Core fetch logic, separated from argument parsing for testability.
async fn fetch_url(
    url: &str,
    headers: &HashMap<String, String>,
    timeout_secs: u64,
    max_bytes: usize,
) -> Result<ToolExecutionResult, AgentError> {
    // Re-check every redirect hop. Validating only the initial URL is not enough: a
    // public URL can 302 straight to 169.254.169.254 and the check would never see it.
    let redirect_policy = reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= MAX_REDIRECTS {
            return attempt.error("too many redirects");
        }
        match reject_internal_host(attempt.url()) {
            Ok(()) => attempt.follow(),
            Err(e) => attempt.error(e.to_string()),
        }
    });

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .redirect(redirect_policy)
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| AgentError::LocalTool(format!("Failed to build HTTP client: {e}")))?;

    let mut request = client.get(url);
    for (key, value) in headers {
        if is_forbidden_header(key) {
            return Err(AgentError::LocalTool(format!(
                "Header '{key}' may not be set by web_fetch."
            )));
        }
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

/// Stream the response body up to `max_bytes`, stopping early once the cap is reached.
async fn read_body_capped(
    response: reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, AgentError> {
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk
            .map_err(|e| AgentError::LocalTool(format!("Failed to read response body: {e}")))?;
        body.extend_from_slice(&chunk);
        if body.len() >= max_bytes {
            body.truncate(max_bytes);
            break;
        }
    }

    Ok(body)
}

/// Headers the model must not be able to set.
///
/// Credential headers would let the agent forge authenticated requests, and the
/// metadata services key off a specific header to prove the caller is local.
const FORBIDDEN_HEADERS: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "cookie",
    "metadata-flavor",
    "x-aws-ec2-metadata-token",
    "x-metadata-token",
    "x-forwarded-for",
    "x-real-ip",
    "host",
];

fn is_forbidden_header(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    FORBIDDEN_HEADERS.contains(&lower.as_str())
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

    let normalized = match parsed.scheme() {
        "https" => parsed,
        "http" => {
            // Auto-upgrade to HTTPS
            let mut upgraded = parsed;
            upgraded
                .set_scheme("https")
                .map_err(|_| AgentError::LocalTool("Failed to upgrade URL to HTTPS".to_string()))?;
            upgraded
        }
        scheme => {
            return Err(AgentError::LocalTool(format!(
                "Unsupported URL scheme '{scheme}'. Only http and https are allowed."
            )))
        }
    };

    reject_internal_host(&normalized)?;
    Ok(normalized.to_string())
}

/// Reject URLs pointing at loopback, link-local, or private-network hosts.
///
/// The model chooses this URL and supplies the headers, so without this check
/// `web_fetch` is a general-purpose request forger aimed at whatever the agent host can
/// reach — most sharply the cloud instance metadata service at 169.254.169.254, which
/// hands out credentials to anything that asks.
fn reject_internal_host(url: &reqwest::Url) -> Result<(), AgentError> {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    let Some(host) = url.host_str() else {
        return Err(AgentError::LocalTool("URL has no host".to_string()));
    };

    let blocked = |what: &str| {
        Err(AgentError::LocalTool(format!(
            "Refusing to fetch '{host}': {what} addresses are not reachable via web_fetch."
        )))
    };

    // `host_str` keeps the square brackets on an IPv6 literal, which would never parse
    // as an address, so strip them before trying.
    let bare = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);

    if let Ok(ip) = bare.parse::<IpAddr>() {
        return match ip {
            IpAddr::V4(v4) => check_v4(v4, &blocked),
            IpAddr::V6(v6) => check_v6(v6, &blocked),
        };
    }

    // Hostname forms that resolve internally by convention.
    let lower = host.to_ascii_lowercase();
    if lower == "localhost"
        || lower.ends_with(".localhost")
        || lower.ends_with(".internal")
        || lower.ends_with(".local")
        || lower == "metadata"
    {
        return blocked("internal");
    }

    fn check_v4(
        v4: Ipv4Addr,
        blocked: &dyn Fn(&str) -> Result<(), AgentError>,
    ) -> Result<(), AgentError> {
        if v4.is_loopback() {
            return blocked("loopback");
        }
        if v4.is_private() {
            return blocked("private-network");
        }
        if v4.is_link_local() {
            // Covers 169.254.169.254, the cloud metadata endpoint.
            return blocked("link-local");
        }
        if v4.is_unspecified() || v4.is_broadcast() || v4.is_multicast() {
            return blocked("non-routable");
        }
        // Carrier-grade NAT and the IETF benchmarking range.
        let o = v4.octets();
        if o[0] == 100 && (64..128).contains(&o[1]) {
            return blocked("carrier-grade NAT");
        }
        if o[0] == 198 && (18..20).contains(&o[1]) {
            return blocked("benchmarking-range");
        }
        Ok(())
    }

    fn check_v6(
        v6: Ipv6Addr,
        blocked: &dyn Fn(&str) -> Result<(), AgentError>,
    ) -> Result<(), AgentError> {
        if v6.is_loopback() {
            return blocked("loopback");
        }
        if v6.is_unspecified() || v6.is_multicast() {
            return blocked("non-routable");
        }
        let seg = v6.segments();
        // fe80::/10 link-local
        if seg[0] & 0xffc0 == 0xfe80 {
            return blocked("link-local");
        }
        // fc00::/7 unique local
        if seg[0] & 0xfe00 == 0xfc00 {
            return blocked("unique-local");
        }
        // IPv4-mapped addresses must be judged by their v4 form.
        if let Some(v4) = v6.to_ipv4_mapped() {
            return check_v4(v4, blocked);
        }
        Ok(())
    }

    Ok(())
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
    result.push_str(&format!("\n\n... [content truncated at {max_bytes} bytes]"));
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
        assert!(err.contains("missing field `url`"));
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
    async fn test_web_fetch_malformed_headers() {
        let result = execute_web_fetch(
            json!({"url": "https://example.com", "headers": "bad"}),
            30,
            102_400,
        )
        .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Invalid arguments for web_fetch"));
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
            .mock("GET", "/custom")
            .match_header("X-Trace-Id", "abc123")
            .with_status(200)
            .with_header("content-type", "text/plain")
            .with_body("traced")
            .create_async()
            .await;

        let mut headers = HashMap::new();
        headers.insert("X-Trace-Id".to_string(), "abc123".to_string());

        let result = fetch_url(&format!("{}/custom", server.url()), &headers, 30, 102_400)
            .await
            .unwrap();

        assert!(!result.is_error);
        assert_eq!(result.content, "traced");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_web_fetch_rejects_credential_headers() {
        // The model picks these values, so allowing them would let it forge
        // authenticated requests to whatever the agent host can reach.
        for name in ["Authorization", "cookie", "Metadata-Flavor"] {
            let mut headers = HashMap::new();
            headers.insert(name.to_string(), "x".to_string());
            let err = fetch_url("https://example.com", &headers, 5, 1024)
                .await
                .unwrap_err();
            assert!(
                err.to_string().contains("may not be set"),
                "{name} was not rejected: {err}"
            );
        }
    }

    #[test]
    fn test_validate_url_rejects_loopback() {
        for url in [
            "http://127.0.0.1/admin",
            "https://localhost:8080/",
            "http://[::1]/",
        ] {
            let err = validate_and_normalize_url(url).unwrap_err();
            assert!(
                err.to_string().contains("Refusing to fetch"),
                "{url}: {err}"
            );
        }
    }

    #[test]
    fn test_validate_url_rejects_cloud_metadata() {
        // The single most valuable SSRF target: hands out instance credentials.
        let err =
            validate_and_normalize_url("http://169.254.169.254/latest/meta-data/").unwrap_err();
        assert!(err.to_string().contains("link-local"), "got: {err}");

        let err = validate_and_normalize_url("http://metadata.google.internal/").unwrap_err();
        assert!(err.to_string().contains("internal"), "got: {err}");
    }

    #[test]
    fn test_validate_url_rejects_private_ranges() {
        for url in [
            "http://10.0.0.5/",
            "http://192.168.1.1/",
            "http://172.16.0.1/",
            "http://100.64.0.1/",
        ] {
            let err = validate_and_normalize_url(url).unwrap_err();
            assert!(
                err.to_string().contains("Refusing to fetch"),
                "{url}: {err}"
            );
        }
    }

    #[test]
    fn test_validate_url_allows_public_hosts() {
        assert!(validate_and_normalize_url("https://example.com/page").is_ok());
        assert!(validate_and_normalize_url("https://8.8.8.8/").is_ok());
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
        assert!(result.unwrap_err().to_string().contains("cannot be empty"));
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
    fn test_headers_default_to_empty() {
        let args: super::super::params::WebFetchArgs =
            super::super::params::parse("web_fetch", json!({"url": "https://example.com"}))
                .unwrap();
        assert!(args.headers.is_none());
    }

    #[test]
    fn test_headers_parse_when_present() {
        let args: super::super::params::WebFetchArgs = super::super::params::parse(
            "web_fetch",
            json!({"url": "https://example.com", "headers": {"Accept": "text/html"}}),
        )
        .unwrap();
        assert_eq!(args.headers.unwrap().get("Accept").unwrap(), "text/html");
    }

    #[test]
    fn test_malformed_headers_are_rejected() {
        let err = super::super::params::parse::<super::super::params::WebFetchArgs>(
            "web_fetch",
            json!({"url": "https://example.com", "headers": "not-an-object"}),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("Invalid arguments for web_fetch"),
            "{err}"
        );
    }
}
