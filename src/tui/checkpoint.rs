use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Tools that modify files and should trigger checkpoint creation.
const FILE_MODIFYING_TOOLS: &[&str] = &["write_file", "edit_file", "bash"];

/// A snapshot of file state before a modifying tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub id: usize,
    pub turn_id: u32,
    pub tool_name: String,
    pub files_affected: Vec<String>,
    pub stash_ref: Option<String>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Manages git-backed file snapshots for undo/rewind.
pub struct CheckpointManager {
    checkpoints: Vec<Checkpoint>,
    session_dir: Option<PathBuf>,
    next_id: usize,
}

impl CheckpointManager {
    /// Create a new checkpoint manager, optionally persisting to a session directory.
    pub fn new(session_dir: Option<PathBuf>) -> Self {
        Self {
            checkpoints: Vec::new(),
            session_dir,
            next_id: 0,
        }
    }

    /// Create a checkpoint before a file-modifying operation.
    /// Uses `git stash create` to capture working tree state without modifying HEAD.
    pub async fn create_checkpoint(
        &mut self,
        turn_id: u32,
        tool_name: &str,
        files: &[String],
    ) -> Result<Checkpoint, std::io::Error> {
        let stash_ref = Self::git_stash_create().await?;

        let checkpoint = Checkpoint {
            id: self.next_id,
            turn_id,
            tool_name: tool_name.to_string(),
            files_affected: files.to_vec(),
            stash_ref,
            timestamp: chrono::Utc::now(),
        };

        self.next_id += 1;
        self.checkpoints.push(checkpoint.clone());
        Ok(checkpoint)
    }

    /// Restore to the most recent checkpoint (undo last edit).
    /// Pops the last checkpoint and applies its stash.
    pub async fn undo(&mut self) -> Result<Option<Checkpoint>, std::io::Error> {
        let checkpoint = match self.checkpoints.pop() {
            Some(cp) => cp,
            None => return Ok(None),
        };

        if let Some(ref stash_ref) = checkpoint.stash_ref {
            Self::git_stash_apply(stash_ref).await?;
        }

        Ok(Some(checkpoint))
    }

    /// List all checkpoints for /rewind display.
    pub fn list(&self) -> &[Checkpoint] {
        &self.checkpoints
    }

    /// Restore to a specific checkpoint by ID.
    /// Removes all checkpoints after the target and applies its stash.
    pub async fn restore(
        &mut self,
        checkpoint_id: usize,
    ) -> Result<Option<Checkpoint>, std::io::Error> {
        let pos = match self
            .checkpoints
            .iter()
            .position(|cp| cp.id == checkpoint_id)
        {
            Some(p) => p,
            None => return Ok(None),
        };

        let checkpoint = self.checkpoints[pos].clone();

        // Remove all checkpoints after this one (inclusive of later ones)
        self.checkpoints.truncate(pos + 1);

        if let Some(ref stash_ref) = checkpoint.stash_ref {
            Self::git_stash_apply(stash_ref).await?;
        }

        Ok(Some(checkpoint))
    }

    /// Check if a tool name is file-modifying.
    pub fn is_file_modifying(tool_name: &str) -> bool {
        FILE_MODIFYING_TOOLS.contains(&tool_name)
    }

    /// Save checkpoints metadata to disk.
    pub fn save(&self) -> Result<(), std::io::Error> {
        let dir = match &self.session_dir {
            Some(d) => d,
            None => return Ok(()),
        };
        std::fs::create_dir_all(dir)?;
        let path = dir.join("checkpoints.json");
        let json = serde_json::to_string_pretty(&self.checkpoints)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Load checkpoints from disk.
    pub fn load(session_dir: &std::path::Path) -> Result<Self, std::io::Error> {
        let path = session_dir.join("checkpoints.json");
        if !path.exists() {
            return Ok(Self::new(Some(session_dir.to_path_buf())));
        }
        let data = std::fs::read_to_string(&path)?;
        let checkpoints: Vec<Checkpoint> = serde_json::from_str(&data)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        let next_id = checkpoints.iter().map(|cp| cp.id + 1).max().unwrap_or(0);
        Ok(Self {
            checkpoints,
            session_dir: Some(session_dir.to_path_buf()),
            next_id,
        })
    }

    /// Run `git stash create` and return the stash ref if there are changes.
    async fn git_stash_create() -> Result<Option<String>, std::io::Error> {
        let output = tokio::process::Command::new("git")
            .args(["stash", "create"])
            .output()
            .await?;

        let stdout = crate::local_tools::tools::output_filter::strip_ansi(
            &String::from_utf8_lossy(&output.stdout),
        )
        .trim()
        .to_string();
        if stdout.is_empty() {
            Ok(None)
        } else {
            Ok(Some(stdout))
        }
    }

    /// Run `git stash apply <ref>`.
    async fn git_stash_apply(stash_ref: &str) -> Result<(), std::io::Error> {
        let output = tokio::process::Command::new("git")
            .args(["stash", "apply", stash_ref])
            .output()
            .await?;

        if !output.status.success() {
            let stderr = crate::local_tools::tools::output_filter::strip_ansi(
                &String::from_utf8_lossy(&output.stderr),
            );
            return Err(std::io::Error::other(format!(
                "git stash apply failed: {stderr}"
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_file_modifying_write_file() {
        assert!(CheckpointManager::is_file_modifying("write_file"));
    }

    #[test]
    fn test_is_file_modifying_edit_file() {
        assert!(CheckpointManager::is_file_modifying("edit_file"));
    }

    #[test]
    fn test_is_file_modifying_bash() {
        assert!(CheckpointManager::is_file_modifying("bash"));
    }

    #[test]
    fn test_is_not_file_modifying_read_file() {
        assert!(!CheckpointManager::is_file_modifying("read_file"));
    }

    #[test]
    fn test_is_not_file_modifying_grep() {
        assert!(!CheckpointManager::is_file_modifying("grep"));
    }

    #[test]
    fn test_is_not_file_modifying_glob() {
        assert!(!CheckpointManager::is_file_modifying("glob"));
    }

    #[test]
    fn test_is_not_file_modifying_navigate() {
        assert!(!CheckpointManager::is_file_modifying("navigate"));
    }

    #[test]
    fn test_new_empty() {
        let mgr = CheckpointManager::new(None);
        assert!(mgr.list().is_empty());
    }

    #[test]
    fn test_new_with_session_dir() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = CheckpointManager::new(Some(dir.path().to_path_buf()));
        assert!(mgr.list().is_empty());
        assert!(mgr.session_dir.is_some());
    }

    #[tokio::test]
    async fn test_create_checkpoint_in_git_repo() {
        let dir = tempfile::tempdir().unwrap();
        // Initialize a git repo
        let status = tokio::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();
        assert!(status.status.success());

        // Configure git user for the test repo
        tokio::process::Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();
        tokio::process::Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();

        // Create initial commit
        std::fs::write(dir.path().join("file.txt"), "initial").unwrap();
        tokio::process::Command::new("git")
            .args(["add", "."])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();
        tokio::process::Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();

        let mut mgr = CheckpointManager::new(Some(dir.path().to_path_buf()));
        let cp = mgr
            .create_checkpoint(1, "write_file", &["file.txt".to_string()])
            .await
            .unwrap();

        assert_eq!(cp.id, 0);
        assert_eq!(cp.turn_id, 1);
        assert_eq!(cp.tool_name, "write_file");
        assert_eq!(cp.files_affected, vec!["file.txt"]);
        assert_eq!(mgr.list().len(), 1);
    }

    #[tokio::test]
    async fn test_undo_empty() {
        let mut mgr = CheckpointManager::new(None);
        let result = mgr.undo().await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_restore_nonexistent_id() {
        let mut mgr = CheckpointManager::new(None);
        let result = mgr.restore(999).await.unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_save_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let session_dir = dir.path().to_path_buf();

        let mut mgr = CheckpointManager::new(Some(session_dir.clone()));
        mgr.checkpoints.push(Checkpoint {
            id: 0,
            turn_id: 1,
            tool_name: "write_file".to_string(),
            files_affected: vec!["src/main.rs".to_string()],
            stash_ref: Some("abc123".to_string()),
            timestamp: chrono::Utc::now(),
        });
        mgr.checkpoints.push(Checkpoint {
            id: 1,
            turn_id: 2,
            tool_name: "edit_file".to_string(),
            files_affected: vec!["src/lib.rs".to_string()],
            stash_ref: None,
            timestamp: chrono::Utc::now(),
        });
        mgr.next_id = 2;
        mgr.save().unwrap();

        let loaded = CheckpointManager::load(&session_dir).unwrap();
        assert_eq!(loaded.checkpoints.len(), 2);
        assert_eq!(loaded.checkpoints[0].tool_name, "write_file");
        assert_eq!(loaded.checkpoints[1].tool_name, "edit_file");
        assert_eq!(loaded.next_id, 2);
    }

    #[test]
    fn test_save_no_session_dir_is_noop() {
        let mgr = CheckpointManager::new(None);
        mgr.save().unwrap(); // Should not error
    }

    #[test]
    fn test_load_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = CheckpointManager::load(&dir.path().to_path_buf()).unwrap();
        assert!(mgr.list().is_empty());
    }

    #[test]
    fn test_checkpoint_serde_roundtrip() {
        let cp = Checkpoint {
            id: 42,
            turn_id: 7,
            tool_name: "bash".to_string(),
            files_affected: vec!["a.txt".to_string(), "b.txt".to_string()],
            stash_ref: Some("deadbeef".to_string()),
            timestamp: chrono::Utc::now(),
        };
        let json = serde_json::to_string(&cp).unwrap();
        let back: Checkpoint = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, 42);
        assert_eq!(back.turn_id, 7);
        assert_eq!(back.tool_name, "bash");
        assert_eq!(back.files_affected.len(), 2);
        assert_eq!(back.stash_ref, Some("deadbeef".to_string()));
    }

    #[test]
    fn test_list_checkpoints() {
        let mut mgr = CheckpointManager::new(None);
        assert!(mgr.list().is_empty());

        mgr.checkpoints.push(Checkpoint {
            id: 0,
            turn_id: 1,
            tool_name: "write_file".to_string(),
            files_affected: vec![],
            stash_ref: None,
            timestamp: chrono::Utc::now(),
        });
        assert_eq!(mgr.list().len(), 1);
    }

    #[test]
    fn test_next_id_increments() {
        let mut mgr = CheckpointManager::new(None);
        assert_eq!(mgr.next_id, 0);

        mgr.checkpoints.push(Checkpoint {
            id: 0,
            turn_id: 1,
            tool_name: "write_file".to_string(),
            files_affected: vec![],
            stash_ref: None,
            timestamp: chrono::Utc::now(),
        });
        mgr.next_id = 1;

        mgr.checkpoints.push(Checkpoint {
            id: 1,
            turn_id: 2,
            tool_name: "edit_file".to_string(),
            files_affected: vec![],
            stash_ref: None,
            timestamp: chrono::Utc::now(),
        });
        mgr.next_id = 2;

        assert_eq!(mgr.next_id, 2);
        assert_eq!(mgr.checkpoints.len(), 2);
    }
}
