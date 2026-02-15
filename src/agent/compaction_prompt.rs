/// System prompt used to ask the LLM to summarize conversation history.
pub const COMPACTION_SYSTEM_PROMPT: &str = r#"You are a conversation summarizer. Your task is to create a concise summary of the conversation history provided below.

Rules:
- Preserve all important facts, decisions, and outcomes
- Preserve any URLs, file paths, code snippets, or specific values mentioned
- Preserve the chronological order of key events
- Remove redundant or repetitive information
- Keep the summary under 2000 tokens
- Focus on what was accomplished and any pending tasks
- Do NOT include tool calls or tool results verbatim — summarize their outcomes instead

Provide the summary as plain text."#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compaction_system_prompt_is_not_empty() {
        assert!(!COMPACTION_SYSTEM_PROMPT.is_empty());
    }

    #[test]
    fn test_compaction_system_prompt_contains_key_instructions() {
        assert!(COMPACTION_SYSTEM_PROMPT.contains("summarizer"));
        assert!(COMPACTION_SYSTEM_PROMPT.contains("Preserve"));
        assert!(COMPACTION_SYSTEM_PROMPT.contains("2000 tokens"));
        assert!(COMPACTION_SYSTEM_PROMPT.contains("plain text"));
    }
}
