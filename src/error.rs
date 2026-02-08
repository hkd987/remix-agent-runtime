use std::fmt;

/// Unified error type for remix-agent-runtime.
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("LLM authentication error: {0}")]
    LlmAuth(String),

    #[error("LLM API error: {0}")]
    Llm(String),

    #[error("LLM rate limited")]
    LlmRateLimited,

    #[error("Browser error: {0}")]
    Browser(String),

    #[error("MCP error: {0}")]
    Mcp(String),

    #[error("Tool execution error: {0}")]
    ToolExecution(String),

    #[error("Timeout after {0}s")]
    Timeout(u64),

    #[error("Max iterations ({0}) reached")]
    MaxIterations(u32),

    #[error("Webhook error: {0}")]
    Webhook(String),

    #[error("Skill error: {0}")]
    Skill(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("YAML error: {0}")]
    Yaml(#[from] serde_yaml::Error),
}

/// Exit codes matching the PRD specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitStatus {
    Success = 0,
    AgentError = 1,
    ConfigError = 2,
}

impl From<ExitStatus> for std::process::ExitCode {
    fn from(status: ExitStatus) -> Self {
        std::process::ExitCode::from(status as u8)
    }
}

impl AgentError {
    /// Map this error to the appropriate CLI exit status.
    pub fn exit_status(&self) -> ExitStatus {
        match self {
            AgentError::Config(_) | AgentError::Yaml(_) => ExitStatus::ConfigError,
            _ => ExitStatus::AgentError,
        }
    }
}

impl fmt::Display for ExitStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExitStatus::Success => write!(f, "success"),
            ExitStatus::AgentError => write!(f, "agent error"),
            ExitStatus::ConfigError => write!(f, "config error"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display_messages() {
        let err = AgentError::Config("missing api_key".into());
        assert_eq!(err.to_string(), "Configuration error: missing api_key");

        let err = AgentError::LlmAuth("invalid key".into());
        assert_eq!(err.to_string(), "LLM authentication error: invalid key");

        let err = AgentError::Llm("server error".into());
        assert_eq!(err.to_string(), "LLM API error: server error");

        let err = AgentError::LlmRateLimited;
        assert_eq!(err.to_string(), "LLM rate limited");

        let err = AgentError::Browser("crashed".into());
        assert_eq!(err.to_string(), "Browser error: crashed");

        let err = AgentError::Mcp("connection lost".into());
        assert_eq!(err.to_string(), "MCP error: connection lost");

        let err = AgentError::ToolExecution("navigate failed".into());
        assert_eq!(err.to_string(), "Tool execution error: navigate failed");

        let err = AgentError::Timeout(300);
        assert_eq!(err.to_string(), "Timeout after 300s");

        let err = AgentError::MaxIterations(50);
        assert_eq!(err.to_string(), "Max iterations (50) reached");

        let err = AgentError::Webhook("connection refused".into());
        assert_eq!(err.to_string(), "Webhook error: connection refused");
    }

    #[test]
    fn test_exit_status_from_error() {
        assert_eq!(
            AgentError::Config("bad".into()).exit_status(),
            ExitStatus::ConfigError
        );
        assert_eq!(
            AgentError::Llm("err".into()).exit_status(),
            ExitStatus::AgentError
        );
        assert_eq!(
            AgentError::Timeout(60).exit_status(),
            ExitStatus::AgentError
        );
        assert_eq!(
            AgentError::MaxIterations(50).exit_status(),
            ExitStatus::AgentError
        );
    }

    #[test]
    fn test_exit_code_conversion() {
        let code: std::process::ExitCode = ExitStatus::Success.into();
        // ExitCode doesn't expose its value, so we just verify it converts
        let _ = code;

        let code: std::process::ExitCode = ExitStatus::AgentError.into();
        let _ = code;

        let code: std::process::ExitCode = ExitStatus::ConfigError.into();
        let _ = code;
    }

    #[test]
    fn test_io_error_conversion() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let agent_err: AgentError = io_err.into();
        assert!(matches!(agent_err, AgentError::Io(_)));
        assert!(agent_err.to_string().contains("file not found"));
    }

    #[test]
    fn test_skill_error() {
        let err = AgentError::Skill("skill not found".into());
        assert_eq!(err.to_string(), "Skill error: skill not found");
        assert_eq!(err.exit_status(), ExitStatus::AgentError);
    }

    #[test]
    fn test_json_error_conversion() {
        let json_err = serde_json::from_str::<serde_json::Value>("invalid").unwrap_err();
        let agent_err: AgentError = json_err.into();
        assert!(matches!(agent_err, AgentError::Json(_)));
    }
}
