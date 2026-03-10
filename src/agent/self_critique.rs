use crate::config::schema::SelfCritiqueConfig;
use crate::llm::types::ContentBlock;

/// Result of a self-critique evaluation.
#[derive(Debug, Clone)]
pub struct CritiqueResult {
    pub approved: bool,
    pub reasoning: String,
    pub suggestions: Option<String>,
}

/// System prompt for the critique model.
pub const CRITIQUE_SYSTEM_PROMPT: &str = r#"You are a careful action reviewer. Your job is to review planned tool calls before they are executed and check for potential issues.

Review the planned actions and determine if they should proceed. Consider:
1. Are the tool arguments correct and complete?
2. Could this action have unintended side effects?
3. Does this action align with the overall task objective?
4. Are there any obvious errors or oversights?

Respond in this exact format:
APPROVED: yes/no
REASONING: <your analysis>
SUGGESTIONS: <optional improvements, or "none">"#;

/// Check if the current set of planned actions should trigger a critique.
///
/// Returns `false` if critique is disabled, if there are no `ToolUse` blocks,
/// or if none of the tool names match the configured trigger list.
pub fn should_critique(config: &SelfCritiqueConfig, planned_actions: &[ContentBlock]) -> bool {
    if !config.enabled {
        return false;
    }

    let tool_names: Vec<&str> = planned_actions
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolUse { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();

    if tool_names.is_empty() {
        return false;
    }

    match &config.trigger_tools {
        Some(triggers) => tool_names
            .iter()
            .any(|name| triggers.iter().any(|t| t == name)),
        None => true, // critique all tool uses when no trigger filter is set
    }
}

/// Build the user message for the critique LLM call.
///
/// Formats the task context and all planned actions (tool uses and text blocks)
/// into a structured prompt for the critique model.
pub fn build_critique_prompt(planned_actions: &[ContentBlock], task_context: &str) -> String {
    let mut prompt = format!("## Current Task Context\n{task_context}\n\n## Planned Actions\n");

    for block in planned_actions {
        match block {
            ContentBlock::ToolUse { name, input, .. } => {
                let input_str =
                    serde_json::to_string_pretty(input).unwrap_or_else(|_| input.to_string());
                prompt.push_str(&format!("Tool: {name}\nInput: {input_str}\n\n"));
            }
            ContentBlock::Text { text } => {
                prompt.push_str(&format!("Assistant says: {text}\n\n"));
            }
            _ => {}
        }
    }

    prompt.push_str("Please review these planned actions.");
    prompt
}

/// Parse the critique model's response into a `CritiqueResult`.
///
/// Expects the response in the format:
/// ```text
/// APPROVED: yes/no
/// REASONING: <analysis>
/// SUGGESTIONS: <improvements or "none">
/// ```
///
/// If the format is unexpected, defaults to approved (fail-open).
pub fn parse_critique_response(response_text: &str) -> CritiqueResult {
    let text_lower = response_text.to_lowercase();

    let approved = if let Some(line) = text_lower.lines().find(|l| l.starts_with("approved:")) {
        line.contains("yes")
    } else {
        // Default to approved if format is unexpected (fail-open)
        true
    };

    let reasoning = response_text
        .lines()
        .find(|l| l.to_lowercase().starts_with("reasoning:"))
        .map(|l| {
            l.get("reasoning:".len()..)
                .unwrap_or(l)
                .trim()
                .to_string()
        })
        .unwrap_or_else(|| response_text.to_string());

    let suggestions = response_text
        .lines()
        .find(|l| l.to_lowercase().starts_with("suggestions:"))
        .and_then(|l| {
            let s = l
                .get("suggestions:".len()..)
                .unwrap_or(l)
                .trim()
                .to_string();
            if s.to_lowercase() == "none" {
                None
            } else {
                Some(s)
            }
        });

    CritiqueResult {
        approved,
        reasoning,
        suggestions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_tool_use(name: &str, input: serde_json::Value) -> ContentBlock {
        ContentBlock::ToolUse {
            id: format!("toolu_{name}"),
            name: name.to_string(),
            input,
        }
    }

    fn make_text(text: &str) -> ContentBlock {
        ContentBlock::Text {
            text: text.to_string(),
        }
    }

    #[test]
    fn test_default_config_disabled() {
        let config = SelfCritiqueConfig::default();
        assert!(!config.enabled);
        assert!(config.model.is_none());
        assert_eq!(config.max_tokens, 2048);
        assert!(config.trigger_tools.is_none());
        assert_eq!(config.max_rounds, 2);
    }

    #[test]
    fn test_should_critique_disabled() {
        let config = SelfCritiqueConfig::default();
        let actions = vec![make_tool_use("navigate", json!({"url": "https://example.com"}))];
        assert!(!should_critique(&config, &actions));
    }

    #[test]
    fn test_should_critique_no_tool_use() {
        let config = SelfCritiqueConfig {
            enabled: true,
            ..Default::default()
        };
        let actions = vec![make_text("Just some text")];
        assert!(!should_critique(&config, &actions));
    }

    #[test]
    fn test_should_critique_all_tools_when_no_triggers() {
        let config = SelfCritiqueConfig {
            enabled: true,
            trigger_tools: None,
            ..Default::default()
        };
        let actions = vec![make_tool_use("anything", json!({}))];
        assert!(should_critique(&config, &actions));
    }

    #[test]
    fn test_should_critique_matching_trigger_tool() {
        let config = SelfCritiqueConfig {
            enabled: true,
            trigger_tools: Some(vec!["navigate".to_string(), "execute".to_string()]),
            ..Default::default()
        };
        let actions = vec![make_tool_use("navigate", json!({"url": "https://example.com"}))];
        assert!(should_critique(&config, &actions));
    }

    #[test]
    fn test_should_critique_non_matching_trigger_tool() {
        let config = SelfCritiqueConfig {
            enabled: true,
            trigger_tools: Some(vec!["navigate".to_string()]),
            ..Default::default()
        };
        let actions = vec![make_tool_use("screenshot", json!({}))];
        assert!(!should_critique(&config, &actions));
    }

    #[test]
    fn test_build_critique_prompt_includes_task_context() {
        let actions = vec![make_tool_use("navigate", json!({"url": "https://example.com"}))];
        let prompt = build_critique_prompt(&actions, "Navigate to example.com and take a screenshot");
        assert!(prompt.contains("Navigate to example.com and take a screenshot"));
        assert!(prompt.contains("## Current Task Context"));
    }

    #[test]
    fn test_build_critique_prompt_includes_tool_details() {
        let actions = vec![make_tool_use(
            "navigate",
            json!({"url": "https://example.com"}),
        )];
        let prompt = build_critique_prompt(&actions, "test context");
        assert!(prompt.contains("Tool: navigate"));
        assert!(prompt.contains("https://example.com"));
    }

    #[test]
    fn test_build_critique_prompt_includes_text_blocks() {
        let actions = vec![
            make_text("I will navigate to the page now."),
            make_tool_use("navigate", json!({"url": "https://example.com"})),
        ];
        let prompt = build_critique_prompt(&actions, "test");
        assert!(prompt.contains("Assistant says: I will navigate to the page now."));
    }

    #[test]
    fn test_parse_critique_response_approved() {
        let response = "APPROVED: yes\nREASONING: The action looks correct.\nSUGGESTIONS: none";
        let result = parse_critique_response(response);
        assert!(result.approved);
        assert_eq!(result.reasoning, "The action looks correct.");
        assert!(result.suggestions.is_none());
    }

    #[test]
    fn test_parse_critique_response_rejected() {
        let response =
            "APPROVED: no\nREASONING: The URL is invalid.\nSUGGESTIONS: Fix the URL format.";
        let result = parse_critique_response(response);
        assert!(!result.approved);
        assert_eq!(result.reasoning, "The URL is invalid.");
        assert_eq!(result.suggestions.as_deref(), Some("Fix the URL format."));
    }

    #[test]
    fn test_parse_critique_response_with_suggestions() {
        let response = "APPROVED: yes\nREASONING: Mostly fine.\nSUGGESTIONS: Add a timeout parameter.";
        let result = parse_critique_response(response);
        assert!(result.approved);
        assert_eq!(
            result.suggestions.as_deref(),
            Some("Add a timeout parameter.")
        );
    }

    #[test]
    fn test_parse_critique_response_no_suggestions() {
        let response = "APPROVED: yes\nREASONING: All good.\nSUGGESTIONS: none";
        let result = parse_critique_response(response);
        assert!(result.suggestions.is_none());
    }

    #[test]
    fn test_parse_critique_response_malformed_defaults_approved() {
        let response = "This is not in the expected format at all.";
        let result = parse_critique_response(response);
        assert!(result.approved); // fail-open
        // Reasoning falls back to the full response text
        assert_eq!(result.reasoning, response);
        assert!(result.suggestions.is_none());
    }

    #[test]
    fn test_critique_system_prompt_not_empty() {
        assert!(!CRITIQUE_SYSTEM_PROMPT.is_empty());
        assert!(CRITIQUE_SYSTEM_PROMPT.contains("APPROVED"));
        assert!(CRITIQUE_SYSTEM_PROMPT.contains("REASONING"));
        assert!(CRITIQUE_SYSTEM_PROMPT.contains("SUGGESTIONS"));
    }

    #[test]
    fn test_config_serde_roundtrip() {
        let config = SelfCritiqueConfig {
            enabled: true,
            model: Some("claude-haiku-3-20240307".to_string()),
            max_tokens: 4096,
            trigger_tools: Some(vec!["navigate".to_string(), "execute".to_string()]),
            max_rounds: 3,
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: SelfCritiqueConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, config);

        // Also verify YAML roundtrip
        let yaml = serde_yaml::to_string(&config).unwrap();
        let from_yaml: SelfCritiqueConfig = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(from_yaml, config);
    }
}
