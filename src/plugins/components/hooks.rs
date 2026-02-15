use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::error::AgentError;

/// When a hook fires relative to tool execution or agent lifecycle.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HookTiming {
    PreToolUse,
    PostToolUse,
    PostToolUseFailure,
    SessionStart,
    SessionEnd,
    PreCompact,
    Stop,
    SubagentStart,
    SubagentStop,
}

/// A single hook command to execute.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookCommand {
    #[serde(rename = "type")]
    pub hook_type: String,
    pub command: String,
}

/// A hook rule: matcher pattern + list of commands.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookRule {
    /// Pipe-separated tool name patterns, e.g. "navigate|click".
    pub matcher: String,
    pub hooks: Vec<HookCommand>,
}

/// Root hooks.json format (matches Claude Code format).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HooksConfigFile {
    pub hooks: HooksConfig,
}

/// Hook configuration with pre/post tool use and lifecycle rules.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HooksConfig {
    #[serde(default, rename = "PreToolUse")]
    pub pre_tool_use: Vec<HookRule>,
    #[serde(default, rename = "PostToolUse")]
    pub post_tool_use: Vec<HookRule>,
    #[serde(default, rename = "PostToolUseFailure")]
    pub post_tool_use_failure: Vec<HookRule>,
    #[serde(default, rename = "SessionStart")]
    pub session_start: Vec<HookRule>,
    #[serde(default, rename = "SessionEnd")]
    pub session_end: Vec<HookRule>,
    #[serde(default, rename = "PreCompact")]
    pub pre_compact: Vec<HookRule>,
    #[serde(default, rename = "Stop")]
    pub stop: Vec<HookRule>,
    #[serde(default, rename = "SubagentStart")]
    pub subagent_start: Vec<HookRule>,
    #[serde(default, rename = "SubagentStop")]
    pub subagent_stop: Vec<HookRule>,
}

/// A resolved hook entry ready for execution.
#[derive(Debug, Clone)]
pub struct ResolvedHook {
    pub timing: HookTiming,
    pub matcher: String,
    pub command: String,
    pub plugin_root: PathBuf,
}

/// Registry of all discovered hooks.
#[derive(Debug, Clone, Default)]
pub struct HookRegistry {
    hooks: Vec<ResolvedHook>,
}

impl HookRegistry {
    pub fn new() -> Self {
        Self { hooks: Vec::new() }
    }

    /// Parse a hooks.json file and add all hooks to the registry.
    pub fn load_from_file(
        &mut self,
        config_path: &Path,
        plugin_root: &Path,
    ) -> Result<(), AgentError> {
        let config_file = parse_hooks_config(config_path)?;
        let plugin_root = plugin_root.to_path_buf();

        let timing_rules: &[(HookTiming, &[HookRule])] = &[
            (HookTiming::PreToolUse, &config_file.hooks.pre_tool_use),
            (HookTiming::PostToolUse, &config_file.hooks.post_tool_use),
            (
                HookTiming::PostToolUseFailure,
                &config_file.hooks.post_tool_use_failure,
            ),
            (HookTiming::SessionStart, &config_file.hooks.session_start),
            (HookTiming::SessionEnd, &config_file.hooks.session_end),
            (HookTiming::PreCompact, &config_file.hooks.pre_compact),
            (HookTiming::Stop, &config_file.hooks.stop),
            (HookTiming::SubagentStart, &config_file.hooks.subagent_start),
            (HookTiming::SubagentStop, &config_file.hooks.subagent_stop),
        ];

        for (timing, rules) in timing_rules {
            for rule in *rules {
                for cmd in &rule.hooks {
                    self.hooks.push(ResolvedHook {
                        timing: timing.clone(),
                        matcher: rule.matcher.clone(),
                        command: cmd.command.clone(),
                        plugin_root: plugin_root.clone(),
                    });
                }
            }
        }

        Ok(())
    }

    /// Find all hooks matching a tool name and timing.
    pub fn matching_hooks(&self, tool_name: &str, timing: &HookTiming) -> Vec<&ResolvedHook> {
        self.hooks
            .iter()
            .filter(|hook| {
                if &hook.timing != timing {
                    return false;
                }
                // Build regex from pipe-separated matcher pattern.
                // Wrap in ^(...)$ to ensure full match.
                let pattern = format!("^({})$", hook.matcher);
                match Regex::new(&pattern) {
                    Ok(re) => re.is_match(tool_name),
                    Err(_) => {
                        tracing::warn!(
                            matcher = %hook.matcher,
                            "Invalid hook matcher regex, skipping"
                        );
                        false
                    }
                }
            })
            .collect()
    }

    /// Find all hooks matching a lifecycle timing (ignoring matcher patterns).
    /// Used for lifecycle events like SessionStart, Stop, etc.
    pub fn lifecycle_hooks(&self, timing: &HookTiming) -> Vec<&ResolvedHook> {
        self.hooks
            .iter()
            .filter(|hook| &hook.timing == timing)
            .collect()
    }

    pub fn len(&self) -> usize {
        self.hooks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }
}

/// Parse a hooks.json file from disk.
pub fn parse_hooks_config(config_path: &Path) -> Result<HooksConfigFile, AgentError> {
    let content = std::fs::read_to_string(config_path)
        .map_err(|e| AgentError::Plugin(format!("Failed to read hooks.json: {e}")))?;
    serde_json::from_str(&content)
        .map_err(|e| AgentError::Plugin(format!("Invalid hooks.json: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_hooks_json(dir: &TempDir, content: &str) -> PathBuf {
        let path = dir.path().join("hooks.json");
        fs::write(&path, content).unwrap();
        path
    }

    fn sample_hooks_json() -> &'static str {
        r#"{
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "navigate|click",
                        "hooks": [
                            { "type": "command", "command": "echo pre-hook" }
                        ]
                    }
                ],
                "PostToolUse": [
                    {
                        "matcher": "screenshot",
                        "hooks": [
                            { "type": "command", "command": "echo post-hook" }
                        ]
                    }
                ]
            }
        }"#
    }

    #[test]
    fn test_parse_hooks_config_valid() {
        let dir = TempDir::new().unwrap();
        let path = write_hooks_json(&dir, sample_hooks_json());
        let config = parse_hooks_config(&path).unwrap();
        assert_eq!(config.hooks.pre_tool_use.len(), 1);
        assert_eq!(config.hooks.post_tool_use.len(), 1);
        assert_eq!(config.hooks.pre_tool_use[0].matcher, "navigate|click");
        assert_eq!(config.hooks.post_tool_use[0].matcher, "screenshot");
    }

    #[test]
    fn test_parse_hooks_config_invalid_json() {
        let dir = TempDir::new().unwrap();
        let path = write_hooks_json(&dir, "not valid json");
        let result = parse_hooks_config(&path);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid hooks.json"));
    }

    #[test]
    fn test_parse_hooks_config_file_not_found() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nonexistent.json");
        let result = parse_hooks_config(&path);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Failed to read hooks.json"));
    }

    #[test]
    fn test_parse_hooks_config_empty_hooks() {
        let dir = TempDir::new().unwrap();
        let path = write_hooks_json(
            &dir,
            r#"{ "hooks": { "PreToolUse": [], "PostToolUse": [] } }"#,
        );
        let config = parse_hooks_config(&path).unwrap();
        assert!(config.hooks.pre_tool_use.is_empty());
        assert!(config.hooks.post_tool_use.is_empty());
    }

    #[test]
    fn test_parse_hooks_config_missing_post_tool_use() {
        let dir = TempDir::new().unwrap();
        let path = write_hooks_json(
            &dir,
            r#"{ "hooks": { "PreToolUse": [{ "matcher": "bash", "hooks": [{ "type": "command", "command": "echo hi" }] }] } }"#,
        );
        let config = parse_hooks_config(&path).unwrap();
        assert_eq!(config.hooks.pre_tool_use.len(), 1);
        assert!(config.hooks.post_tool_use.is_empty());
    }

    #[test]
    fn test_hook_registry_new_is_empty() {
        let registry = HookRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn test_hook_registry_default_is_empty() {
        let registry = HookRegistry::default();
        assert!(registry.is_empty());
    }

    #[test]
    fn test_hook_registry_load_from_file() {
        let dir = TempDir::new().unwrap();
        let path = write_hooks_json(&dir, sample_hooks_json());
        let mut registry = HookRegistry::new();
        registry.load_from_file(&path, dir.path()).unwrap();
        assert_eq!(registry.len(), 2);
        assert!(!registry.is_empty());
    }

    #[test]
    fn test_hook_registry_load_multiple_commands_per_rule() {
        let dir = TempDir::new().unwrap();
        let json = r#"{
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "bash",
                        "hooks": [
                            { "type": "command", "command": "echo first" },
                            { "type": "command", "command": "echo second" }
                        ]
                    }
                ],
                "PostToolUse": []
            }
        }"#;
        let path = write_hooks_json(&dir, json);
        let mut registry = HookRegistry::new();
        registry.load_from_file(&path, dir.path()).unwrap();
        assert_eq!(registry.len(), 2);
    }

    #[test]
    fn test_hook_registry_load_multiple_files() {
        let dir1 = TempDir::new().unwrap();
        let dir2 = TempDir::new().unwrap();

        let path1 = write_hooks_json(
            &dir1,
            r#"{ "hooks": { "PreToolUse": [{ "matcher": "navigate", "hooks": [{ "type": "command", "command": "echo a" }] }] } }"#,
        );
        let path2 = write_hooks_json(
            &dir2,
            r#"{ "hooks": { "PostToolUse": [{ "matcher": "click", "hooks": [{ "type": "command", "command": "echo b" }] }] } }"#,
        );

        let mut registry = HookRegistry::new();
        registry.load_from_file(&path1, dir1.path()).unwrap();
        registry.load_from_file(&path2, dir2.path()).unwrap();
        assert_eq!(registry.len(), 2);
    }

    #[test]
    fn test_matching_hooks_pre_tool_use() {
        let dir = TempDir::new().unwrap();
        let path = write_hooks_json(&dir, sample_hooks_json());
        let mut registry = HookRegistry::new();
        registry.load_from_file(&path, dir.path()).unwrap();

        let matches = registry.matching_hooks("navigate", &HookTiming::PreToolUse);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].command, "echo pre-hook");
    }

    #[test]
    fn test_matching_hooks_post_tool_use() {
        let dir = TempDir::new().unwrap();
        let path = write_hooks_json(&dir, sample_hooks_json());
        let mut registry = HookRegistry::new();
        registry.load_from_file(&path, dir.path()).unwrap();

        let matches = registry.matching_hooks("screenshot", &HookTiming::PostToolUse);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].command, "echo post-hook");
    }

    #[test]
    fn test_matching_hooks_pipe_separated_pattern() {
        let dir = TempDir::new().unwrap();
        let path = write_hooks_json(&dir, sample_hooks_json());
        let mut registry = HookRegistry::new();
        registry.load_from_file(&path, dir.path()).unwrap();

        // "click" should also match "navigate|click"
        let matches = registry.matching_hooks("click", &HookTiming::PreToolUse);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].command, "echo pre-hook");
    }

    #[test]
    fn test_matching_hooks_no_match() {
        let dir = TempDir::new().unwrap();
        let path = write_hooks_json(&dir, sample_hooks_json());
        let mut registry = HookRegistry::new();
        registry.load_from_file(&path, dir.path()).unwrap();

        let matches = registry.matching_hooks("type_text", &HookTiming::PreToolUse);
        assert!(matches.is_empty());
    }

    #[test]
    fn test_matching_hooks_wrong_timing() {
        let dir = TempDir::new().unwrap();
        let path = write_hooks_json(&dir, sample_hooks_json());
        let mut registry = HookRegistry::new();
        registry.load_from_file(&path, dir.path()).unwrap();

        // "navigate" has a PreToolUse hook, not PostToolUse
        let matches = registry.matching_hooks("navigate", &HookTiming::PostToolUse);
        assert!(matches.is_empty());
    }

    #[test]
    fn test_matching_hooks_partial_name_no_match() {
        let dir = TempDir::new().unwrap();
        let path = write_hooks_json(&dir, sample_hooks_json());
        let mut registry = HookRegistry::new();
        registry.load_from_file(&path, dir.path()).unwrap();

        // "nav" should NOT match "navigate" because of ^(...)$ anchoring
        let matches = registry.matching_hooks("nav", &HookTiming::PreToolUse);
        assert!(matches.is_empty());
    }

    #[test]
    fn test_matching_hooks_regex_wildcard_pattern() {
        let dir = TempDir::new().unwrap();
        let json = r#"{
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": ".*",
                        "hooks": [{ "type": "command", "command": "echo catch-all" }]
                    }
                ],
                "PostToolUse": []
            }
        }"#;
        let path = write_hooks_json(&dir, json);
        let mut registry = HookRegistry::new();
        registry.load_from_file(&path, dir.path()).unwrap();

        let matches = registry.matching_hooks("anything_at_all", &HookTiming::PreToolUse);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].command, "echo catch-all");
    }

    #[test]
    fn test_matching_hooks_invalid_regex_skipped() {
        let mut registry = HookRegistry::new();
        registry.hooks.push(ResolvedHook {
            timing: HookTiming::PreToolUse,
            matcher: "[invalid regex".to_string(),
            command: "echo bad".to_string(),
            plugin_root: PathBuf::from("/tmp"),
        });

        // Invalid regex should be skipped gracefully
        let matches = registry.matching_hooks("anything", &HookTiming::PreToolUse);
        assert!(matches.is_empty());
    }

    #[test]
    fn test_resolved_hook_stores_plugin_root() {
        let dir = TempDir::new().unwrap();
        let plugin_root = dir.path().to_path_buf();
        let path = write_hooks_json(&dir, sample_hooks_json());
        let mut registry = HookRegistry::new();
        registry.load_from_file(&path, &plugin_root).unwrap();

        let matches = registry.matching_hooks("navigate", &HookTiming::PreToolUse);
        assert_eq!(matches[0].plugin_root, plugin_root);
    }

    #[test]
    fn test_hook_timing_equality() {
        assert_eq!(HookTiming::PreToolUse, HookTiming::PreToolUse);
        assert_eq!(HookTiming::PostToolUse, HookTiming::PostToolUse);
        assert_ne!(HookTiming::PreToolUse, HookTiming::PostToolUse);
        assert_eq!(
            HookTiming::PostToolUseFailure,
            HookTiming::PostToolUseFailure
        );
        assert_eq!(HookTiming::SessionStart, HookTiming::SessionStart);
        assert_eq!(HookTiming::SessionEnd, HookTiming::SessionEnd);
        assert_eq!(HookTiming::PreCompact, HookTiming::PreCompact);
        assert_eq!(HookTiming::Stop, HookTiming::Stop);
        assert_eq!(HookTiming::SubagentStart, HookTiming::SubagentStart);
        assert_eq!(HookTiming::SubagentStop, HookTiming::SubagentStop);
        assert_ne!(HookTiming::SessionStart, HookTiming::SessionEnd);
    }

    #[test]
    fn test_hook_timing_serde_roundtrip() {
        let timings = vec![
            HookTiming::PreToolUse,
            HookTiming::PostToolUse,
            HookTiming::PostToolUseFailure,
            HookTiming::SessionStart,
            HookTiming::SessionEnd,
            HookTiming::PreCompact,
            HookTiming::Stop,
            HookTiming::SubagentStart,
            HookTiming::SubagentStop,
        ];
        for timing in timings {
            let json = serde_json::to_string(&timing).unwrap();
            let deserialized: HookTiming = serde_json::from_str(&json).unwrap();
            assert_eq!(deserialized, timing);
        }
    }

    #[test]
    fn test_lifecycle_hooks_method() {
        let dir = TempDir::new().unwrap();
        let json = r#"{
            "hooks": {
                "SessionStart": [
                    {
                        "matcher": ".*",
                        "hooks": [{ "type": "command", "command": "echo session start" }]
                    }
                ],
                "Stop": [
                    {
                        "matcher": ".*",
                        "hooks": [{ "type": "command", "command": "echo stop" }]
                    }
                ]
            }
        }"#;
        let path = write_hooks_json(&dir, json);
        let mut registry = HookRegistry::new();
        registry.load_from_file(&path, dir.path()).unwrap();

        let session_start = registry.lifecycle_hooks(&HookTiming::SessionStart);
        assert_eq!(session_start.len(), 1);
        assert_eq!(session_start[0].command, "echo session start");

        let stop = registry.lifecycle_hooks(&HookTiming::Stop);
        assert_eq!(stop.len(), 1);
        assert_eq!(stop[0].command, "echo stop");

        let empty = registry.lifecycle_hooks(&HookTiming::PreCompact);
        assert!(empty.is_empty());
    }

    #[test]
    fn test_hooks_config_with_new_timings() {
        let dir = TempDir::new().unwrap();
        let json = r#"{
            "hooks": {
                "PreToolUse": [],
                "PostToolUse": [],
                "PostToolUseFailure": [
                    {
                        "matcher": "bash",
                        "hooks": [{ "type": "command", "command": "echo failure" }]
                    }
                ],
                "SessionStart": [
                    {
                        "matcher": ".*",
                        "hooks": [{ "type": "command", "command": "echo start" }]
                    }
                ],
                "SessionEnd": [],
                "PreCompact": [],
                "Stop": [],
                "SubagentStart": [],
                "SubagentStop": []
            }
        }"#;
        let path = write_hooks_json(&dir, json);
        let mut registry = HookRegistry::new();
        registry.load_from_file(&path, dir.path()).unwrap();
        assert_eq!(registry.len(), 2);

        let failure_hooks = registry.matching_hooks("bash", &HookTiming::PostToolUseFailure);
        assert_eq!(failure_hooks.len(), 1);
    }

    #[test]
    fn test_hook_command_serde_roundtrip() {
        let cmd = HookCommand {
            hook_type: "command".to_string(),
            command: "echo hello".to_string(),
        };
        let json = serde_json::to_value(&cmd).unwrap();
        assert_eq!(json["type"], "command");
        assert_eq!(json["command"], "echo hello");

        let deserialized: HookCommand = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.hook_type, "command");
        assert_eq!(deserialized.command, "echo hello");
    }

    #[test]
    fn test_hook_rule_serde_roundtrip() {
        let rule = HookRule {
            matcher: "navigate|click".to_string(),
            hooks: vec![HookCommand {
                hook_type: "command".to_string(),
                command: "echo test".to_string(),
            }],
        };
        let json = serde_json::to_value(&rule).unwrap();
        let deserialized: HookRule = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.matcher, "navigate|click");
        assert_eq!(deserialized.hooks.len(), 1);
    }

    #[test]
    fn test_hooks_config_file_serde_roundtrip() {
        let config = HooksConfigFile {
            hooks: HooksConfig {
                pre_tool_use: vec![HookRule {
                    matcher: "bash".to_string(),
                    hooks: vec![HookCommand {
                        hook_type: "command".to_string(),
                        command: "echo pre".to_string(),
                    }],
                }],
                post_tool_use: vec![],
                post_tool_use_failure: vec![],
                session_start: vec![],
                session_end: vec![],
                pre_compact: vec![],
                stop: vec![],
                subagent_start: vec![],
                subagent_stop: vec![],
            },
        };
        let json_str = serde_json::to_string(&config).unwrap();
        let deserialized: HooksConfigFile = serde_json::from_str(&json_str).unwrap();
        assert_eq!(deserialized.hooks.pre_tool_use.len(), 1);
        assert!(deserialized.hooks.post_tool_use.is_empty());
    }

    #[test]
    fn test_hook_registry_clone() {
        let dir = TempDir::new().unwrap();
        let path = write_hooks_json(&dir, sample_hooks_json());
        let mut registry = HookRegistry::new();
        registry.load_from_file(&path, dir.path()).unwrap();

        let cloned = registry.clone();
        assert_eq!(cloned.len(), registry.len());
    }

    #[test]
    fn test_multiple_rules_same_timing() {
        let dir = TempDir::new().unwrap();
        let json = r#"{
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "navigate",
                        "hooks": [{ "type": "command", "command": "echo first" }]
                    },
                    {
                        "matcher": "navigate|click",
                        "hooks": [{ "type": "command", "command": "echo second" }]
                    }
                ],
                "PostToolUse": []
            }
        }"#;
        let path = write_hooks_json(&dir, json);
        let mut registry = HookRegistry::new();
        registry.load_from_file(&path, dir.path()).unwrap();

        // "navigate" matches both rules
        let matches = registry.matching_hooks("navigate", &HookTiming::PreToolUse);
        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn test_load_from_file_error_propagation() {
        let mut registry = HookRegistry::new();
        let result = registry.load_from_file(Path::new("/nonexistent/hooks.json"), Path::new("/"));
        assert!(result.is_err());
    }
}
