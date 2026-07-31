//! Behavioural contract every [`SessionStorage`] backend must satisfy.
//!
//! The file and Postgres backends had silently diverged: the caller passes the entire
//! conversation on each iteration, which the file store handled by truncating and
//! rewriting, while the Postgres store appended — duplicating every message on every
//! iteration. Nothing tested them against a common contract, and CI never compiled the
//! Postgres one at all.
//!
//! These helpers are generic over the backend so both can be driven through the same
//! assertions from their own test modules.

#![cfg(test)]

use crate::llm::types::{ContentBlock, Message, Role};

use super::traits::SessionStorage;

pub fn message(role: Role, text: &str) -> Message {
    Message {
        role,
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
    }
}

/// Saving the same conversation repeatedly — what the agent loop does every iteration —
/// must leave exactly one copy of it.
pub async fn assert_save_messages_is_idempotent<S: SessionStorage>(store: &S) {
    let metadata = store.create("task").await.expect("create session");

    let conversation = vec![
        message(Role::User, "do the thing"),
        message(Role::Assistant, "working on it"),
    ];

    // Three iterations, each re-sending the whole conversation.
    for _ in 0..3 {
        store
            .save_messages(&metadata.id, &conversation)
            .await
            .expect("save messages");
    }

    let snapshot = store.load(&metadata.id).await.expect("load session");
    assert_eq!(
        snapshot.messages.len(),
        conversation.len(),
        "backend duplicated messages across repeated saves: got {} for a {}-message \
         conversation",
        snapshot.messages.len(),
        conversation.len()
    );
}

/// A shorter list must replace a longer one, since compaction rewrites history in place
/// and the persisted log has to mirror the live conversation.
pub async fn assert_save_messages_replaces_history<S: SessionStorage>(store: &S) {
    let metadata = store.create("task").await.expect("create session");

    let before = vec![
        message(Role::User, "first"),
        message(Role::Assistant, "second"),
        message(Role::User, "third"),
        message(Role::Assistant, "fourth"),
    ];
    store
        .save_messages(&metadata.id, &before)
        .await
        .expect("save messages");

    // Compaction collapses the history to a summary plus the most recent turn.
    let after = vec![
        message(Role::User, "<summary>earlier work</summary>"),
        message(Role::Assistant, "fourth"),
    ];
    store
        .save_messages(&metadata.id, &after)
        .await
        .expect("save compacted messages");

    let snapshot = store.load(&metadata.id).await.expect("load session");
    assert_eq!(
        snapshot.messages.len(),
        after.len(),
        "compacted history did not replace the stored log"
    );

    let first_text = match &snapshot.messages[0].content[0] {
        ContentBlock::Text { text } => text.clone(),
        other => panic!("unexpected block: {other:?}"),
    };
    assert!(
        first_text.contains("<summary>"),
        "stale pre-compaction messages survived: {first_text}"
    );
}

/// Steps are also re-sent whole each iteration and must not accumulate.
pub async fn assert_save_steps_is_idempotent<S: SessionStorage>(store: &S) {
    use crate::output::result::StepRecord;

    let metadata = store.create("task").await.expect("create session");

    let steps = vec![StepRecord {
        iteration: 1,
        tool: "bash".to_string(),
        input: serde_json::json!({"command": "ls"}),
        output: serde_json::Value::String("ok".to_string()),
        duration_ms: 5,
        is_error: None,
    }];

    for _ in 0..3 {
        store
            .save_steps(&metadata.id, &steps)
            .await
            .expect("save steps");
    }

    let snapshot = store.load(&metadata.id).await.expect("load session");
    assert_eq!(
        snapshot.steps.len(),
        steps.len(),
        "backend duplicated steps across repeated saves"
    );
}
