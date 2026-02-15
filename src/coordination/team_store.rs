use std::fs;
use std::path::{Path, PathBuf};

use tokio::sync::RwLock;

use crate::error::AgentError;

use super::team_types::{MemberStatus, TeamConfig, TeamCreateParams, TeamId, TeamMember};

pub struct TeamStore {
    team: RwLock<Option<TeamConfig>>,
    storage_dir: PathBuf,
}

impl std::fmt::Debug for TeamStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TeamStore")
            .field("storage_dir", &self.storage_dir)
            .finish()
    }
}

impl TeamStore {
    pub fn new(storage_dir: PathBuf) -> Self {
        let _ = fs::create_dir_all(&storage_dir);
        let existing = Self::load_from_disk(&storage_dir).unwrap_or(None);
        Self {
            team: RwLock::new(existing),
            storage_dir,
        }
    }

    pub async fn create(&self, params: TeamCreateParams) -> Result<TeamConfig, AgentError> {
        let mut guard = self.team.write().await;
        if guard.is_some() {
            return Err(AgentError::ToolExecution("Team already exists".to_string()));
        }

        let slug = params
            .team_name
            .to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect::<String>();
        // Collapse consecutive dashes and trim leading/trailing dashes
        let slug = slug
            .split('-')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("-");

        let config = TeamConfig {
            id: TeamId(slug),
            name: params.team_name,
            description: params.description,
            members: vec![],
            created_at: chrono::Utc::now(),
        };

        *guard = Some(config.clone());
        drop(guard);
        self.persist().await?;
        Ok(config)
    }

    pub async fn get(&self) -> Option<TeamConfig> {
        self.team.read().await.clone()
    }

    pub async fn add_member(&self, member: TeamMember) -> Result<(), AgentError> {
        let mut guard = self.team.write().await;
        let team = guard
            .as_mut()
            .ok_or_else(|| AgentError::ToolExecution("No team exists".to_string()))?;

        if team.members.iter().any(|m| m.name == member.name) {
            return Err(AgentError::ToolExecution(format!(
                "Member '{}' already exists",
                member.name
            )));
        }

        team.members.push(member);
        drop(guard);
        self.persist().await?;
        Ok(())
    }

    pub async fn remove_member(&self, name: &str) -> Result<(), AgentError> {
        let mut guard = self.team.write().await;
        let team = guard
            .as_mut()
            .ok_or_else(|| AgentError::ToolExecution("No team exists".to_string()))?;

        let idx = team
            .members
            .iter()
            .position(|m| m.name == name)
            .ok_or_else(|| AgentError::ToolExecution(format!("Member '{}' not found", name)))?;

        team.members.remove(idx);
        drop(guard);
        self.persist().await?;
        Ok(())
    }

    pub async fn update_member_status(
        &self,
        name: &str,
        status: MemberStatus,
    ) -> Result<(), AgentError> {
        let mut guard = self.team.write().await;
        let team = guard
            .as_mut()
            .ok_or_else(|| AgentError::ToolExecution("No team exists".to_string()))?;

        let member = team
            .members
            .iter_mut()
            .find(|m| m.name == name)
            .ok_or_else(|| AgentError::ToolExecution(format!("Member '{}' not found", name)))?;

        member.status = status;
        drop(guard);
        self.persist().await?;
        Ok(())
    }

    pub async fn persist(&self) -> Result<(), AgentError> {
        let guard = self.team.read().await;
        if let Some(config) = guard.as_ref() {
            let json = serde_json::to_string_pretty(config)?;
            let tmp_path = self.storage_dir.join("team.json.tmp");
            let final_path = self.storage_dir.join("team.json");
            fs::write(&tmp_path, &json)?;
            fs::rename(&tmp_path, &final_path)?;
        }
        Ok(())
    }

    pub fn load_from_disk(storage_dir: &Path) -> Result<Option<TeamConfig>, AgentError> {
        let path = storage_dir.join("team.json");
        if !path.exists() {
            return Ok(None);
        }
        let content = fs::read_to_string(&path)?;
        let config: TeamConfig = serde_json::from_str(&content)?;
        Ok(Some(config))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_params(name: &str) -> TeamCreateParams {
        TeamCreateParams {
            team_name: name.to_string(),
            description: Some(format!("{} description", name)),
        }
    }

    fn make_member(name: &str) -> TeamMember {
        TeamMember {
            name: name.to_string(),
            agent_type: None,
            status: MemberStatus::Active,
        }
    }

    #[tokio::test]
    async fn test_create_team() {
        let tmp = TempDir::new().unwrap();
        let store = TeamStore::new(tmp.path().to_path_buf());

        let config = store.create(make_params("My Team")).await.unwrap();
        assert_eq!(config.id, TeamId("my-team".to_string()));
        assert_eq!(config.name, "My Team");
        assert_eq!(config.description, Some("My Team description".to_string()));
        assert!(config.members.is_empty());
    }

    #[tokio::test]
    async fn test_create_team_slug_special_chars() {
        let tmp = TempDir::new().unwrap();
        let store = TeamStore::new(tmp.path().to_path_buf());

        let config = store
            .create(make_params("Hello World!! Test"))
            .await
            .unwrap();
        assert_eq!(config.id, TeamId("hello-world-test".to_string()));
    }

    #[tokio::test]
    async fn test_get_returns_none_when_empty() {
        let tmp = TempDir::new().unwrap();
        let store = TeamStore::new(tmp.path().to_path_buf());
        assert!(store.get().await.is_none());
    }

    #[tokio::test]
    async fn test_get_returns_team() {
        let tmp = TempDir::new().unwrap();
        let store = TeamStore::new(tmp.path().to_path_buf());
        store.create(make_params("Alpha")).await.unwrap();

        let team = store.get().await.unwrap();
        assert_eq!(team.name, "Alpha");
    }

    #[tokio::test]
    async fn test_create_team_already_exists_errors() {
        let tmp = TempDir::new().unwrap();
        let store = TeamStore::new(tmp.path().to_path_buf());

        store.create(make_params("Team A")).await.unwrap();
        let result = store.create(make_params("Team B")).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already exists"));
    }

    #[tokio::test]
    async fn test_add_member() {
        let tmp = TempDir::new().unwrap();
        let store = TeamStore::new(tmp.path().to_path_buf());
        store.create(make_params("Team")).await.unwrap();

        store.add_member(make_member("worker-1")).await.unwrap();
        let team = store.get().await.unwrap();
        assert_eq!(team.members.len(), 1);
        assert_eq!(team.members[0].name, "worker-1");
    }

    #[tokio::test]
    async fn test_add_member_no_team_errors() {
        let tmp = TempDir::new().unwrap();
        let store = TeamStore::new(tmp.path().to_path_buf());

        let result = store.add_member(make_member("worker-1")).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No team exists"));
    }

    #[tokio::test]
    async fn test_add_duplicate_member_errors() {
        let tmp = TempDir::new().unwrap();
        let store = TeamStore::new(tmp.path().to_path_buf());
        store.create(make_params("Team")).await.unwrap();

        store.add_member(make_member("worker-1")).await.unwrap();
        let result = store.add_member(make_member("worker-1")).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already exists"));
    }

    #[tokio::test]
    async fn test_remove_member() {
        let tmp = TempDir::new().unwrap();
        let store = TeamStore::new(tmp.path().to_path_buf());
        store.create(make_params("Team")).await.unwrap();

        store.add_member(make_member("worker-1")).await.unwrap();
        store.add_member(make_member("worker-2")).await.unwrap();
        store.remove_member("worker-1").await.unwrap();

        let team = store.get().await.unwrap();
        assert_eq!(team.members.len(), 1);
        assert_eq!(team.members[0].name, "worker-2");
    }

    #[tokio::test]
    async fn test_remove_nonexistent_member_errors() {
        let tmp = TempDir::new().unwrap();
        let store = TeamStore::new(tmp.path().to_path_buf());
        store.create(make_params("Team")).await.unwrap();

        let result = store.remove_member("ghost").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_update_member_status() {
        let tmp = TempDir::new().unwrap();
        let store = TeamStore::new(tmp.path().to_path_buf());
        store.create(make_params("Team")).await.unwrap();
        store.add_member(make_member("worker-1")).await.unwrap();

        store
            .update_member_status("worker-1", MemberStatus::Idle)
            .await
            .unwrap();
        let team = store.get().await.unwrap();
        assert_eq!(team.members[0].status, MemberStatus::Idle);
    }

    #[tokio::test]
    async fn test_update_status_nonexistent_member_errors() {
        let tmp = TempDir::new().unwrap();
        let store = TeamStore::new(tmp.path().to_path_buf());
        store.create(make_params("Team")).await.unwrap();

        let result = store
            .update_member_status("ghost", MemberStatus::ShutDown)
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_persist_and_reload() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().to_path_buf();

        // Create and populate
        {
            let store = TeamStore::new(dir.clone());
            store.create(make_params("Persist Team")).await.unwrap();
            store.add_member(make_member("worker-1")).await.unwrap();
        }

        // Reload from disk
        let store2 = TeamStore::new(dir);
        let team = store2.get().await.unwrap();
        assert_eq!(team.name, "Persist Team");
        assert_eq!(team.members.len(), 1);
        assert_eq!(team.members[0].name, "worker-1");
    }

    #[tokio::test]
    async fn test_load_from_disk_no_file() {
        let tmp = TempDir::new().unwrap();
        let result = TeamStore::load_from_disk(tmp.path()).unwrap();
        assert!(result.is_none());
    }
}
