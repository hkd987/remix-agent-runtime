/// Default system prompt for interactive TUI chat mode.
pub const INTERACTIVE_SYSTEM_PROMPT: &str = r#"You are a coding assistant working in an interactive terminal session.
The user will ask you to write, modify, debug, and explain code.

Guidelines:
- Discover the project environment first: check the directory structure,
  build system, and language versions before making changes.
- Follow a PLAN → BUILD → VERIFY cycle for each task.
- Write tests alongside code when possible.
- Run tests and linters before declaring work complete.
- Be concise in explanations — the user can see your tool calls.
- When you're done with a task, summarize what changed and any follow-up steps.

You have access to file operations (read, write, edit), shell execution (bash),
search tools (glob, grep), and web fetching. Use them proactively."#;

/// Get the system prompt for interactive chat mode.
/// Returns the custom prompt if provided, otherwise the built-in default.
pub fn get_interactive_prompt(custom: Option<&str>) -> &str {
    custom.unwrap_or(INTERACTIVE_SYSTEM_PROMPT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_prompt_returned_when_no_custom() {
        let result = get_interactive_prompt(None);
        assert_eq!(result, INTERACTIVE_SYSTEM_PROMPT);
    }

    #[test]
    fn test_custom_prompt_overrides_default() {
        let custom = "You are a helpful bot.";
        let result = get_interactive_prompt(Some(custom));
        assert_eq!(result, custom);
    }

    #[test]
    fn test_prompt_contains_plan_build_verify() {
        assert!(INTERACTIVE_SYSTEM_PROMPT.contains("PLAN"));
        assert!(INTERACTIVE_SYSTEM_PROMPT.contains("BUILD"));
        assert!(INTERACTIVE_SYSTEM_PROMPT.contains("VERIFY"));
    }

    #[test]
    fn test_prompt_contains_key_guidelines() {
        assert!(INTERACTIVE_SYSTEM_PROMPT.contains("coding assistant"));
        assert!(INTERACTIVE_SYSTEM_PROMPT.contains("tests"));
        assert!(INTERACTIVE_SYSTEM_PROMPT.contains("linters"));
        assert!(INTERACTIVE_SYSTEM_PROMPT.contains("tool calls"));
    }

    #[test]
    fn test_prompt_mentions_available_tools() {
        assert!(INTERACTIVE_SYSTEM_PROMPT.contains("file operations"));
        assert!(INTERACTIVE_SYSTEM_PROMPT.contains("shell execution"));
        assert!(INTERACTIVE_SYSTEM_PROMPT.contains("search tools"));
        assert!(INTERACTIVE_SYSTEM_PROMPT.contains("web fetching"));
    }

    #[test]
    fn test_prompt_is_not_empty() {
        assert!(!INTERACTIVE_SYSTEM_PROMPT.is_empty());
    }

    #[test]
    fn test_empty_custom_prompt_is_used() {
        let result = get_interactive_prompt(Some(""));
        assert_eq!(result, "");
    }
}
