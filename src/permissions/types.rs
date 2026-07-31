use serde::{Deserialize, Serialize};

/// Permission mode that controls how tool access is evaluated.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    #[default]
    Default,
    AcceptEdits,
    BypassPermissions,
    Plan,
    DontAsk,
}

/// Policy that determines which tools may be executed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionPolicy {
    pub mode: PermissionMode,
    pub allowed_tools: Vec<String>,
    pub denied_tools: Vec<String>,
}

impl Default for PermissionPolicy {
    fn default() -> Self {
        Self {
            mode: PermissionMode::Default,
            allowed_tools: Vec::new(),
            denied_tools: Vec::new(),
        }
    }
}

/// Result of checking a tool name against a permission policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    Allow,
    Deny { reason: String },
    Ask,
}

/// Concatenate every string value in a tool's arguments, so an argument-scoped rule
/// can match regardless of which parameter carries the dangerous value.
fn flatten_argument_values(value: &serde_json::Value) -> String {
    let mut out = String::new();
    fn walk(v: &serde_json::Value, out: &mut String) {
        match v {
            serde_json::Value::String(s) => {
                out.push_str(s);
                out.push('\n');
            }
            serde_json::Value::Array(items) => items.iter().for_each(|i| walk(i, out)),
            serde_json::Value::Object(map) => map.values().for_each(|i| walk(i, out)),
            other => {
                out.push_str(&other.to_string());
                out.push('\n');
            }
        }
    }
    walk(value, &mut out);
    out
}

/// Write/edit tools that are auto-allowed in AcceptEdits mode.
const ACCEPT_EDITS_AUTO_ALLOW: &[&str] = &["write_file", "edit_file", "bash", "multi_edit"];

impl PermissionPolicy {
    /// Compile every configured pattern, returning a description of the first that is
    /// invalid.
    ///
    /// Call this at startup. An unparseable pattern is silently ignored during
    /// matching, which for a *deny* pattern means it fails open — a typo would quietly
    /// grant access the operator believed they had revoked. Surfacing it as a hard
    /// config error is the only safe handling.
    pub fn validate(&self) -> Result<(), String> {
        for (label, patterns) in [
            ("denied_tools", &self.denied_tools),
            ("allowed_tools", &self.allowed_tools),
        ] {
            for pattern in patterns {
                if let Err(e) = regex::Regex::new(pattern) {
                    return Err(format!("invalid regex in {label}: `{pattern}`: {e}"));
                }
            }
        }
        Ok(())
    }

    /// Log a warning when the effective policy is broader than it may appear.
    ///
    /// `Default` mode falls through to `Ask`, and a non-interactive run has no one to
    /// ask, so `Ask` executes the tool. That makes headless `Default` mode
    /// "allow everything not explicitly denied" — worth stating out loud rather than
    /// leaving implicit.
    pub fn warn_if_permissive(&self, interactive: bool) {
        match self.mode {
            PermissionMode::BypassPermissions => {
                tracing::warn!(
                    "Permission mode is `bypass_permissions`: every tool is allowed and \
                     denied_tools is ignored."
                );
            }
            PermissionMode::Default if !interactive => {
                tracing::warn!(
                    denied_tools = ?self.denied_tools,
                    "Permission mode is `default` in a non-interactive run: tools that would \
                     prompt are executed instead. Effectively every tool is allowed except \
                     the denied patterns. Use `--permission-mode dont_ask` to deny instead."
                );
            }
            PermissionMode::AcceptEdits => {
                tracing::warn!(
                    auto_allowed = ?ACCEPT_EDITS_AUTO_ALLOW,
                    "Permission mode is `accept_edits`: these tools run without prompting."
                );
            }
            _ => {}
        }
    }

    /// Returns the first denied pattern that matches `tool_name`, if any.
    ///
    /// Deny patterns are matched **unanchored** so a broad pattern catches variants.
    fn matches_denied(&self, tool_name: &str) -> Option<&str> {
        self.denied_tools.iter().find_map(|pattern| {
            match regex::Regex::new(pattern) {
                Ok(re) if re.is_match(tool_name) => Some(pattern.as_str()),
                Ok(_) => None,
                Err(e) => {
                    // Cannot evaluate the rule, so treat it as matching rather than
                    // letting a malformed deny rule fail open. `validate()` should have
                    // rejected this at startup.
                    tracing::error!(
                        pattern = %pattern,
                        error = %e,
                        "Invalid deny pattern; denying the tool rather than failing open"
                    );
                    Some(pattern.as_str())
                }
            }
        })
    }

    /// Returns `true` if `tool_name` matches any pattern in `allowed_tools`.
    ///
    /// Allow patterns are **anchored** to the whole tool name, so `read` grants
    /// `read_file` only if written as such and never accidentally matches
    /// `spread_files`. Alternations still work: `navigate|click` becomes
    /// `^(?:navigate|click)$`.
    fn matches_allowed(&self, tool_name: &str) -> bool {
        self.allowed_tools.iter().any(|pattern| {
            let anchored = format!("^(?:{pattern})$");
            regex::Regex::new(&anchored)
                .map(|re| re.is_match(tool_name))
                .unwrap_or_else(|e| {
                    tracing::error!(
                        pattern = %pattern,
                        error = %e,
                        "Invalid allow pattern; not granting access"
                    );
                    false
                })
        })
    }

    /// Check whether a tool is allowed, denied, or requires user confirmation.
    ///
    /// `read_only` comes from the tool's own [`ToolDefinition`], which is the single
    /// source of truth for what "read-only" means. It used to be decided by hardcoded
    /// name lists kept in three places that had already drifted apart, so plan mode
    /// advertised tools it then refused to run and refused tools it never advertised.
    ///
    /// Evaluation order:
    /// 1. `BypassPermissions` mode always allows.
    /// 2. `Plan` mode allows only read-only tools, denies everything else.
    /// 3. `AcceptEdits` mode: denied patterns first, then auto-allows write tools
    ///    (`write_file`, `edit_file`, `bash`, `multi_edit`), then allowed patterns, then `Ask`.
    /// 4. `Default` / `DontAsk` mode: denied patterns first, then allowed patterns.
    ///    `Default` falls through to `Ask`; `DontAsk` falls through to `Deny`.
    pub fn check(&self, tool_name: &str) -> PermissionDecision {
        // Callers that do not have the definition to hand assume the tool mutates,
        // which is the safe default in Plan mode.
        self.check_tool(tool_name, false)
    }

    /// Check a tool, given whether its definition declares it read-only.
    pub fn check_tool(&self, tool_name: &str, read_only: bool) -> PermissionDecision {
        self.check_call(tool_name, read_only, None)
    }

    /// Check a specific tool *call*, so rules can match on arguments as well as name.
    ///
    /// A name-only rule can express "deny bash" but not "deny `rm -rf`", which is the
    /// distinction operators actually want. A pattern may target arguments by writing
    /// `tool_name:pattern`; the part after the colon is matched against the call's
    /// argument values.
    pub fn check_call(
        &self,
        tool_name: &str,
        read_only: bool,
        arguments: Option<&serde_json::Value>,
    ) -> PermissionDecision {
        // Argument-scoped denials are checked first and in every mode except bypass,
        // since they exist to stop specific dangerous calls.
        if self.mode != PermissionMode::BypassPermissions {
            if let Some(args) = arguments {
                if let Some(pattern) = self.matches_denied_call(tool_name, args) {
                    return PermissionDecision::Deny {
                        reason: format!(
                            "Tool call '{tool_name}' matches denied pattern '{pattern}'"
                        ),
                    };
                }
            }
        }
        self.check_by_name(tool_name, read_only)
    }

    /// Match `tool:arg-pattern` rules against a call's argument values.
    fn matches_denied_call(&self, tool_name: &str, arguments: &serde_json::Value) -> Option<&str> {
        let arg_text = flatten_argument_values(arguments);
        self.denied_tools.iter().find_map(|pattern| {
            let (tool_part, arg_part) = pattern.split_once(':')?;
            if tool_part != tool_name {
                return None;
            }
            match regex::Regex::new(arg_part) {
                Ok(re) if re.is_match(&arg_text) => Some(pattern.as_str()),
                Ok(_) => None,
                Err(e) => {
                    tracing::error!(
                        pattern = %pattern,
                        error = %e,
                        "Invalid argument deny pattern; denying the call rather than failing open"
                    );
                    Some(pattern.as_str())
                }
            }
        })
    }

    fn check_by_name(&self, tool_name: &str, read_only: bool) -> PermissionDecision {
        match self.mode {
            PermissionMode::BypassPermissions => PermissionDecision::Allow,
            PermissionMode::Plan => {
                if read_only {
                    PermissionDecision::Allow
                } else {
                    PermissionDecision::Deny {
                        reason: format!(
                            "Tool '{tool_name}' is not allowed in Plan mode (read-only)"
                        ),
                    }
                }
            }
            PermissionMode::AcceptEdits => {
                if let Some(pattern) = self.matches_denied(tool_name) {
                    return PermissionDecision::Deny {
                        reason: format!("Tool '{tool_name}' matches denied pattern '{pattern}'"),
                    };
                }

                // Auto-allow write/edit tools
                if ACCEPT_EDITS_AUTO_ALLOW.contains(&tool_name) {
                    return PermissionDecision::Allow;
                }

                if self.matches_allowed(tool_name) {
                    return PermissionDecision::Allow;
                }

                // Nothing matched — caller decides
                PermissionDecision::Ask
            }
            PermissionMode::Default | PermissionMode::DontAsk => {
                if let Some(pattern) = self.matches_denied(tool_name) {
                    return PermissionDecision::Deny {
                        reason: format!("Tool '{tool_name}' matches denied pattern '{pattern}'"),
                    };
                }

                if self.matches_allowed(tool_name) {
                    return PermissionDecision::Allow;
                }

                // DontAsk denies instead of asking
                if self.mode == PermissionMode::DontAsk {
                    return PermissionDecision::Deny {
                        reason: format!(
                            "Tool '{tool_name}' is not in the allowed list (DontAsk mode)"
                        ),
                    };
                }

                // Nothing matched — caller decides
                PermissionDecision::Ask
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Serde roundtrip tests ---

    #[test]
    fn test_permission_mode_default_serde() {
        let mode = PermissionMode::Default;
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, "\"default\"");
        let back: PermissionMode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, PermissionMode::Default);
    }

    #[test]
    fn test_permission_mode_accept_edits_serde() {
        let mode = PermissionMode::AcceptEdits;
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, "\"accept_edits\"");
        let back: PermissionMode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, PermissionMode::AcceptEdits);
    }

    #[test]
    fn test_permission_mode_bypass_serde() {
        let mode = PermissionMode::BypassPermissions;
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, "\"bypass_permissions\"");
        let back: PermissionMode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, PermissionMode::BypassPermissions);
    }

    #[test]
    fn test_permission_mode_plan_serde() {
        let mode = PermissionMode::Plan;
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, "\"plan\"");
        let back: PermissionMode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, PermissionMode::Plan);
    }

    // --- Default values ---

    #[test]
    fn test_permission_mode_default_is_default() {
        assert_eq!(PermissionMode::default(), PermissionMode::Default);
    }

    #[test]
    fn test_permission_policy_default() {
        let policy = PermissionPolicy::default();
        assert_eq!(policy.mode, PermissionMode::Default);
        assert!(policy.allowed_tools.is_empty());
        assert!(policy.denied_tools.is_empty());
    }

    // --- BypassPermissions always allows ---

    #[test]
    fn test_bypass_allows_any_tool() {
        let policy = PermissionPolicy {
            mode: PermissionMode::BypassPermissions,
            allowed_tools: vec![],
            denied_tools: vec![".*".to_string()], // deny-all pattern ignored
        };
        assert_eq!(policy.check("bash"), PermissionDecision::Allow);
        assert_eq!(policy.check("write_file"), PermissionDecision::Allow);
        assert_eq!(policy.check("navigate"), PermissionDecision::Allow);
    }

    // --- Plan mode ---

    #[test]
    fn test_plan_mode_allows_read_file() {
        let policy = PermissionPolicy {
            mode: PermissionMode::Plan,
            ..Default::default()
        };
        assert_eq!(
            policy.check_tool("read_file", true),
            PermissionDecision::Allow
        );
    }

    #[test]
    fn test_plan_mode_allows_grep() {
        let policy = PermissionPolicy {
            mode: PermissionMode::Plan,
            ..Default::default()
        };
        assert_eq!(policy.check_tool("grep", true), PermissionDecision::Allow);
    }

    #[test]
    fn test_plan_mode_allows_glob() {
        let policy = PermissionPolicy {
            mode: PermissionMode::Plan,
            ..Default::default()
        };
        assert_eq!(policy.check_tool("glob", true), PermissionDecision::Allow);
    }

    #[test]
    fn test_plan_mode_allows_load_skill() {
        let policy = PermissionPolicy {
            mode: PermissionMode::Plan,
            ..Default::default()
        };
        assert_eq!(
            policy.check_tool("load_skill", true),
            PermissionDecision::Allow
        );
        assert_eq!(
            policy.check_tool("read_skill_resource", true),
            PermissionDecision::Allow
        );
    }

    #[test]
    fn test_plan_mode_denies_write_tools() {
        let policy = PermissionPolicy {
            mode: PermissionMode::Plan,
            ..Default::default()
        };
        let decision = policy.check("write_file");
        assert!(matches!(decision, PermissionDecision::Deny { .. }));

        let decision = policy.check("bash");
        assert!(matches!(decision, PermissionDecision::Deny { .. }));
    }

    // --- Denied takes precedence over allowed ---

    #[test]
    fn test_denied_takes_precedence() {
        let policy = PermissionPolicy {
            mode: PermissionMode::Default,
            allowed_tools: vec!["bash".to_string()],
            denied_tools: vec!["bash".to_string()],
        };
        let decision = policy.check("bash");
        assert!(matches!(decision, PermissionDecision::Deny { .. }));
    }

    // --- Regex patterns ---

    #[test]
    fn test_allowed_regex_pattern() {
        let policy = PermissionPolicy {
            mode: PermissionMode::Default,
            allowed_tools: vec!["read_.*".to_string()],
            denied_tools: vec![],
        };
        assert_eq!(
            policy.check_tool("read_file", true),
            PermissionDecision::Allow
        );
        assert_eq!(
            policy.check_tool("read_skill_resource", true),
            PermissionDecision::Allow
        );
        assert_eq!(policy.check("write_file"), PermissionDecision::Ask);
    }

    #[test]
    fn test_denied_regex_pattern() {
        let policy = PermissionPolicy {
            mode: PermissionMode::Default,
            allowed_tools: vec![],
            denied_tools: vec!["write_.*".to_string()],
        };
        let decision = policy.check("write_file");
        assert!(matches!(decision, PermissionDecision::Deny { .. }));
        assert_eq!(
            policy.check_tool("read_file", true),
            PermissionDecision::Ask
        );
    }

    #[test]
    fn test_denied_regex_precedence_over_allowed_regex() {
        let policy = PermissionPolicy {
            mode: PermissionMode::AcceptEdits,
            allowed_tools: vec![".*".to_string()],
            denied_tools: vec!["bash".to_string()],
        };
        assert!(matches!(
            policy.check("bash"),
            PermissionDecision::Deny { .. }
        ));
        assert_eq!(
            policy.check_tool("read_file", true),
            PermissionDecision::Allow
        );
    }

    // --- Default returns Ask ---

    #[test]
    fn test_default_mode_returns_ask_for_unknown() {
        let policy = PermissionPolicy::default();
        assert_eq!(policy.check("some_random_tool"), PermissionDecision::Ask);
    }

    // --- AcceptEdits auto-allows write tools ---

    #[test]
    fn test_accept_edits_auto_allows_write_file() {
        let policy = PermissionPolicy {
            mode: PermissionMode::AcceptEdits,
            allowed_tools: vec![],
            denied_tools: vec![],
        };
        assert_eq!(policy.check("write_file"), PermissionDecision::Allow);
    }

    #[test]
    fn test_accept_edits_auto_allows_edit_file() {
        let policy = PermissionPolicy {
            mode: PermissionMode::AcceptEdits,
            allowed_tools: vec![],
            denied_tools: vec![],
        };
        assert_eq!(policy.check("edit_file"), PermissionDecision::Allow);
    }

    #[test]
    fn test_accept_edits_auto_allows_bash() {
        let policy = PermissionPolicy {
            mode: PermissionMode::AcceptEdits,
            allowed_tools: vec![],
            denied_tools: vec![],
        };
        assert_eq!(policy.check("bash"), PermissionDecision::Allow);
    }

    #[test]
    fn test_accept_edits_auto_allows_multi_edit() {
        let policy = PermissionPolicy {
            mode: PermissionMode::AcceptEdits,
            allowed_tools: vec![],
            denied_tools: vec![],
        };
        assert_eq!(policy.check("multi_edit"), PermissionDecision::Allow);
    }

    #[test]
    fn test_accept_edits_returns_ask_for_unknown() {
        let policy = PermissionPolicy {
            mode: PermissionMode::AcceptEdits,
            allowed_tools: vec![],
            denied_tools: vec![],
        };
        assert_eq!(policy.check("navigate"), PermissionDecision::Ask);
    }

    #[test]
    fn test_accept_edits_denied_overrides_auto_allow() {
        let policy = PermissionPolicy {
            mode: PermissionMode::AcceptEdits,
            allowed_tools: vec![],
            denied_tools: vec!["bash".to_string()],
        };
        assert!(matches!(
            policy.check("bash"),
            PermissionDecision::Deny { .. }
        ));
        // Other write tools still auto-allowed
        assert_eq!(policy.check("write_file"), PermissionDecision::Allow);
    }

    #[test]
    fn test_accept_edits_allowed_patterns_still_work() {
        let policy = PermissionPolicy {
            mode: PermissionMode::AcceptEdits,
            allowed_tools: vec!["navigate".to_string()],
            denied_tools: vec![],
        };
        assert_eq!(policy.check("navigate"), PermissionDecision::Allow);
    }

    // --- DontAsk mode ---

    #[test]
    fn test_dont_ask_mode_serde() {
        let mode = PermissionMode::DontAsk;
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, "\"dont_ask\"");
        let back: PermissionMode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, PermissionMode::DontAsk);
    }

    #[test]
    fn test_dont_ask_denies_unknown_tools() {
        let policy = PermissionPolicy {
            mode: PermissionMode::DontAsk,
            allowed_tools: vec![],
            denied_tools: vec![],
        };
        let decision = policy.check("bash");
        assert!(matches!(decision, PermissionDecision::Deny { .. }));
    }

    #[test]
    fn test_dont_ask_allows_explicitly_allowed() {
        let policy = PermissionPolicy {
            mode: PermissionMode::DontAsk,
            allowed_tools: vec!["bash".to_string()],
            denied_tools: vec![],
        };
        assert_eq!(policy.check("bash"), PermissionDecision::Allow);
    }

    #[test]
    fn test_dont_ask_denied_takes_precedence() {
        let policy = PermissionPolicy {
            mode: PermissionMode::DontAsk,
            allowed_tools: vec!["bash".to_string()],
            denied_tools: vec!["bash".to_string()],
        };
        assert!(matches!(
            policy.check("bash"),
            PermissionDecision::Deny { .. }
        ));
    }

    #[test]
    fn test_dont_ask_allows_regex_pattern() {
        let policy = PermissionPolicy {
            mode: PermissionMode::DontAsk,
            allowed_tools: vec!["read_.*".to_string()],
            denied_tools: vec![],
        };
        assert_eq!(
            policy.check_tool("read_file", true),
            PermissionDecision::Allow
        );
        assert!(matches!(
            policy.check("write_file"),
            PermissionDecision::Deny { .. }
        ));
    }

    // --- PermissionDecision equality ---

    #[test]
    fn test_permission_decision_equality() {
        assert_eq!(PermissionDecision::Allow, PermissionDecision::Allow);
        assert_eq!(PermissionDecision::Ask, PermissionDecision::Ask);
        assert_eq!(
            PermissionDecision::Deny {
                reason: "x".to_string()
            },
            PermissionDecision::Deny {
                reason: "x".to_string()
            },
        );
        assert_ne!(PermissionDecision::Allow, PermissionDecision::Ask);
        assert_ne!(
            PermissionDecision::Deny {
                reason: "a".to_string()
            },
            PermissionDecision::Deny {
                reason: "b".to_string()
            },
        );
    }

    // --- Invalid regex is gracefully handled ---

    #[test]
    fn test_invalid_regex_in_denied_fails_closed() {
        let policy = PermissionPolicy {
            mode: PermissionMode::Default,
            allowed_tools: vec![],
            denied_tools: vec!["[invalid".to_string()],
        };
        // An unevaluatable deny rule must not grant access. A typo in a deny pattern
        // silently permitting the tool is the worst possible failure mode, so the
        // rule is treated as matching.
        assert!(matches!(
            policy.check("bash"),
            PermissionDecision::Deny { .. }
        ));
    }

    #[test]
    fn test_validate_rejects_invalid_denied_pattern() {
        let policy = PermissionPolicy {
            mode: PermissionMode::Default,
            allowed_tools: vec![],
            denied_tools: vec!["[invalid".to_string()],
        };
        let err = policy.validate().unwrap_err();
        assert!(err.contains("denied_tools"), "got: {err}");
    }

    #[test]
    fn test_validate_rejects_invalid_allowed_pattern() {
        let policy = PermissionPolicy {
            mode: PermissionMode::Default,
            allowed_tools: vec!["(unclosed".to_string()],
            denied_tools: vec![],
        };
        let err = policy.validate().unwrap_err();
        assert!(err.contains("allowed_tools"), "got: {err}");
    }

    #[test]
    fn test_validate_accepts_valid_patterns() {
        let policy = PermissionPolicy {
            mode: PermissionMode::Default,
            allowed_tools: vec!["navigate|click".to_string()],
            denied_tools: vec!["bash".to_string()],
        };
        assert!(policy.validate().is_ok());
    }

    #[test]
    fn test_allow_pattern_is_anchored() {
        let policy = PermissionPolicy {
            mode: PermissionMode::Default,
            allowed_tools: vec!["read".to_string()],
            denied_tools: vec![],
        };
        // `read` must not grant `spread_files` by substring match.
        assert_eq!(policy.check("spread_files"), PermissionDecision::Ask);
        assert_eq!(policy.check("read"), PermissionDecision::Allow);
    }

    #[test]
    fn test_allow_pattern_alternation_still_works_when_anchored() {
        let policy = PermissionPolicy {
            mode: PermissionMode::Default,
            allowed_tools: vec!["navigate|click|screenshot".to_string()],
            denied_tools: vec![],
        };
        assert_eq!(policy.check("navigate"), PermissionDecision::Allow);
        assert_eq!(policy.check("click"), PermissionDecision::Allow);
        assert_eq!(policy.check("screenshot"), PermissionDecision::Allow);
        assert_eq!(policy.check("navigate_away"), PermissionDecision::Ask);
    }

    #[test]
    fn test_invalid_regex_in_allowed_is_skipped() {
        let policy = PermissionPolicy {
            mode: PermissionMode::Default,
            allowed_tools: vec!["[invalid".to_string()],
            denied_tools: vec![],
        };
        assert_eq!(policy.check("bash"), PermissionDecision::Ask);
    }

    // --- PermissionPolicy serde roundtrip ---

    #[test]
    fn test_permission_policy_serde_roundtrip() {
        let policy = PermissionPolicy {
            mode: PermissionMode::Plan,
            allowed_tools: vec!["read_.*".to_string()],
            denied_tools: vec!["bash".to_string()],
        };
        let json = serde_json::to_string(&policy).unwrap();
        let back: PermissionPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(back.mode, PermissionMode::Plan);
        assert_eq!(back.allowed_tools, vec!["read_.*"]);
        assert_eq!(back.denied_tools, vec!["bash"]);
    }

    // --- Argument-scoped rules ---
    //
    // A name-only rule can only express "deny bash", never "deny `rm -rf`", which is
    // the distinction an operator actually wants.

    fn deny_call_policy(patterns: &[&str]) -> PermissionPolicy {
        PermissionPolicy {
            mode: PermissionMode::Default,
            allowed_tools: vec![],
            denied_tools: patterns.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn test_argument_scoped_rule_denies_matching_call() {
        let policy = deny_call_policy(&[r"bash:rm\s+-rf"]);
        let args = serde_json::json!({"command": "rm -rf /tmp/x"});
        assert!(matches!(
            policy.check_call("bash", false, Some(&args)),
            PermissionDecision::Deny { .. }
        ));
    }

    #[test]
    fn test_argument_scoped_rule_allows_other_calls_to_same_tool() {
        let policy = deny_call_policy(&[r"bash:rm\s+-rf"]);
        let args = serde_json::json!({"command": "cargo test"});
        // The whole tool is not banned — only the dangerous invocation.
        assert_eq!(
            policy.check_call("bash", false, Some(&args)),
            PermissionDecision::Ask
        );
    }

    #[test]
    fn test_argument_scoped_rule_does_not_affect_other_tools() {
        let policy = deny_call_policy(&[r"bash:rm\s+-rf"]);
        let args = serde_json::json!({"content": "rm -rf /"});
        assert_eq!(
            policy.check_call("write_file", false, Some(&args)),
            PermissionDecision::Ask
        );
    }

    #[test]
    fn test_argument_scoped_rule_searches_all_argument_values() {
        // The dangerous value may not be in the parameter the operator expected.
        let policy = deny_call_policy(&["bash:secret"]);
        let args = serde_json::json!({"command": "echo hi", "env": {"TOKEN": "secret"}});
        assert!(matches!(
            policy.check_call("bash", false, Some(&args)),
            PermissionDecision::Deny { .. }
        ));
    }

    #[test]
    fn test_name_only_rules_still_deny_the_whole_tool() {
        let policy = deny_call_policy(&["bash"]);
        let args = serde_json::json!({"command": "cargo test"});
        assert!(matches!(
            policy.check_call("bash", false, Some(&args)),
            PermissionDecision::Deny { .. }
        ));
    }

    #[test]
    fn test_argument_rules_are_ignored_under_bypass() {
        let mut policy = deny_call_policy(&[r"bash:rm\s+-rf"]);
        policy.mode = PermissionMode::BypassPermissions;
        let args = serde_json::json!({"command": "rm -rf /"});
        assert_eq!(
            policy.check_call("bash", false, Some(&args)),
            PermissionDecision::Allow
        );
    }

    #[test]
    fn test_invalid_argument_pattern_fails_closed() {
        let policy = deny_call_policy(&["bash:[unclosed"]);
        let args = serde_json::json!({"command": "ls"});
        assert!(matches!(
            policy.check_call("bash", false, Some(&args)),
            PermissionDecision::Deny { .. }
        ));
    }
}
