//! Conversation invariants required by the Anthropic Messages API.
//!
//! The agent loop mutates the message list from many places — nudges, goal checks,
//! self-critique, reminder injection, and four compaction stages. Each of those can
//! independently break a structural rule the API enforces, and the failure surfaces
//! only as an opaque HTTP 400 at the next request.
//!
//! [`validate_conversation`] checks those rules locally so a break is caught at the
//! mutation site instead of one network round trip later.

use crate::llm::types::{ContentBlock, Message, Role};

/// Validate the structural invariants the Messages API **rejects** a request for.
///
/// Checks:
/// 1. No message has empty content.
/// 2. Every `tool_use` block is answered by a `tool_result` with the same id in the
///    immediately following user message.
/// 3. No `tool_result` refers to a `tool_use` that was never issued.
///
/// Deliberately **not** checked here: strict user/assistant alternation. The API
/// accepts consecutive same-role messages and merges them, which the loop relies on —
/// `AgentState::inject_system_notification` appends a user message after a
/// tool-result user message on every reminder, loop-detection warning, and inbox
/// delivery, and those paths have run against the live API. See
/// [`check_role_alternation`] for that as a separate hygiene check.
///
/// Returns a human-readable description of the first violation found.
pub fn validate_conversation(messages: &[Message]) -> Result<(), String> {
    if messages.is_empty() {
        return Ok(());
    }

    for (i, msg) in messages.iter().enumerate() {
        if msg.content.is_empty() {
            return Err(format!(
                "message {i} ({:?}) has empty content; the API rejects empty content arrays",
                msg.role
            ));
        }
    }

    for (i, msg) in messages.iter().enumerate() {
        let pending = tool_use_ids(msg);
        if pending.is_empty() {
            continue;
        }

        let Some(next) = messages.get(i + 1) else {
            return Err(format!(
                "message {i} issues tool_use {pending:?} but is the last message; \
                 every tool_use must be answered by a tool_result"
            ));
        };

        if !matches!(next.role, Role::User) {
            return Err(format!(
                "message {i} issues tool_use {pending:?} but message {} is {:?}, not `user`",
                i + 1,
                next.role
            ));
        }

        let answered = tool_result_ids(next);
        for id in &pending {
            if !answered.contains(id) {
                return Err(format!(
                    "orphaned tool_use `{id}` in message {i}: message {} contains no matching \
                     tool_result (ids present: {answered:?})",
                    i + 1
                ));
            }
        }
    }

    for (i, msg) in messages.iter().enumerate() {
        let results = tool_result_ids(msg);
        if results.is_empty() {
            continue;
        }
        let issued = i
            .checked_sub(1)
            .map(|prev| tool_use_ids(&messages[prev]))
            .unwrap_or_default();
        for id in &results {
            if !issued.contains(id) {
                return Err(format!(
                    "dangling tool_result `{id}` in message {i}: no preceding tool_use with \
                     that id (ids issued: {issued:?})"
                ));
            }
        }
    }

    Ok(())
}

/// Hygiene check: report consecutive same-role messages.
///
/// Not fatal — the API merges them — but a run of user messages usually means several
/// notifications were injected back to back without the model getting a turn in
/// between, which dilutes each one. Useful for diagnostics, not for gating requests.
pub fn check_role_alternation(messages: &[Message]) -> Result<(), String> {
    for (i, pair) in messages.windows(2).enumerate() {
        let same = match (&pair[0].role, &pair[1].role) {
            (Role::User, Role::User) => Some("user"),
            (Role::Assistant, Role::Assistant) => Some("assistant"),
            _ => None,
        };
        if let Some(role) = same {
            return Err(format!(
                "messages {i} and {} are both `{role}`; they will be merged by the API",
                i + 1
            ));
        }
    }
    Ok(())
}

fn tool_use_ids(msg: &Message) -> Vec<String> {
    msg.content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolUse { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect()
}

fn tool_result_ids(msg: &Message) -> Vec<String> {
    msg.content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.clone()),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::types::ToolResultContent;
    use serde_json::json;

    fn text(role: Role, s: &str) -> Message {
        Message {
            role,
            content: vec![ContentBlock::Text {
                text: s.to_string(),
            }],
        }
    }

    fn tool_use(id: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: id.to_string(),
                name: "bash".to_string(),
                input: json!({"command": "ls"}),
            }],
        }
    }

    fn tool_result(id: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: id.to_string(),
                content: ToolResultContent::Text("ok".to_string()),
                is_error: None,
            }],
        }
    }

    #[test]
    fn empty_conversation_is_valid() {
        assert!(validate_conversation(&[]).is_ok());
    }

    #[test]
    fn simple_alternating_conversation_is_valid() {
        let msgs = vec![text(Role::User, "task"), text(Role::Assistant, "done")];
        assert!(validate_conversation(&msgs).is_ok());
    }

    #[test]
    fn paired_tool_use_and_result_is_valid() {
        let msgs = vec![
            text(Role::User, "task"),
            tool_use("t1"),
            tool_result("t1"),
            text(Role::Assistant, "done"),
        ];
        assert!(validate_conversation(&msgs).is_ok());
    }

    #[test]
    fn orphaned_tool_use_is_rejected() {
        // This is exactly the shape self-critique rejection produces: an assistant
        // message carrying tool_use followed by a plain user text message.
        let msgs = vec![
            text(Role::User, "task"),
            tool_use("t1"),
            text(Role::User, "[SELF_CRITIQUE] rejected"),
        ];
        let err = validate_conversation(&msgs).unwrap_err();
        assert!(err.contains("orphaned tool_use `t1`"), "got: {err}");
    }

    #[test]
    fn tool_use_as_last_message_is_rejected() {
        let msgs = vec![text(Role::User, "task"), tool_use("t1")];
        let err = validate_conversation(&msgs).unwrap_err();
        assert!(err.contains("last message"), "got: {err}");
    }

    #[test]
    fn partially_answered_tool_use_is_rejected() {
        let msgs = vec![
            text(Role::User, "task"),
            Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::ToolUse {
                        id: "t1".into(),
                        name: "bash".into(),
                        input: json!({}),
                    },
                    ContentBlock::ToolUse {
                        id: "t2".into(),
                        name: "bash".into(),
                        input: json!({}),
                    },
                ],
            },
            tool_result("t1"),
        ];
        let err = validate_conversation(&msgs).unwrap_err();
        assert!(err.contains("orphaned tool_use `t2`"), "got: {err}");
    }

    #[test]
    fn dangling_tool_result_is_rejected() {
        // Assistant message carries no tool_use, so the following tool_result has
        // nothing to answer. Roles still alternate, isolating the dangling check.
        let msgs = vec![
            text(Role::User, "task"),
            text(Role::Assistant, "thinking out loud"),
            tool_result("nope"),
        ];
        let err = validate_conversation(&msgs).unwrap_err();
        assert!(err.contains("dangling tool_result `nope`"), "got: {err}");
    }

    #[test]
    fn consecutive_user_messages_are_not_fatal() {
        // The loop injects notifications as user messages after tool-result user
        // messages; the API merges them, so this must not be treated as a violation.
        let msgs = vec![text(Role::User, "a"), text(Role::User, "b")];
        assert!(validate_conversation(&msgs).is_ok());
    }

    #[test]
    fn alternation_check_flags_consecutive_user_messages() {
        let msgs = vec![text(Role::User, "a"), text(Role::User, "b")];
        let err = check_role_alternation(&msgs).unwrap_err();
        assert!(err.contains("both `user`"), "got: {err}");
    }

    #[test]
    fn alternation_check_flags_consecutive_assistant_messages() {
        let msgs = vec![
            text(Role::User, "a"),
            text(Role::Assistant, "b"),
            text(Role::Assistant, "c"),
        ];
        let err = check_role_alternation(&msgs).unwrap_err();
        assert!(err.contains("both `assistant`"), "got: {err}");
    }

    #[test]
    fn alternation_check_passes_on_alternating_conversation() {
        let msgs = vec![
            text(Role::User, "a"),
            text(Role::Assistant, "b"),
            text(Role::User, "c"),
        ];
        assert!(check_role_alternation(&msgs).is_ok());
    }

    #[test]
    fn empty_content_is_rejected() {
        let msgs = vec![Message {
            role: Role::User,
            content: vec![],
        }];
        let err = validate_conversation(&msgs).unwrap_err();
        assert!(err.contains("empty content"), "got: {err}");
    }

    #[test]
    fn thinking_blocks_do_not_affect_validation() {
        let msgs = vec![
            text(Role::User, "task"),
            Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::Thinking {
                        thinking: "hmm".into(),
                        signature: "sig".into(),
                    },
                    ContentBlock::ToolUse {
                        id: "t1".into(),
                        name: "bash".into(),
                        input: json!({}),
                    },
                ],
            },
            tool_result("t1"),
        ];
        assert!(validate_conversation(&msgs).is_ok());
    }
}
