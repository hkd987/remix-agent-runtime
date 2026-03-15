/// Definition of a slash command available in the TUI.
#[derive(Debug, Clone)]
pub struct SlashCommand {
    pub name: &'static str,
    pub description: &'static str,
    pub aliases: &'static [&'static str],
}

/// Action to take when a slash command is parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandAction {
    Help,
    Clear,
    Compact,
    Context,
    Diff,
    Undo,
    Rewind,
    Quit,
}

/// Registry of all available slash commands.
pub struct CommandRegistry {
    commands: Vec<SlashCommand>,
}

impl CommandRegistry {
    /// Create a new registry with all MVP commands.
    pub fn new() -> Self {
        Self {
            commands: vec![
                SlashCommand {
                    name: "/help",
                    description: "Show available commands and shortcuts",
                    aliases: &["/h", "/?"],
                },
                SlashCommand {
                    name: "/clear",
                    description: "Clear chat display, keep session",
                    aliases: &["/cls"],
                },
                SlashCommand {
                    name: "/compact",
                    description: "Trigger context compaction",
                    aliases: &[],
                },
                SlashCommand {
                    name: "/context",
                    description: "Show token usage breakdown",
                    aliases: &["/ctx"],
                },
                SlashCommand {
                    name: "/diff",
                    description: "Show git diff since session start",
                    aliases: &[],
                },
                SlashCommand {
                    name: "/undo",
                    description: "Restore files to before last edit",
                    aliases: &[],
                },
                SlashCommand {
                    name: "/rewind",
                    description: "Show checkpoint list for selective restore",
                    aliases: &[],
                },
                SlashCommand {
                    name: "/quit",
                    description: "Exit the session",
                    aliases: &["/q", "/exit"],
                },
            ],
        }
    }

    /// Parse user input into a command action.
    /// Returns None if the input is not a recognized command.
    pub fn parse(input: &str) -> Option<CommandAction> {
        let trimmed = input.trim();
        match trimmed {
            "/help" | "/h" | "/?" => Some(CommandAction::Help),
            "/clear" | "/cls" => Some(CommandAction::Clear),
            "/compact" => Some(CommandAction::Compact),
            "/context" | "/ctx" => Some(CommandAction::Context),
            "/diff" => Some(CommandAction::Diff),
            "/undo" => Some(CommandAction::Undo),
            "/rewind" => Some(CommandAction::Rewind),
            "/quit" | "/q" | "/exit" => Some(CommandAction::Quit),
            _ => None,
        }
    }

    /// Filter commands whose name or aliases start with the given prefix.
    /// Used for autocomplete in the command palette.
    pub fn filter(&self, prefix: &str) -> Vec<&SlashCommand> {
        if prefix.is_empty() {
            return self.commands.iter().collect();
        }
        let lower_prefix = prefix.to_lowercase();
        self.commands
            .iter()
            .filter(|cmd| {
                cmd.name.to_lowercase().starts_with(&lower_prefix)
                    || cmd
                        .aliases
                        .iter()
                        .any(|a| a.to_lowercase().starts_with(&lower_prefix))
            })
            .collect()
    }

    /// Get all registered commands.
    pub fn all(&self) -> &[SlashCommand] {
        &self.commands
    }
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_help() {
        assert_eq!(CommandRegistry::parse("/help"), Some(CommandAction::Help));
    }

    #[test]
    fn test_parse_help_alias_h() {
        assert_eq!(CommandRegistry::parse("/h"), Some(CommandAction::Help));
    }

    #[test]
    fn test_parse_help_alias_question() {
        assert_eq!(CommandRegistry::parse("/?"), Some(CommandAction::Help));
    }

    #[test]
    fn test_parse_clear() {
        assert_eq!(CommandRegistry::parse("/clear"), Some(CommandAction::Clear));
    }

    #[test]
    fn test_parse_clear_alias() {
        assert_eq!(CommandRegistry::parse("/cls"), Some(CommandAction::Clear));
    }

    #[test]
    fn test_parse_compact() {
        assert_eq!(
            CommandRegistry::parse("/compact"),
            Some(CommandAction::Compact)
        );
    }

    #[test]
    fn test_parse_context() {
        assert_eq!(
            CommandRegistry::parse("/context"),
            Some(CommandAction::Context)
        );
    }

    #[test]
    fn test_parse_context_alias() {
        assert_eq!(CommandRegistry::parse("/ctx"), Some(CommandAction::Context));
    }

    #[test]
    fn test_parse_diff() {
        assert_eq!(CommandRegistry::parse("/diff"), Some(CommandAction::Diff));
    }

    #[test]
    fn test_parse_undo() {
        assert_eq!(CommandRegistry::parse("/undo"), Some(CommandAction::Undo));
    }

    #[test]
    fn test_parse_rewind() {
        assert_eq!(
            CommandRegistry::parse("/rewind"),
            Some(CommandAction::Rewind)
        );
    }

    #[test]
    fn test_parse_quit() {
        assert_eq!(CommandRegistry::parse("/quit"), Some(CommandAction::Quit));
    }

    #[test]
    fn test_parse_quit_alias_q() {
        assert_eq!(CommandRegistry::parse("/q"), Some(CommandAction::Quit));
    }

    #[test]
    fn test_parse_quit_alias_exit() {
        assert_eq!(CommandRegistry::parse("/exit"), Some(CommandAction::Quit));
    }

    #[test]
    fn test_parse_unknown_returns_none() {
        assert_eq!(CommandRegistry::parse("/unknown"), None);
        assert_eq!(CommandRegistry::parse("hello"), None);
        assert_eq!(CommandRegistry::parse(""), None);
    }

    #[test]
    fn test_parse_with_whitespace() {
        assert_eq!(
            CommandRegistry::parse("  /help  "),
            Some(CommandAction::Help)
        );
        assert_eq!(CommandRegistry::parse("  /q  "), Some(CommandAction::Quit));
    }

    #[test]
    fn test_parse_case_sensitive() {
        // Commands are case-sensitive — uppercase should not match
        assert_eq!(CommandRegistry::parse("/HELP"), None);
        assert_eq!(CommandRegistry::parse("/Help"), None);
    }

    #[test]
    fn test_filter_empty_prefix_returns_all() {
        let registry = CommandRegistry::new();
        let results = registry.filter("");
        assert_eq!(results.len(), registry.all().len());
    }

    #[test]
    fn test_filter_with_prefix() {
        let registry = CommandRegistry::new();
        let results = registry.filter("/c");
        let names: Vec<&str> = results.iter().map(|c| c.name).collect();
        assert!(names.contains(&"/clear"));
        assert!(names.contains(&"/compact"));
        assert!(names.contains(&"/context"));
        assert!(!names.contains(&"/help"));
    }

    #[test]
    fn test_filter_with_alias_prefix() {
        let registry = CommandRegistry::new();
        // "/ct" should match /context via alias /ctx
        let results = registry.filter("/ct");
        let names: Vec<&str> = results.iter().map(|c| c.name).collect();
        assert!(names.contains(&"/context"));
    }

    #[test]
    fn test_filter_no_match() {
        let registry = CommandRegistry::new();
        let results = registry.filter("/xyz");
        assert!(results.is_empty());
    }

    #[test]
    fn test_filter_case_insensitive() {
        let registry = CommandRegistry::new();
        let results = registry.filter("/H");
        let names: Vec<&str> = results.iter().map(|c| c.name).collect();
        assert!(names.contains(&"/help"));
    }

    #[test]
    fn test_all_returns_all_commands() {
        let registry = CommandRegistry::new();
        assert_eq!(registry.all().len(), 8);
    }

    #[test]
    fn test_default_impl() {
        let registry = CommandRegistry::default();
        assert_eq!(registry.all().len(), 8);
    }

    #[test]
    fn test_command_action_equality() {
        assert_eq!(CommandAction::Help, CommandAction::Help);
        assert_ne!(CommandAction::Help, CommandAction::Quit);
    }
}
