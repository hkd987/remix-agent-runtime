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

/// Read-only tools that are always allowed in Plan mode.
const PLAN_MODE_ALLOWED: &[&str] = &[
    "read_file",
    "grep",
    "glob",
    "load_skill",
    "read_skill_resource",
];

impl PermissionPolicy {
    /// Check whether a tool is allowed, denied, or requires user confirmation.
    ///
    /// Evaluation order:
    /// 1. `BypassPermissions` mode always allows.
    /// 2. `Plan` mode allows only read-only tools, denies everything else.
    /// 3. `denied_tools` patterns are checked first (deny wins over allow).
    /// 4. `allowed_tools` patterns are checked next.
    /// 5. If nothing matches, `Ask` is returned (caller decides).
    pub fn check(&self, tool_name: &str) -> PermissionDecision {
        match self.mode {
            PermissionMode::BypassPermissions => PermissionDecision::Allow,
            PermissionMode::Plan => {
                if PLAN_MODE_ALLOWED.contains(&tool_name) {
                    PermissionDecision::Allow
                } else {
                    PermissionDecision::Deny {
                        reason: format!(
                            "Tool '{tool_name}' is not allowed in Plan mode (read-only)"
                        ),
                    }
                }
            }
            PermissionMode::Default | PermissionMode::AcceptEdits => {
                // Denied patterns take precedence
                for pattern in &self.denied_tools {
                    if let Ok(re) = regex::Regex::new(pattern) {
                        if re.is_match(tool_name) {
                            return PermissionDecision::Deny {
                                reason: format!(
                                    "Tool '{tool_name}' matches denied pattern '{pattern}'"
                                ),
                            };
                        }
                    }
                }

                // Allowed patterns
                for pattern in &self.allowed_tools {
                    if let Ok(re) = regex::Regex::new(pattern) {
                        if re.is_match(tool_name) {
                            return PermissionDecision::Allow;
                        }
                    }
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
        assert_eq!(policy.check("read_file"), PermissionDecision::Allow);
    }

    #[test]
    fn test_plan_mode_allows_grep() {
        let policy = PermissionPolicy {
            mode: PermissionMode::Plan,
            ..Default::default()
        };
        assert_eq!(policy.check("grep"), PermissionDecision::Allow);
    }

    #[test]
    fn test_plan_mode_allows_glob() {
        let policy = PermissionPolicy {
            mode: PermissionMode::Plan,
            ..Default::default()
        };
        assert_eq!(policy.check("glob"), PermissionDecision::Allow);
    }

    #[test]
    fn test_plan_mode_allows_load_skill() {
        let policy = PermissionPolicy {
            mode: PermissionMode::Plan,
            ..Default::default()
        };
        assert_eq!(policy.check("load_skill"), PermissionDecision::Allow);
        assert_eq!(
            policy.check("read_skill_resource"),
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
        assert_eq!(policy.check("read_file"), PermissionDecision::Allow);
        assert_eq!(
            policy.check("read_skill_resource"),
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
        assert_eq!(policy.check("read_file"), PermissionDecision::Ask);
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
        assert_eq!(policy.check("read_file"), PermissionDecision::Allow);
    }

    // --- Default/AcceptEdits returns Ask ---

    #[test]
    fn test_default_mode_returns_ask_for_unknown() {
        let policy = PermissionPolicy::default();
        assert_eq!(policy.check("some_random_tool"), PermissionDecision::Ask);
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
    fn test_invalid_regex_in_denied_is_skipped() {
        let policy = PermissionPolicy {
            mode: PermissionMode::Default,
            allowed_tools: vec![],
            denied_tools: vec!["[invalid".to_string()],
        };
        // Invalid regex should be skipped, falling through to Ask
        assert_eq!(policy.check("bash"), PermissionDecision::Ask);
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
}
