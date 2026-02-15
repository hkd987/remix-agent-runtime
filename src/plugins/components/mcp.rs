use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

use crate::error::AgentError;

/// A single MCP server configuration from .mcp.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

/// The root .mcp.json format (matches Claude Code format)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(non_snake_case)]
pub struct McpConfigFile {
    #[serde(default)]
    pub mcpServers: HashMap<String, McpServerConfig>,
}

const PLUGIN_ROOT_PLACEHOLDER: &str = "${CLAUDE_PLUGIN_ROOT}";

/// Parse a .mcp.json file and substitute ${CLAUDE_PLUGIN_ROOT} placeholders.
pub fn parse_mcp_config(
    config_path: &Path,
    plugin_root: &Path,
) -> Result<McpConfigFile, AgentError> {
    let content = std::fs::read_to_string(config_path)
        .map_err(|e| AgentError::Plugin(format!("Failed to read .mcp.json: {e}")))?;

    let substituted = content.replace(PLUGIN_ROOT_PLACEHOLDER, &plugin_root.to_string_lossy());

    let config: McpConfigFile = serde_json::from_str(&substituted)
        .map_err(|e| AgentError::Plugin(format!("Invalid .mcp.json: {e}")))?;

    Ok(config)
}

/// Build a tokio::process::Command from an MCP server config.
pub fn build_mcp_command(
    server_config: &McpServerConfig,
    plugin_root: &Path,
) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(&server_config.command);
    cmd.args(&server_config.args);
    for (key, val) in &server_config.env {
        let substituted = val.replace(PLUGIN_ROOT_PLACEHOLDER, &plugin_root.to_string_lossy());
        cmd.env(key, substituted);
    }
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use tempfile::TempDir;

    fn write_mcp_json(dir: &Path, content: &str) -> std::path::PathBuf {
        let path = dir.join(".mcp.json");
        fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn test_parse_simple_config() {
        let dir = TempDir::new().unwrap();
        let config_path = write_mcp_json(
            dir.path(),
            &json!({
                "mcpServers": {
                    "my-server": {
                        "command": "npx",
                        "args": ["-y", "my-mcp-server"]
                    }
                }
            })
            .to_string(),
        );

        let config = parse_mcp_config(&config_path, dir.path()).unwrap();
        assert_eq!(config.mcpServers.len(), 1);

        let server = &config.mcpServers["my-server"];
        assert_eq!(server.command, "npx");
        assert_eq!(server.args, vec!["-y", "my-mcp-server"]);
        assert!(server.env.is_empty());
    }

    #[test]
    fn test_parse_config_with_env() {
        let dir = TempDir::new().unwrap();
        let config_path = write_mcp_json(
            dir.path(),
            &json!({
                "mcpServers": {
                    "db-server": {
                        "command": "node",
                        "args": ["server.js"],
                        "env": {
                            "DB_HOST": "localhost",
                            "DB_PORT": "5432"
                        }
                    }
                }
            })
            .to_string(),
        );

        let config = parse_mcp_config(&config_path, dir.path()).unwrap();
        let server = &config.mcpServers["db-server"];
        assert_eq!(server.env["DB_HOST"], "localhost");
        assert_eq!(server.env["DB_PORT"], "5432");
    }

    #[test]
    fn test_parse_config_with_plugin_root_substitution() {
        let dir = TempDir::new().unwrap();
        let plugin_root = dir.path().join("plugins").join("my-plugin");
        fs::create_dir_all(&plugin_root).unwrap();

        let raw_json = json!({
            "mcpServers": {
                "local-server": {
                    "command": "node",
                    "args": ["${CLAUDE_PLUGIN_ROOT}/dist/server.js"],
                    "env": {
                        "CONFIG_PATH": "${CLAUDE_PLUGIN_ROOT}/config.yaml"
                    }
                }
            }
        })
        .to_string();

        let config_path = write_mcp_json(dir.path(), &raw_json);
        let config = parse_mcp_config(&config_path, &plugin_root).unwrap();
        let server = &config.mcpServers["local-server"];

        let expected_arg = format!("{}/dist/server.js", plugin_root.display());
        assert_eq!(server.args[0], expected_arg);

        let expected_env = format!("{}/config.yaml", plugin_root.display());
        assert_eq!(server.env["CONFIG_PATH"], expected_env);
    }

    #[test]
    fn test_parse_config_multiple_servers() {
        let dir = TempDir::new().unwrap();
        let config_path = write_mcp_json(
            dir.path(),
            &json!({
                "mcpServers": {
                    "server-a": {
                        "command": "npx",
                        "args": ["server-a"]
                    },
                    "server-b": {
                        "command": "python",
                        "args": ["-m", "server_b"]
                    }
                }
            })
            .to_string(),
        );

        let config = parse_mcp_config(&config_path, dir.path()).unwrap();
        assert_eq!(config.mcpServers.len(), 2);
        assert!(config.mcpServers.contains_key("server-a"));
        assert!(config.mcpServers.contains_key("server-b"));
    }

    #[test]
    fn test_parse_config_empty_servers() {
        let dir = TempDir::new().unwrap();
        let config_path = write_mcp_json(dir.path(), &json!({ "mcpServers": {} }).to_string());

        let config = parse_mcp_config(&config_path, dir.path()).unwrap();
        assert!(config.mcpServers.is_empty());
    }

    #[test]
    fn test_parse_config_missing_mcp_servers_defaults_empty() {
        let dir = TempDir::new().unwrap();
        let config_path = write_mcp_json(dir.path(), "{}");

        let config = parse_mcp_config(&config_path, dir.path()).unwrap();
        assert!(config.mcpServers.is_empty());
    }

    #[test]
    fn test_parse_config_file_not_found() {
        let dir = TempDir::new().unwrap();
        let fake_path = dir.path().join("nonexistent.json");

        let err = parse_mcp_config(&fake_path, dir.path()).unwrap_err();
        assert!(err.to_string().contains("Failed to read .mcp.json"));
    }

    #[test]
    fn test_parse_config_invalid_json() {
        let dir = TempDir::new().unwrap();
        let config_path = write_mcp_json(dir.path(), "not valid json {{{");

        let err = parse_mcp_config(&config_path, dir.path()).unwrap_err();
        assert!(err.to_string().contains("Invalid .mcp.json"));
    }

    #[test]
    fn test_parse_config_defaults_for_optional_fields() {
        let dir = TempDir::new().unwrap();
        let config_path = write_mcp_json(
            dir.path(),
            &json!({
                "mcpServers": {
                    "minimal": {
                        "command": "echo"
                    }
                }
            })
            .to_string(),
        );

        let config = parse_mcp_config(&config_path, dir.path()).unwrap();
        let server = &config.mcpServers["minimal"];
        assert_eq!(server.command, "echo");
        assert!(server.args.is_empty());
        assert!(server.env.is_empty());
    }

    #[test]
    fn test_build_mcp_command_basic() {
        let server_config = McpServerConfig {
            command: "npx".to_string(),
            args: vec!["-y".to_string(), "my-mcp-server".to_string()],
            env: HashMap::new(),
        };

        let plugin_root = Path::new("/tmp/test-plugin");
        let cmd = build_mcp_command(&server_config, plugin_root);

        let program = cmd.as_std().get_program();
        assert_eq!(program, "npx");

        let args: Vec<&std::ffi::OsStr> = cmd.as_std().get_args().collect();
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "-y");
        assert_eq!(args[1], "my-mcp-server");
    }

    #[test]
    fn test_build_mcp_command_with_env() {
        let mut env = HashMap::new();
        env.insert("API_KEY".to_string(), "secret".to_string());
        env.insert("BASE_URL".to_string(), "http://localhost".to_string());

        let server_config = McpServerConfig {
            command: "node".to_string(),
            args: vec!["server.js".to_string()],
            env,
        };

        let plugin_root = Path::new("/tmp/test-plugin");
        let cmd = build_mcp_command(&server_config, plugin_root);

        let envs: HashMap<&std::ffi::OsStr, Option<&std::ffi::OsStr>> =
            cmd.as_std().get_envs().collect();
        assert!(envs.contains_key(std::ffi::OsStr::new("API_KEY")));
        assert!(envs.contains_key(std::ffi::OsStr::new("BASE_URL")));
    }

    #[test]
    fn test_build_mcp_command_env_substitution() {
        let mut env = HashMap::new();
        env.insert(
            "CONFIG".to_string(),
            "${CLAUDE_PLUGIN_ROOT}/config.yaml".to_string(),
        );

        let server_config = McpServerConfig {
            command: "node".to_string(),
            args: vec![],
            env,
        };

        let plugin_root = Path::new("/home/user/plugins/my-plugin");
        let cmd = build_mcp_command(&server_config, plugin_root);

        let envs: HashMap<&std::ffi::OsStr, Option<&std::ffi::OsStr>> =
            cmd.as_std().get_envs().collect();
        let config_val = envs[std::ffi::OsStr::new("CONFIG")].unwrap();
        assert_eq!(config_val, "/home/user/plugins/my-plugin/config.yaml");
    }

    #[test]
    fn test_mcp_server_config_serde_roundtrip() {
        let mut env = HashMap::new();
        env.insert("KEY".to_string(), "value".to_string());

        let config = McpServerConfig {
            command: "npx".to_string(),
            args: vec!["-y".to_string(), "server".to_string()],
            env,
        };

        let serialized = serde_json::to_string(&config).unwrap();
        let deserialized: McpServerConfig = serde_json::from_str(&serialized).unwrap();

        assert_eq!(deserialized.command, "npx");
        assert_eq!(deserialized.args, vec!["-y", "server"]);
        assert_eq!(deserialized.env["KEY"], "value");
    }

    #[test]
    fn test_mcp_config_file_serde_roundtrip() {
        let mut servers = HashMap::new();
        servers.insert(
            "test".to_string(),
            McpServerConfig {
                command: "echo".to_string(),
                args: vec![],
                env: HashMap::new(),
            },
        );

        let config = McpConfigFile {
            mcpServers: servers,
        };

        let serialized = serde_json::to_string(&config).unwrap();
        let deserialized: McpConfigFile = serde_json::from_str(&serialized).unwrap();

        assert_eq!(deserialized.mcpServers.len(), 1);
        assert_eq!(deserialized.mcpServers["test"].command, "echo");
    }

    #[test]
    fn test_no_substitution_when_no_placeholder() {
        let dir = TempDir::new().unwrap();
        let config_path = write_mcp_json(
            dir.path(),
            &json!({
                "mcpServers": {
                    "plain": {
                        "command": "/usr/bin/node",
                        "args": ["/opt/server.js"],
                        "env": {
                            "HOME": "/home/user"
                        }
                    }
                }
            })
            .to_string(),
        );

        let config = parse_mcp_config(&config_path, dir.path()).unwrap();
        let server = &config.mcpServers["plain"];
        assert_eq!(server.command, "/usr/bin/node");
        assert_eq!(server.args[0], "/opt/server.js");
        assert_eq!(server.env["HOME"], "/home/user");
    }
}
