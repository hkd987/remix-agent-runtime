use super::types::LspLanguage;
use crate::config::schema::LspConfig;

/// Resolve the server command for a language, considering user overrides.
pub fn resolve_server_command(
    language: LspLanguage,
    config: &LspConfig,
) -> Option<(String, Vec<String>)> {
    // Check user overrides first
    let lang_key = language.language_id();
    if let Some(cmd) = config.server_overrides.get(lang_key) {
        // User override: parse "command arg1 arg2" format
        let parts: Vec<&str> = cmd.split_whitespace().collect();
        if parts.is_empty() {
            return None;
        }
        let binary = parts[0].to_string();
        let args: Vec<String> = parts[1..].iter().map(|s| s.to_string()).collect();
        return Some((binary, args));
    }

    // Check if default server binary is available on PATH
    let default_cmd = language.default_server_command();
    match which::which(default_cmd) {
        Ok(_) => {
            let args = language
                .default_server_args()
                .iter()
                .map(|s| s.to_string())
                .collect();
            Some((default_cmd.to_string(), args))
        }
        Err(_) => None,
    }
}

/// Detect the LSP language from a file path.
pub fn detect_language_from_file(file_path: &str) -> Option<LspLanguage> {
    let path = std::path::Path::new(file_path);
    path.extension()
        .and_then(|ext| ext.to_str())
        .and_then(LspLanguage::from_extension)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_detect_language_from_file() {
        assert_eq!(
            detect_language_from_file("src/main.rs"),
            Some(LspLanguage::Rust)
        );
        assert_eq!(
            detect_language_from_file("src/app.ts"),
            Some(LspLanguage::TypeScript)
        );
        assert_eq!(
            detect_language_from_file("src/app.tsx"),
            Some(LspLanguage::TypeScript)
        );
        assert_eq!(
            detect_language_from_file("src/index.js"),
            Some(LspLanguage::JavaScript)
        );
        assert_eq!(
            detect_language_from_file("lib/utils.py"),
            Some(LspLanguage::Python)
        );
        assert_eq!(
            detect_language_from_file("cmd/main.go"),
            Some(LspLanguage::Go)
        );
        assert_eq!(detect_language_from_file("README.md"), None);
        assert_eq!(detect_language_from_file("Makefile"), None);
    }

    #[test]
    fn test_resolve_with_override() {
        let config = LspConfig {
            enabled: true,
            request_timeout_secs: 30,
            server_overrides: {
                let mut m = HashMap::new();
                m.insert("rust".to_string(), "custom-analyzer --flag".to_string());
                m
            },
        };
        let result = resolve_server_command(LspLanguage::Rust, &config);
        assert!(result.is_some());
        let (cmd, args) = result.unwrap();
        assert_eq!(cmd, "custom-analyzer");
        assert_eq!(args, vec!["--flag"]);
    }

    #[test]
    fn test_resolve_without_override_missing_binary() {
        let config = LspConfig {
            enabled: true,
            request_timeout_secs: 30,
            server_overrides: HashMap::new(),
        };
        // Go server (gopls) might not be installed; just verify no panic
        let _result = resolve_server_command(LspLanguage::Go, &config);
    }
}
