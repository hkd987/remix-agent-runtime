use std::collections::HashMap;
use tokio::io::{BufReader, BufWriter};
use tokio::process::{ChildStdin, ChildStdout};

/// Supported LSP language identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LspLanguage {
    Rust,
    TypeScript,
    JavaScript,
    Python,
    Go,
}

impl LspLanguage {
    /// Detect language from file extension.
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext {
            "rs" => Some(Self::Rust),
            "ts" | "tsx" => Some(Self::TypeScript),
            "js" | "jsx" | "mjs" | "cjs" => Some(Self::JavaScript),
            "py" | "pyi" => Some(Self::Python),
            "go" => Some(Self::Go),
            _ => None,
        }
    }

    /// Default server binary name for this language.
    pub fn default_server_command(&self) -> &'static str {
        match self {
            Self::Rust => "rust-analyzer",
            Self::TypeScript => "typescript-language-server",
            Self::JavaScript => "typescript-language-server",
            Self::Python => "pyright-langserver",
            Self::Go => "gopls",
        }
    }

    /// Default server arguments.
    pub fn default_server_args(&self) -> Vec<&'static str> {
        match self {
            Self::Rust => vec![],
            Self::TypeScript | Self::JavaScript => vec!["--stdio"],
            Self::Python => vec!["--stdio"],
            Self::Go => vec!["serve"],
        }
    }

    /// LSP language identifier string.
    pub fn language_id(&self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::TypeScript => "typescript",
            Self::JavaScript => "javascript",
            Self::Python => "python",
            Self::Go => "go",
        }
    }

    /// Parse from a string identifier.
    pub fn from_str_id(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "rust" | "rs" => Some(Self::Rust),
            "typescript" | "ts" => Some(Self::TypeScript),
            "javascript" | "js" => Some(Self::JavaScript),
            "python" | "py" => Some(Self::Python),
            "go" | "golang" => Some(Self::Go),
            _ => None,
        }
    }
}

/// Handle to a running LSP server process.
pub struct LspServerHandle {
    pub language: LspLanguage,
    pub process: tokio::process::Child,
    pub stdin: BufWriter<ChildStdin>,
    pub stdout: BufReader<ChildStdout>,
    pub request_id: i64,
    pub initialized: bool,
    /// Pending diagnostics keyed by file URI.
    pub diagnostics: HashMap<String, Vec<lsp_types::Diagnostic>>,
}

impl std::fmt::Debug for LspServerHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LspServerHandle")
            .field("language", &self.language)
            .field("request_id", &self.request_id)
            .field("initialized", &self.initialized)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_extension() {
        assert_eq!(LspLanguage::from_extension("rs"), Some(LspLanguage::Rust));
        assert_eq!(
            LspLanguage::from_extension("ts"),
            Some(LspLanguage::TypeScript)
        );
        assert_eq!(
            LspLanguage::from_extension("tsx"),
            Some(LspLanguage::TypeScript)
        );
        assert_eq!(
            LspLanguage::from_extension("js"),
            Some(LspLanguage::JavaScript)
        );
        assert_eq!(
            LspLanguage::from_extension("jsx"),
            Some(LspLanguage::JavaScript)
        );
        assert_eq!(
            LspLanguage::from_extension("mjs"),
            Some(LspLanguage::JavaScript)
        );
        assert_eq!(LspLanguage::from_extension("py"), Some(LspLanguage::Python));
        assert_eq!(
            LspLanguage::from_extension("pyi"),
            Some(LspLanguage::Python)
        );
        assert_eq!(LspLanguage::from_extension("go"), Some(LspLanguage::Go));
        assert_eq!(LspLanguage::from_extension("txt"), None);
        assert_eq!(LspLanguage::from_extension(""), None);
    }

    #[test]
    fn test_default_server_command() {
        assert_eq!(LspLanguage::Rust.default_server_command(), "rust-analyzer");
        assert_eq!(
            LspLanguage::TypeScript.default_server_command(),
            "typescript-language-server"
        );
        assert_eq!(
            LspLanguage::JavaScript.default_server_command(),
            "typescript-language-server"
        );
        assert_eq!(
            LspLanguage::Python.default_server_command(),
            "pyright-langserver"
        );
        assert_eq!(LspLanguage::Go.default_server_command(), "gopls");
    }

    #[test]
    fn test_language_id() {
        assert_eq!(LspLanguage::Rust.language_id(), "rust");
        assert_eq!(LspLanguage::TypeScript.language_id(), "typescript");
        assert_eq!(LspLanguage::JavaScript.language_id(), "javascript");
        assert_eq!(LspLanguage::Python.language_id(), "python");
        assert_eq!(LspLanguage::Go.language_id(), "go");
    }

    #[test]
    fn test_from_str_id() {
        assert_eq!(LspLanguage::from_str_id("rust"), Some(LspLanguage::Rust));
        assert_eq!(LspLanguage::from_str_id("rs"), Some(LspLanguage::Rust));
        assert_eq!(
            LspLanguage::from_str_id("TypeScript"),
            Some(LspLanguage::TypeScript)
        );
        assert_eq!(
            LspLanguage::from_str_id("ts"),
            Some(LspLanguage::TypeScript)
        );
        assert_eq!(
            LspLanguage::from_str_id("python"),
            Some(LspLanguage::Python)
        );
        assert_eq!(LspLanguage::from_str_id("py"), Some(LspLanguage::Python));
        assert_eq!(LspLanguage::from_str_id("Go"), Some(LspLanguage::Go));
        assert_eq!(LspLanguage::from_str_id("golang"), Some(LspLanguage::Go));
        assert_eq!(LspLanguage::from_str_id("unknown"), None);
    }

    #[test]
    fn test_default_server_args() {
        assert!(LspLanguage::Rust.default_server_args().is_empty());
        assert_eq!(
            LspLanguage::TypeScript.default_server_args(),
            vec!["--stdio"]
        );
        assert_eq!(LspLanguage::Python.default_server_args(), vec!["--stdio"]);
        assert_eq!(LspLanguage::Go.default_server_args(), vec!["serve"]);
    }
}
