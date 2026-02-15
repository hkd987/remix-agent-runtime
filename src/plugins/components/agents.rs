use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::warn;

use crate::error::AgentError;

/// Parsed agent definition from a markdown file with YAML frontmatter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDefinition {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub tools: Vec<String>,
}

/// Full agent entry with metadata and prompt body.
#[derive(Debug, Clone)]
pub struct AgentEntry {
    pub definition: AgentDefinition,
    pub body: String,
    pub source_path: PathBuf,
}

/// Parse an agent markdown file with YAML frontmatter.
///
/// Expects YAML frontmatter delimited by `---` fences at the start,
/// followed by a markdown body.
///
/// Format:
/// ```text
/// ---
/// name: researcher
/// description: Searches the web for information
/// model: claude-sonnet-4-20250514
/// tools:
///   - web_search
///   - read_file
/// ---
/// # Researcher Agent
///
/// You are a research specialist...
/// ```
pub fn parse_agent_md(content: &str) -> Result<(AgentDefinition, String), AgentError> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return Err(AgentError::Plugin(
            "Agent file must start with YAML frontmatter (---)".to_string(),
        ));
    }

    // Find the closing --- fence (skip the opening one)
    let after_open = &trimmed[3..];
    let close_idx = after_open.find("\n---").ok_or_else(|| {
        AgentError::Plugin("Agent file missing closing frontmatter fence (---)".to_string())
    })?;

    let yaml_content = &after_open[..close_idx];
    let body_start = close_idx + 4; // skip "\n---"
    let body = if body_start < after_open.len() {
        after_open[body_start..]
            .trim_start_matches('\n')
            .to_string()
    } else {
        String::new()
    };

    let definition: AgentDefinition = serde_yaml::from_str(yaml_content)
        .map_err(|e| AgentError::Plugin(format!("Invalid agent frontmatter YAML: {e}")))?;

    // Validate name
    if definition.name.is_empty() {
        return Err(AgentError::Plugin(
            "Agent name must not be empty".to_string(),
        ));
    }

    // Validate description
    if definition.description.is_empty() {
        return Err(AgentError::Plugin(
            "Agent description must not be empty".to_string(),
        ));
    }

    Ok((definition, body))
}

/// Discover agent definition files from a list of file paths.
///
/// Each path should point to an `*.md` file. Invalid files are warned and skipped.
pub fn discover_agents_in_files(agent_files: &[&PathBuf]) -> Vec<AgentEntry> {
    let mut agents = Vec::new();

    for file_path in agent_files {
        let content = match std::fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(e) => {
                warn!(
                    path = %file_path.display(),
                    error = %e,
                    "Failed to read agent file, skipping"
                );
                continue;
            }
        };

        match parse_agent_md(&content) {
            Ok((definition, body)) => {
                agents.push(AgentEntry {
                    definition,
                    body,
                    source_path: file_path.to_path_buf(),
                });
            }
            Err(e) => {
                warn!(
                    path = %file_path.display(),
                    error = %e,
                    "Invalid agent file, skipping"
                );
            }
        }
    }

    agents
}

/// Generate system prompt content describing available agents.
///
/// Returns `None` if no agents are available.
pub fn inject_agents_into_system_prompt(agents: &[AgentEntry]) -> Option<String> {
    if agents.is_empty() {
        return None;
    }

    let mut lines = vec!["<available_agents>".to_string()];
    for agent in agents {
        lines.push(format!(
            "<agent name=\"{}\">{}</agent>",
            agent.definition.name, agent.definition.description
        ));
    }
    lines.push("</available_agents>".to_string());
    Some(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn valid_agent_md() -> String {
        r#"---
name: researcher
description: Searches the web for information
model: claude-sonnet-4-20250514
tools:
  - web_search
  - read_file
---
# Researcher Agent

You are a research specialist.
"#
        .to_string()
    }

    fn minimal_agent_md() -> String {
        r#"---
name: helper
description: A helpful assistant
---
Body text here.
"#
        .to_string()
    }

    // --- parse_agent_md ---

    #[test]
    fn test_parse_agent_md_valid() {
        let (def, body) = parse_agent_md(&valid_agent_md()).unwrap();
        assert_eq!(def.name, "researcher");
        assert_eq!(def.description, "Searches the web for information");
        assert_eq!(def.model, Some("claude-sonnet-4-20250514".to_string()));
        assert_eq!(def.tools, vec!["web_search", "read_file"]);
        assert!(body.contains("# Researcher Agent"));
        assert!(body.contains("You are a research specialist."));
    }

    #[test]
    fn test_parse_agent_md_minimal() {
        let (def, body) = parse_agent_md(&minimal_agent_md()).unwrap();
        assert_eq!(def.name, "helper");
        assert_eq!(def.description, "A helpful assistant");
        assert!(def.model.is_none());
        assert!(def.tools.is_empty());
        assert_eq!(body.trim(), "Body text here.");
    }

    #[test]
    fn test_parse_agent_md_missing_frontmatter() {
        let result = parse_agent_md("# No frontmatter\nJust text.");
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("frontmatter"));
    }

    #[test]
    fn test_parse_agent_md_missing_closing_fence() {
        let content = "---\nname: test\ndescription: test\n# No closing fence";
        let result = parse_agent_md(content);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("closing"));
    }

    #[test]
    fn test_parse_agent_md_empty_name() {
        let content = "---\nname: \"\"\ndescription: some desc\n---\nBody";
        let result = parse_agent_md(content);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("name must not be empty"));
    }

    #[test]
    fn test_parse_agent_md_empty_description() {
        let content = "---\nname: test\ndescription: \"\"\n---\nBody";
        let result = parse_agent_md(content);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("description must not be empty"));
    }

    #[test]
    fn test_parse_agent_md_bad_yaml() {
        let content = "---\n{{invalid yaml}}\n---\nBody";
        let result = parse_agent_md(content);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("YAML"));
    }

    #[test]
    fn test_parse_agent_md_empty_body() {
        let content = "---\nname: test\ndescription: test agent\n---";
        let (def, body) = parse_agent_md(content).unwrap();
        assert_eq!(def.name, "test");
        assert!(body.is_empty());
    }

    #[test]
    fn test_parse_agent_md_with_leading_whitespace() {
        let content = "  \n---\nname: test\ndescription: test agent\n---\nBody";
        let (def, body) = parse_agent_md(content).unwrap();
        assert_eq!(def.name, "test");
        assert_eq!(body.trim(), "Body");
    }

    #[test]
    fn test_parse_agent_md_no_model_defaults_none() {
        let content = "---\nname: simple\ndescription: simple agent\n---\nBody";
        let (def, _body) = parse_agent_md(content).unwrap();
        assert!(def.model.is_none());
    }

    #[test]
    fn test_parse_agent_md_no_tools_defaults_empty() {
        let content = "---\nname: simple\ndescription: simple agent\n---\nBody";
        let (def, _body) = parse_agent_md(content).unwrap();
        assert!(def.tools.is_empty());
    }

    // --- discover_agents_in_files ---

    #[test]
    fn test_discover_agents_valid_files() {
        let dir = TempDir::new().unwrap();
        let file1 = dir.path().join("agent1.md");
        let file2 = dir.path().join("agent2.md");
        fs::write(&file1, valid_agent_md()).unwrap();
        fs::write(&file2, minimal_agent_md()).unwrap();

        let files: Vec<PathBuf> = vec![file1, file2];
        let file_refs: Vec<&PathBuf> = files.iter().collect();
        let agents = discover_agents_in_files(&file_refs);

        assert_eq!(agents.len(), 2);
        assert_eq!(agents[0].definition.name, "researcher");
        assert_eq!(agents[1].definition.name, "helper");
    }

    #[test]
    fn test_discover_agents_mixed_valid_invalid() {
        let dir = TempDir::new().unwrap();

        let valid_file = dir.path().join("valid.md");
        fs::write(&valid_file, valid_agent_md()).unwrap();

        let invalid_file = dir.path().join("invalid.md");
        fs::write(&invalid_file, "no frontmatter here").unwrap();

        let missing_file = dir.path().join("nonexistent.md");

        let files: Vec<PathBuf> = vec![valid_file, invalid_file, missing_file];
        let file_refs: Vec<&PathBuf> = files.iter().collect();
        let agents = discover_agents_in_files(&file_refs);

        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].definition.name, "researcher");
    }

    #[test]
    fn test_discover_agents_empty_list() {
        let agents = discover_agents_in_files(&[]);
        assert!(agents.is_empty());
    }

    #[test]
    fn test_discover_agents_all_invalid() {
        let dir = TempDir::new().unwrap();

        let bad1 = dir.path().join("bad1.md");
        fs::write(&bad1, "no frontmatter").unwrap();

        let bad2 = dir.path().join("bad2.md");
        fs::write(&bad2, "---\n{{bad yaml}}\n---\nBody").unwrap();

        let files: Vec<PathBuf> = vec![bad1, bad2];
        let file_refs: Vec<&PathBuf> = files.iter().collect();
        let agents = discover_agents_in_files(&file_refs);

        assert!(agents.is_empty());
    }

    #[test]
    fn test_discover_agents_preserves_source_path() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("my-agent.md");
        fs::write(&file, minimal_agent_md()).unwrap();

        let files: Vec<PathBuf> = vec![file.clone()];
        let file_refs: Vec<&PathBuf> = files.iter().collect();
        let agents = discover_agents_in_files(&file_refs);

        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].source_path, file);
    }

    // --- inject_agents_into_system_prompt ---

    #[test]
    fn test_inject_agents_empty_list() {
        assert!(inject_agents_into_system_prompt(&[]).is_none());
    }

    #[test]
    fn test_inject_agents_single() {
        let agents = vec![AgentEntry {
            definition: AgentDefinition {
                name: "researcher".to_string(),
                description: "Searches the web".to_string(),
                model: None,
                tools: vec![],
            },
            body: "Body".to_string(),
            source_path: PathBuf::from("/tmp/researcher.md"),
        }];

        let prompt = inject_agents_into_system_prompt(&agents).unwrap();
        assert!(prompt.contains("<available_agents>"));
        assert!(prompt.contains("</available_agents>"));
        assert!(prompt.contains("<agent name=\"researcher\">Searches the web</agent>"));
    }

    #[test]
    fn test_inject_agents_multiple() {
        let agents = vec![
            AgentEntry {
                definition: AgentDefinition {
                    name: "researcher".to_string(),
                    description: "Searches the web".to_string(),
                    model: None,
                    tools: vec![],
                },
                body: String::new(),
                source_path: PathBuf::from("/tmp/researcher.md"),
            },
            AgentEntry {
                definition: AgentDefinition {
                    name: "coder".to_string(),
                    description: "Writes code".to_string(),
                    model: Some("claude-sonnet-4-20250514".to_string()),
                    tools: vec!["write_file".to_string()],
                },
                body: String::new(),
                source_path: PathBuf::from("/tmp/coder.md"),
            },
        ];

        let prompt = inject_agents_into_system_prompt(&agents).unwrap();
        assert!(prompt.contains("<agent name=\"researcher\">Searches the web</agent>"));
        assert!(prompt.contains("<agent name=\"coder\">Writes code</agent>"));

        // Verify ordering matches input order
        let researcher_pos = prompt.find("researcher").unwrap();
        let coder_pos = prompt.find("coder").unwrap();
        assert!(researcher_pos < coder_pos);
    }

    // --- AgentDefinition serde ---

    #[test]
    fn test_agent_definition_serde_roundtrip() {
        let def = AgentDefinition {
            name: "test-agent".to_string(),
            description: "A test agent".to_string(),
            model: Some("claude-sonnet-4-20250514".to_string()),
            tools: vec!["read_file".to_string(), "write_file".to_string()],
        };

        let json = serde_json::to_string(&def).unwrap();
        let deserialized: AgentDefinition = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.name, "test-agent");
        assert_eq!(deserialized.description, "A test agent");
        assert_eq!(
            deserialized.model,
            Some("claude-sonnet-4-20250514".to_string())
        );
        assert_eq!(deserialized.tools, vec!["read_file", "write_file"]);
    }

    #[test]
    fn test_agent_definition_defaults() {
        let json = r#"{"name":"minimal","description":"minimal agent"}"#;
        let def: AgentDefinition = serde_json::from_str(json).unwrap();
        assert!(def.model.is_none());
        assert!(def.tools.is_empty());
    }
}
