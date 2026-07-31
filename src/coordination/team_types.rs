use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct TeamId(pub String);

impl fmt::Display for TeamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemberStatus {
    Active,
    Idle,
    ShutDown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMember {
    pub name: String,
    pub agent_type: Option<String>,
    pub status: MemberStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamConfig {
    pub id: TeamId,
    pub name: String,
    pub description: Option<String>,
    pub members: Vec<TeamMember>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamCreateParams {
    pub team_name: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_team_id_display() {
        let id = TeamId("my-team".to_string());
        assert_eq!(format!("{}", id), "my-team");
    }

    #[test]
    fn test_team_id_equality() {
        let id1 = TeamId("alpha".to_string());
        let id2 = TeamId("alpha".to_string());
        let id3 = TeamId("beta".to_string());
        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_team_id_hash() {
        let mut set = HashSet::new();
        set.insert(TeamId("alpha".to_string()));
        set.insert(TeamId("alpha".to_string()));
        assert_eq!(set.len(), 1);
        set.insert(TeamId("beta".to_string()));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn test_team_id_serde_roundtrip() {
        let id = TeamId("my-team-123".to_string());
        let json = serde_json::to_string(&id).unwrap();
        let deserialized: TeamId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, deserialized);
    }

    #[test]
    fn test_member_status_serde_roundtrip() {
        let statuses = vec![
            MemberStatus::Active,
            MemberStatus::Idle,
            MemberStatus::ShutDown,
        ];
        for status in statuses {
            let json = serde_json::to_string(&status).unwrap();
            let deserialized: MemberStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(status, deserialized);
        }
    }

    #[test]
    fn test_member_status_serde_snake_case() {
        assert_eq!(
            serde_json::to_string(&MemberStatus::Active).unwrap(),
            "\"active\""
        );
        assert_eq!(
            serde_json::to_string(&MemberStatus::Idle).unwrap(),
            "\"idle\""
        );
        assert_eq!(
            serde_json::to_string(&MemberStatus::ShutDown).unwrap(),
            "\"shut_down\""
        );
    }

    #[test]
    fn test_team_member_serde() {
        let member = TeamMember {
            name: "worker-1".to_string(),
            agent_type: Some("browser".to_string()),
            status: MemberStatus::Active,
        };
        let json = serde_json::to_string(&member).unwrap();
        let deserialized: TeamMember = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "worker-1");
        assert_eq!(deserialized.agent_type, Some("browser".to_string()));
        assert_eq!(deserialized.status, MemberStatus::Active);
    }

    #[test]
    fn test_team_member_serde_no_agent_type() {
        let member = TeamMember {
            name: "worker-2".to_string(),
            agent_type: None,
            status: MemberStatus::Idle,
        };
        let json = serde_json::to_string(&member).unwrap();
        let deserialized: TeamMember = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "worker-2");
        assert_eq!(deserialized.agent_type, None);
        assert_eq!(deserialized.status, MemberStatus::Idle);
    }

    #[test]
    fn test_team_config_serde_roundtrip() {
        let config = TeamConfig {
            id: TeamId("test-team".to_string()),
            name: "Test Team".to_string(),
            description: Some("A test team".to_string()),
            members: vec![
                TeamMember {
                    name: "lead".to_string(),
                    agent_type: Some("coordinator".to_string()),
                    status: MemberStatus::Active,
                },
                TeamMember {
                    name: "worker".to_string(),
                    agent_type: None,
                    status: MemberStatus::Idle,
                },
            ],
            created_at: Utc::now(),
        };
        let json = serde_json::to_string_pretty(&config).unwrap();
        let deserialized: TeamConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, config.id);
        assert_eq!(deserialized.name, config.name);
        assert_eq!(deserialized.description, config.description);
        assert_eq!(deserialized.members.len(), 2);
        assert_eq!(deserialized.members[0].name, "lead");
        assert_eq!(deserialized.members[1].name, "worker");
    }

    #[test]
    fn test_team_config_serde_no_description() {
        let config = TeamConfig {
            id: TeamId("minimal".to_string()),
            name: "Minimal".to_string(),
            description: None,
            members: vec![],
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: TeamConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.description, None);
        assert!(deserialized.members.is_empty());
    }

    #[test]
    fn test_team_create_params_serde() {
        let params = TeamCreateParams {
            team_name: "New Team".to_string(),
            description: Some("Description".to_string()),
        };
        let json = serde_json::to_string(&params).unwrap();
        let deserialized: TeamCreateParams = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.team_name, "New Team");
        assert_eq!(deserialized.description, Some("Description".to_string()));
    }

    #[test]
    fn test_team_create_params_serde_no_description() {
        let params = TeamCreateParams {
            team_name: "Bare Team".to_string(),
            description: None,
        };
        let json = serde_json::to_string(&params).unwrap();
        let deserialized: TeamCreateParams = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.team_name, "Bare Team");
        assert_eq!(deserialized.description, None);
    }
}
