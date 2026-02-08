use std::path::{Path, PathBuf};
use tracing::warn;

use crate::error::AgentError;

use super::types::{validate_skill_name, SkillEntry, SkillMetadata, SkillSet};

/// Parse a SKILL.md file content into metadata and body.
///
/// Expects YAML frontmatter delimited by `---` fences at the start,
/// followed by a markdown body.
pub fn parse_skill_md(content: &str) -> Result<(SkillMetadata, String), AgentError> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return Err(AgentError::Skill(
            "SKILL.md must start with YAML frontmatter (---)".to_string(),
        ));
    }

    // Find the closing --- fence (skip the opening one)
    let after_open = &trimmed[3..];
    let close_idx = after_open.find("\n---").ok_or_else(|| {
        AgentError::Skill("SKILL.md missing closing frontmatter fence (---)".to_string())
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

    let metadata: SkillMetadata = serde_yaml::from_str(yaml_content)
        .map_err(|e| AgentError::Skill(format!("Invalid SKILL.md frontmatter YAML: {e}")))?;

    // Validate name
    validate_skill_name(&metadata.name)
        .map_err(|e| AgentError::Skill(format!("Invalid skill name '{}': {e}", metadata.name)))?;

    // Validate description
    if metadata.description.is_empty() {
        return Err(AgentError::Skill(
            "Skill description must not be empty".to_string(),
        ));
    }
    if metadata.description.len() > 1024 {
        return Err(AgentError::Skill(format!(
            "Skill description too long ({} chars, max 1024)",
            metadata.description.len()
        )));
    }

    Ok((metadata, body))
}

/// Scan a single directory for skill subdirectories containing SKILL.md files.
///
/// Only immediate subdirectories are scanned. Invalid skills are warned and skipped.
pub fn discover_skills_in_dir(dir: &Path) -> Result<Vec<SkillEntry>, AgentError> {
    let entries = std::fs::read_dir(dir).map_err(|e| {
        AgentError::Skill(format!(
            "Failed to read skills directory '{}': {e}",
            dir.display()
        ))
    })?;

    let mut skills = Vec::new();

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                warn!(error = %e, dir = %dir.display(), "Failed to read directory entry");
                continue;
            }
        };

        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let skill_md_path = path.join("SKILL.md");
        if !skill_md_path.exists() {
            continue;
        }

        let content = match std::fs::read_to_string(&skill_md_path) {
            Ok(c) => c,
            Err(e) => {
                warn!(
                    path = %skill_md_path.display(),
                    error = %e,
                    "Failed to read SKILL.md, skipping"
                );
                continue;
            }
        };

        match parse_skill_md(&content) {
            Ok((metadata, body)) => {
                let dir_path = match std::fs::canonicalize(&path) {
                    Ok(p) => p,
                    Err(e) => {
                        warn!(
                            path = %path.display(),
                            error = %e,
                            "Failed to canonicalize skill directory, skipping"
                        );
                        continue;
                    }
                };
                let skill_md_abs = dir_path.join("SKILL.md");
                skills.push(SkillEntry {
                    metadata,
                    body,
                    dir_path,
                    skill_md_path: skill_md_abs,
                });
            }
            Err(e) => {
                warn!(
                    path = %skill_md_path.display(),
                    error = %e,
                    "Invalid SKILL.md, skipping"
                );
            }
        }
    }

    Ok(skills)
}

/// Build the list of directories to scan for skills.
///
/// Merges extra dirs (from CLI/config) with default locations:
/// - `./skills/` (relative to current dir)
/// - `~/.remix/skills/` (user home)
pub fn default_skill_dirs(extra_dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = extra_dirs.to_vec();

    // Add ./skills/ relative to current working directory
    dirs.push(PathBuf::from("./skills"));

    // Add ~/.remix/skills/
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(home).join(".remix").join("skills"));
    }

    dirs
}

/// Discover all skills across multiple directories.
///
/// Scans each directory, skipping those that don't exist.
/// First-found-wins on name collisions.
pub fn discover_all_skills(dirs: &[PathBuf]) -> Result<SkillSet, AgentError> {
    let mut skill_set = SkillSet::new();

    for dir in dirs {
        if !dir.exists() {
            continue;
        }

        let entries = discover_skills_in_dir(dir)?;
        for entry in entries {
            if let Err(msg) = skill_set.insert(entry) {
                warn!("{msg}");
            }
        }
    }

    Ok(skill_set)
}

/// Generate system prompt content describing available skills.
///
/// Returns `None` if no skills are available.
pub fn inject_skills_into_system_prompt(skill_set: &SkillSet) -> Option<String> {
    if skill_set.is_empty() {
        return None;
    }

    let mut lines = vec!["<available_skills>".to_string()];

    for name in skill_set.sorted_names() {
        if let Some(entry) = skill_set.get(name) {
            lines.push(format!(
                "<skill name=\"{}\">{}</skill>",
                entry.metadata.name, entry.metadata.description
            ));
        }
    }

    lines.push("</available_skills>".to_string());
    lines.push(String::new());
    lines.push(
        "Use the `load_skill` tool with a skill name to load its full instructions before using it."
            .to_string(),
    );

    Some(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn valid_skill_md() -> String {
        r#"---
name: test-skill
description: A test skill for unit testing
license: MIT
compatibility:
  - remix-agent
---
# Test Skill

This is the body of the test skill.
Use it for testing purposes.
"#
        .to_string()
    }

    fn minimal_skill_md() -> String {
        r#"---
name: minimal
description: Minimal skill
---
Body text here.
"#
        .to_string()
    }

    #[test]
    fn test_parse_skill_md_valid() {
        let (metadata, body) = parse_skill_md(&valid_skill_md()).unwrap();
        assert_eq!(metadata.name, "test-skill");
        assert_eq!(metadata.description, "A test skill for unit testing");
        assert_eq!(metadata.license, Some("MIT".to_string()));
        assert_eq!(metadata.compatibility, vec!["remix-agent".to_string()]);
        assert!(body.contains("# Test Skill"));
        assert!(body.contains("Use it for testing purposes."));
    }

    #[test]
    fn test_parse_skill_md_minimal() {
        let (metadata, body) = parse_skill_md(&minimal_skill_md()).unwrap();
        assert_eq!(metadata.name, "minimal");
        assert_eq!(metadata.description, "Minimal skill");
        assert!(metadata.license.is_none());
        assert!(metadata.compatibility.is_empty());
        assert_eq!(body.trim(), "Body text here.");
    }

    #[test]
    fn test_parse_skill_md_missing_frontmatter() {
        let result = parse_skill_md("# No frontmatter\nJust text.");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("frontmatter"));
    }

    #[test]
    fn test_parse_skill_md_missing_closing_fence() {
        let content = "---\nname: test\ndescription: test\n# No closing fence";
        let result = parse_skill_md(content);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("closing"));
    }

    #[test]
    fn test_parse_skill_md_empty_body() {
        let content = "---\nname: test\ndescription: test\n---";
        let (metadata, body) = parse_skill_md(content).unwrap();
        assert_eq!(metadata.name, "test");
        assert!(body.is_empty());
    }

    #[test]
    fn test_parse_skill_md_bad_yaml() {
        let content = "---\n{{invalid yaml}}\n---\nBody";
        let result = parse_skill_md(content);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("YAML"));
    }

    #[test]
    fn test_parse_skill_md_invalid_name() {
        let content = "---\nname: Invalid_Name\ndescription: test\n---\nBody";
        let result = parse_skill_md(content);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid skill name"));
    }

    #[test]
    fn test_parse_skill_md_name_too_long() {
        let long_name = "a".repeat(65);
        let content = format!("---\nname: {long_name}\ndescription: test\n---\nBody");
        let result = parse_skill_md(&content);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_skill_md_empty_description() {
        let content = "---\nname: test\ndescription: \"\"\n---\nBody";
        let result = parse_skill_md(content);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("description must not be empty"));
    }

    #[test]
    fn test_parse_skill_md_description_too_long() {
        let long_desc = "a".repeat(1025);
        let content = format!("---\nname: test\ndescription: \"{long_desc}\"\n---\nBody");
        let result = parse_skill_md(&content);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("too long"));
    }

    #[test]
    fn test_discover_skills_in_dir_valid() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join("my-skill");
        fs::create_dir(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), valid_skill_md()).unwrap();

        let skills = discover_skills_in_dir(dir.path()).unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].metadata.name, "test-skill");
        assert!(skills[0].dir_path.is_absolute());
    }

    #[test]
    fn test_discover_skills_in_dir_mixed() {
        let dir = TempDir::new().unwrap();

        // Valid skill
        let valid_dir = dir.path().join("valid-skill");
        fs::create_dir(&valid_dir).unwrap();
        fs::write(valid_dir.join("SKILL.md"), minimal_skill_md()).unwrap();

        // Invalid skill (bad frontmatter)
        let invalid_dir = dir.path().join("bad-skill");
        fs::create_dir(&invalid_dir).unwrap();
        fs::write(invalid_dir.join("SKILL.md"), "not valid").unwrap();

        // Directory without SKILL.md
        let empty_dir = dir.path().join("no-skill");
        fs::create_dir(&empty_dir).unwrap();

        // File (not a directory)
        fs::write(dir.path().join("readme.txt"), "hello").unwrap();

        let skills = discover_skills_in_dir(dir.path()).unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].metadata.name, "minimal");
    }

    #[test]
    fn test_discover_skills_in_dir_empty() {
        let dir = TempDir::new().unwrap();
        let skills = discover_skills_in_dir(dir.path()).unwrap();
        assert!(skills.is_empty());
    }

    #[test]
    fn test_discover_skills_in_dir_nonexistent() {
        let result = discover_skills_in_dir(Path::new("/nonexistent/path"));
        assert!(result.is_err());
    }

    #[test]
    fn test_default_skill_dirs_with_extra() {
        let extras = vec![PathBuf::from("/custom/skills")];
        let dirs = default_skill_dirs(&extras);
        assert!(dirs.len() >= 2);
        assert_eq!(dirs[0], PathBuf::from("/custom/skills"));
        assert_eq!(dirs[1], PathBuf::from("./skills"));
    }

    #[test]
    fn test_default_skill_dirs_without_extra() {
        let dirs = default_skill_dirs(&[]);
        assert!(!dirs.is_empty());
        assert_eq!(dirs[0], PathBuf::from("./skills"));
    }

    #[test]
    fn test_discover_all_skills_multi_dir() {
        let dir1 = TempDir::new().unwrap();
        let dir2 = TempDir::new().unwrap();

        let skill1_dir = dir1.path().join("skill-a");
        fs::create_dir(&skill1_dir).unwrap();
        fs::write(
            skill1_dir.join("SKILL.md"),
            "---\nname: skill-a\ndescription: First skill\n---\nBody A",
        )
        .unwrap();

        let skill2_dir = dir2.path().join("skill-b");
        fs::create_dir(&skill2_dir).unwrap();
        fs::write(
            skill2_dir.join("SKILL.md"),
            "---\nname: skill-b\ndescription: Second skill\n---\nBody B",
        )
        .unwrap();

        let dirs = vec![dir1.path().to_path_buf(), dir2.path().to_path_buf()];
        let skill_set = discover_all_skills(&dirs).unwrap();
        assert_eq!(skill_set.len(), 2);
        assert!(skill_set.get("skill-a").is_some());
        assert!(skill_set.get("skill-b").is_some());
    }

    #[test]
    fn test_discover_all_skills_missing_dirs_skipped() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join("my-skill");
        fs::create_dir(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), minimal_skill_md()).unwrap();

        let dirs = vec![PathBuf::from("/nonexistent/path"), dir.path().to_path_buf()];
        let skill_set = discover_all_skills(&dirs).unwrap();
        assert_eq!(skill_set.len(), 1);
    }

    #[test]
    fn test_discover_all_skills_duplicate_first_wins() {
        let dir1 = TempDir::new().unwrap();
        let dir2 = TempDir::new().unwrap();

        let skill1_dir = dir1.path().join("my-skill");
        fs::create_dir(&skill1_dir).unwrap();
        fs::write(
            skill1_dir.join("SKILL.md"),
            "---\nname: dupe\ndescription: First version\n---\nFirst body",
        )
        .unwrap();

        let skill2_dir = dir2.path().join("dupe-skill");
        fs::create_dir(&skill2_dir).unwrap();
        fs::write(
            skill2_dir.join("SKILL.md"),
            "---\nname: dupe\ndescription: Second version\n---\nSecond body",
        )
        .unwrap();

        let dirs = vec![dir1.path().to_path_buf(), dir2.path().to_path_buf()];
        let skill_set = discover_all_skills(&dirs).unwrap();
        assert_eq!(skill_set.len(), 1);
        assert_eq!(
            skill_set.get("dupe").unwrap().metadata.description,
            "First version"
        );
    }

    #[test]
    fn test_inject_skills_empty_set() {
        let skill_set = SkillSet::new();
        assert!(inject_skills_into_system_prompt(&skill_set).is_none());
    }

    #[test]
    fn test_inject_skills_single() {
        let mut skill_set = SkillSet::new();
        skill_set
            .insert(SkillEntry {
                metadata: SkillMetadata {
                    name: "test-skill".to_string(),
                    description: "A test skill".to_string(),
                    license: None,
                    compatibility: vec![],
                    metadata: Default::default(),
                    allowed_tools: vec![],
                },
                body: "Body".to_string(),
                dir_path: PathBuf::from("/tmp/test-skill"),
                skill_md_path: PathBuf::from("/tmp/test-skill/SKILL.md"),
            })
            .unwrap();

        let prompt = inject_skills_into_system_prompt(&skill_set).unwrap();
        assert!(prompt.contains("<available_skills>"));
        assert!(prompt.contains("</available_skills>"));
        assert!(prompt.contains("<skill name=\"test-skill\">A test skill</skill>"));
        assert!(prompt.contains("load_skill"));
    }

    #[test]
    fn test_inject_skills_multiple_sorted() {
        let mut skill_set = SkillSet::new();
        for name in &["zebra-skill", "alpha-skill", "middle-skill"] {
            skill_set
                .insert(SkillEntry {
                    metadata: SkillMetadata {
                        name: name.to_string(),
                        description: format!("Desc for {name}"),
                        license: None,
                        compatibility: vec![],
                        metadata: Default::default(),
                        allowed_tools: vec![],
                    },
                    body: String::new(),
                    dir_path: PathBuf::from(format!("/tmp/{name}")),
                    skill_md_path: PathBuf::from(format!("/tmp/{name}/SKILL.md")),
                })
                .unwrap();
        }

        let prompt = inject_skills_into_system_prompt(&skill_set).unwrap();
        let alpha_pos = prompt.find("alpha-skill").unwrap();
        let middle_pos = prompt.find("middle-skill").unwrap();
        let zebra_pos = prompt.find("zebra-skill").unwrap();
        assert!(alpha_pos < middle_pos);
        assert!(middle_pos < zebra_pos);
    }

    #[test]
    fn test_parse_skill_md_with_metadata_and_allowed_tools() {
        let content = r#"---
name: advanced-skill
description: An advanced skill
metadata:
  author: test
  version: "1.0"
allowed_tools:
  - navigate
  - click
---
# Advanced

Instructions here.
"#;
        let (metadata, _body) = parse_skill_md(content).unwrap();
        assert_eq!(metadata.metadata.get("author").unwrap(), "test");
        assert_eq!(metadata.metadata.get("version").unwrap(), "1.0");
        assert_eq!(metadata.allowed_tools, vec!["navigate", "click"]);
    }
}
