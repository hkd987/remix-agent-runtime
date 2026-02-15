use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use tokio::sync::RwLock;

use crate::error::AgentError;

use super::inbox_types::InboxMessage;

pub struct InboxStore {
    inboxes: RwLock<HashMap<String, Vec<InboxMessage>>>,
    storage_dir: PathBuf,
}

impl std::fmt::Debug for InboxStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InboxStore")
            .field("storage_dir", &self.storage_dir)
            .finish()
    }
}

impl InboxStore {
    pub fn new(storage_dir: PathBuf) -> Self {
        let inbox_dir = storage_dir.join("inbox");
        let _ = fs::create_dir_all(&inbox_dir);
        let existing = Self::load_from_disk(&storage_dir).unwrap_or_default();
        Self {
            inboxes: RwLock::new(existing),
            storage_dir,
        }
    }

    pub async fn deliver(&self, recipient: &str, message: InboxMessage) -> Result<(), AgentError> {
        let mut guard = self.inboxes.write().await;
        guard
            .entry(recipient.to_string())
            .or_default()
            .push(message);
        Ok(())
    }

    pub async fn broadcast(
        &self,
        message: InboxMessage,
        recipients: &[String],
    ) -> Result<(), AgentError> {
        let mut guard = self.inboxes.write().await;
        for recipient in recipients {
            let mut msg = message.clone();
            msg.recipient = Some(recipient.clone());
            guard.entry(recipient.clone()).or_default().push(msg);
        }
        Ok(())
    }

    pub async fn drain(&self, agent_name: &str) -> Result<Vec<InboxMessage>, AgentError> {
        let mut guard = self.inboxes.write().await;
        let messages = guard.remove(agent_name).unwrap_or_default();
        Ok(messages)
    }

    pub async fn peek(&self, agent_name: &str) -> Vec<InboxMessage> {
        let guard = self.inboxes.read().await;
        guard.get(agent_name).cloned().unwrap_or_default()
    }

    pub async fn persist(&self) -> Result<(), AgentError> {
        let inbox_dir = self.storage_dir.join("inbox");
        let _ = fs::create_dir_all(&inbox_dir);

        let guard = self.inboxes.read().await;
        for (agent_name, messages) in guard.iter() {
            let tmp_path = inbox_dir.join(format!("{}.jsonl.tmp", agent_name));
            let final_path = inbox_dir.join(format!("{}.jsonl", agent_name));
            let mut file = fs::File::create(&tmp_path)?;
            for msg in messages {
                let line = serde_json::to_string(msg)?;
                writeln!(file, "{}", line)?;
            }
            fs::rename(&tmp_path, &final_path)?;
        }
        Ok(())
    }

    pub fn load_from_disk(
        storage_dir: &Path,
    ) -> Result<HashMap<String, Vec<InboxMessage>>, AgentError> {
        let inbox_dir = storage_dir.join("inbox");
        if !inbox_dir.exists() {
            return Ok(HashMap::new());
        }

        let mut inboxes = HashMap::new();
        for entry in fs::read_dir(&inbox_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let agent_name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            if agent_name.is_empty() {
                continue;
            }

            let file = fs::File::open(&path)?;
            let reader = BufReader::new(file);
            let mut messages = Vec::new();
            for line in reader.lines() {
                let line = line?;
                if line.trim().is_empty() {
                    continue;
                }
                let msg: InboxMessage = serde_json::from_str(&line)?;
                messages.push(msg);
            }
            if !messages.is_empty() {
                inboxes.insert(agent_name, messages);
            }
        }
        Ok(inboxes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordination::inbox_types::{MessageId, MessageType};
    use chrono::Utc;
    use tempfile::TempDir;

    fn make_message(sender: &str, content: &str) -> InboxMessage {
        InboxMessage {
            id: MessageId::new(),
            message_type: MessageType::Message,
            sender: sender.to_string(),
            recipient: None,
            content: content.to_string(),
            summary: Some("test".to_string()),
            timestamp: Utc::now(),
            request_id: None,
            approve: None,
        }
    }

    fn make_broadcast_message(sender: &str, content: &str) -> InboxMessage {
        InboxMessage {
            id: MessageId::new(),
            message_type: MessageType::Broadcast,
            sender: sender.to_string(),
            recipient: None,
            content: content.to_string(),
            summary: Some("broadcast".to_string()),
            timestamp: Utc::now(),
            request_id: None,
            approve: None,
        }
    }

    #[tokio::test]
    async fn test_deliver_and_drain_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let store = InboxStore::new(tmp.path().to_path_buf());

        let msg = make_message("agent-a", "Hello");
        store.deliver("agent-b", msg).await.unwrap();

        let messages = store.drain("agent-b").await.unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].sender, "agent-a");
        assert_eq!(messages[0].content, "Hello");
    }

    #[tokio::test]
    async fn test_deliver_multiple_messages() {
        let tmp = TempDir::new().unwrap();
        let store = InboxStore::new(tmp.path().to_path_buf());

        store
            .deliver("agent-b", make_message("agent-a", "First"))
            .await
            .unwrap();
        store
            .deliver("agent-b", make_message("agent-c", "Second"))
            .await
            .unwrap();

        let messages = store.drain("agent-b").await.unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].content, "First");
        assert_eq!(messages[1].content, "Second");
    }

    #[tokio::test]
    async fn test_drain_clears_inbox() {
        let tmp = TempDir::new().unwrap();
        let store = InboxStore::new(tmp.path().to_path_buf());

        store
            .deliver("agent-b", make_message("agent-a", "Hello"))
            .await
            .unwrap();

        let messages = store.drain("agent-b").await.unwrap();
        assert_eq!(messages.len(), 1);

        let messages = store.drain("agent-b").await.unwrap();
        assert!(messages.is_empty());
    }

    #[tokio::test]
    async fn test_drain_empty_returns_empty_vec() {
        let tmp = TempDir::new().unwrap();
        let store = InboxStore::new(tmp.path().to_path_buf());

        let messages = store.drain("nonexistent").await.unwrap();
        assert!(messages.is_empty());
    }

    #[tokio::test]
    async fn test_peek_returns_messages() {
        let tmp = TempDir::new().unwrap();
        let store = InboxStore::new(tmp.path().to_path_buf());

        store
            .deliver("agent-b", make_message("agent-a", "Peek me"))
            .await
            .unwrap();

        let messages = store.peek("agent-b").await;
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "Peek me");
    }

    #[tokio::test]
    async fn test_peek_does_not_clear() {
        let tmp = TempDir::new().unwrap();
        let store = InboxStore::new(tmp.path().to_path_buf());

        store
            .deliver("agent-b", make_message("agent-a", "Still here"))
            .await
            .unwrap();

        let first = store.peek("agent-b").await;
        assert_eq!(first.len(), 1);

        let second = store.peek("agent-b").await;
        assert_eq!(second.len(), 1);
    }

    #[tokio::test]
    async fn test_peek_empty_returns_empty_vec() {
        let tmp = TempDir::new().unwrap();
        let store = InboxStore::new(tmp.path().to_path_buf());

        let messages = store.peek("nonexistent").await;
        assert!(messages.is_empty());
    }

    #[tokio::test]
    async fn test_broadcast_delivers_to_all() {
        let tmp = TempDir::new().unwrap();
        let store = InboxStore::new(tmp.path().to_path_buf());

        let msg = make_broadcast_message("lead", "Team update");
        let recipients = vec![
            "worker-1".to_string(),
            "worker-2".to_string(),
            "worker-3".to_string(),
        ];
        store.broadcast(msg, &recipients).await.unwrap();

        for name in &recipients {
            let messages = store.drain(name).await.unwrap();
            assert_eq!(messages.len(), 1);
            assert_eq!(messages[0].content, "Team update");
            assert_eq!(messages[0].sender, "lead");
            assert_eq!(messages[0].recipient, Some(name.clone()));
        }
    }

    #[tokio::test]
    async fn test_broadcast_empty_recipients() {
        let tmp = TempDir::new().unwrap();
        let store = InboxStore::new(tmp.path().to_path_buf());

        let msg = make_broadcast_message("lead", "No one");
        store.broadcast(msg, &[]).await.unwrap();
        // Nothing should be stored
        let messages = store.drain("lead").await.unwrap();
        assert!(messages.is_empty());
    }

    #[tokio::test]
    async fn test_persist_and_reload() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().to_path_buf();

        // Create and populate
        {
            let store = InboxStore::new(dir.clone());
            store
                .deliver("agent-a", make_message("agent-b", "Persisted message"))
                .await
                .unwrap();
            store
                .deliver("agent-a", make_message("agent-c", "Another message"))
                .await
                .unwrap();
            store
                .deliver("agent-b", make_message("agent-a", "For B"))
                .await
                .unwrap();
            store.persist().await.unwrap();
        }

        // Reload from disk
        let store2 = InboxStore::new(dir);
        let messages_a = store2.drain("agent-a").await.unwrap();
        assert_eq!(messages_a.len(), 2);
        assert_eq!(messages_a[0].content, "Persisted message");
        assert_eq!(messages_a[1].content, "Another message");

        let messages_b = store2.drain("agent-b").await.unwrap();
        assert_eq!(messages_b.len(), 1);
        assert_eq!(messages_b[0].content, "For B");
    }

    #[tokio::test]
    async fn test_load_from_disk_no_inbox_dir() {
        let tmp = TempDir::new().unwrap();
        let result = InboxStore::load_from_disk(tmp.path()).unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_concurrent_delivery() {
        let tmp = TempDir::new().unwrap();
        let store = std::sync::Arc::new(InboxStore::new(tmp.path().to_path_buf()));

        let mut handles = Vec::new();
        for i in 0..10 {
            let store = store.clone();
            handles.push(tokio::spawn(async move {
                let msg = make_message(&format!("sender-{}", i), &format!("msg-{}", i));
                store.deliver("target", msg).await.unwrap();
            }));
        }

        for h in handles {
            h.await.unwrap();
        }

        let messages = store.drain("target").await.unwrap();
        assert_eq!(messages.len(), 10);
    }
}
