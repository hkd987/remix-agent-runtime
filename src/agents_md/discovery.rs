use std::path::{Path, PathBuf};

use crate::config::schema::AgentsMdConfig;
use crate::error::AgentError;

/// Content discovered from AGENTS.md files.
#[derive(Debug, Clone)]
pub struct AgentsMdContent {
    pub content: String,
    pub sources: Vec<PathBuf>,
}

/// Discover AGENTS.md files by walking from search_dir up to filesystem root.
/// Files are collected root-to-leaf order (general to specific).
/// Concatenated with "\n\n---\n\n" separators, enforcing max_size_bytes limit.
pub fn discover_agents_md(config: &AgentsMdConfig) -> Result<Option<AgentsMdContent>, AgentError> {
    if !config.enabled {
        return Ok(None);
    }

    let search_dir = match &config.search_dir {
        Some(dir) => dir.clone(),
        None => std::env::current_dir()
            .map_err(|e| AgentError::AgentsMd(format!("Failed to get current directory: {e}")))?,
    };

    let search_dir = search_dir
        .canonicalize()
        .map_err(|e| AgentError::AgentsMd(format!("Failed to canonicalize search dir: {e}")))?;

    // Walk up from search_dir to root, collecting AGENTS.md paths
    let mut agents_md_paths = Vec::new();
    let mut current: Option<&Path> = Some(search_dir.as_path());
    while let Some(dir) = current {
        let agents_md = dir.join("AGENTS.md");
        if agents_md.is_file() {
            agents_md_paths.push(agents_md);
        }
        current = dir.parent();
    }

    if agents_md_paths.is_empty() {
        return Ok(None);
    }

    // Reverse to get root-to-leaf order (general to specific)
    agents_md_paths.reverse();

    // Read and concatenate, enforcing size limit
    let mut content_parts = Vec::new();
    let mut total_size = 0usize;
    let mut sources = Vec::new();

    for path in &agents_md_paths {
        let file_content = std::fs::read_to_string(path)
            .map_err(|e| AgentError::AgentsMd(format!("Failed to read {}: {e}", path.display())))?;

        let separator_size = if content_parts.is_empty() { 0 } else { 5 }; // "\n\n---\n\n".len()
        let new_size = total_size + separator_size + file_content.len();

        if new_size > config.max_size_bytes {
            tracing::warn!(
                path = %path.display(),
                total_size,
                max = config.max_size_bytes,
                "Stopping AGENTS.md accumulation: would exceed size limit"
            );
            break;
        }

        total_size = new_size;
        content_parts.push(file_content);
        sources.push(path.clone());
    }

    if content_parts.is_empty() {
        return Ok(None);
    }

    Ok(Some(AgentsMdContent {
        content: content_parts.join("\n\n---\n\n"),
        sources,
    }))
}

/// Wrap AGENTS.md content in XML tags for system prompt injection.
pub fn inject_agents_md_into_system_prompt(agents_md: &Option<AgentsMdContent>) -> Option<String> {
    agents_md.as_ref().map(|md| {
        format!(
            "<project_instructions>\n{}\n</project_instructions>",
            md.content
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_discover_no_agents_md() {
        let tmp = TempDir::new().unwrap();
        let config = AgentsMdConfig {
            enabled: true,
            search_dir: Some(tmp.path().to_path_buf()),
            max_size_bytes: 32768,
        };
        let result = discover_agents_md(&config).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_discover_single_agents_md() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("AGENTS.md"), "# Project Rules\nBe helpful.").unwrap();
        let config = AgentsMdConfig {
            enabled: true,
            search_dir: Some(tmp.path().to_path_buf()),
            max_size_bytes: 32768,
        };
        let result = discover_agents_md(&config).unwrap().unwrap();
        assert_eq!(result.content, "# Project Rules\nBe helpful.");
        assert_eq!(result.sources.len(), 1);
    }

    #[test]
    fn test_discover_hierarchy() {
        let tmp = TempDir::new().unwrap();
        let parent = tmp.path();
        let child = parent.join("sub");
        fs::create_dir_all(&child).unwrap();

        fs::write(parent.join("AGENTS.md"), "parent rules").unwrap();
        fs::write(child.join("AGENTS.md"), "child rules").unwrap();

        let config = AgentsMdConfig {
            enabled: true,
            search_dir: Some(child.clone()),
            max_size_bytes: 32768,
        };
        let result = discover_agents_md(&config).unwrap().unwrap();
        // Root-to-leaf: parent first, then child
        assert!(result.content.starts_with("parent rules"));
        assert!(result.content.ends_with("child rules"));
        assert!(result.content.contains("\n\n---\n\n"));
        assert_eq!(result.sources.len(), 2);
    }

    #[test]
    fn test_discover_max_size_limit() {
        let tmp = TempDir::new().unwrap();
        let parent = tmp.path();
        let child = parent.join("sub");
        fs::create_dir_all(&child).unwrap();

        // Create content that will exceed a small limit
        let large_content = "x".repeat(100);
        fs::write(parent.join("AGENTS.md"), &large_content).unwrap();
        fs::write(child.join("AGENTS.md"), &large_content).unwrap();

        let config = AgentsMdConfig {
            enabled: true,
            search_dir: Some(child.clone()),
            // First file is 100 bytes, separator is 5, second would be 205 total
            max_size_bytes: 150,
        };
        let result = discover_agents_md(&config).unwrap().unwrap();
        // Only the parent file should be included (root-to-leaf, parent comes first)
        assert_eq!(result.content, large_content);
        assert_eq!(result.sources.len(), 1);
    }

    #[test]
    fn test_discover_disabled() {
        let config = AgentsMdConfig {
            enabled: false,
            search_dir: None,
            max_size_bytes: 32768,
        };
        let result = discover_agents_md(&config).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_discover_custom_search_dir() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("AGENTS.md"), "custom dir content").unwrap();

        let config = AgentsMdConfig {
            enabled: true,
            search_dir: Some(tmp.path().to_path_buf()),
            max_size_bytes: 32768,
        };
        let result = discover_agents_md(&config).unwrap().unwrap();
        assert_eq!(result.content, "custom dir content");
    }

    #[test]
    fn test_inject_none() {
        let result = inject_agents_md_into_system_prompt(&None);
        assert!(result.is_none());
    }

    #[test]
    fn test_inject_some() {
        let content = AgentsMdContent {
            content: "Be helpful.".to_string(),
            sources: vec![PathBuf::from("/project/AGENTS.md")],
        };
        let result = inject_agents_md_into_system_prompt(&Some(content)).unwrap();
        assert_eq!(
            result,
            "<project_instructions>\nBe helpful.\n</project_instructions>"
        );
    }

    #[test]
    fn test_inject_preserves_content() {
        let original = "# Rules\n\n- Rule 1\n- Rule 2\n\n## Section\nDetails here.";
        let content = AgentsMdContent {
            content: original.to_string(),
            sources: vec![PathBuf::from("/a/AGENTS.md")],
        };
        let result = inject_agents_md_into_system_prompt(&Some(content)).unwrap();
        assert!(result.contains(original));
        assert!(result.starts_with("<project_instructions>\n"));
        assert!(result.ends_with("\n</project_instructions>"));
    }

    #[test]
    fn test_discover_empty_agents_md() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("AGENTS.md"), "").unwrap();

        let config = AgentsMdConfig {
            enabled: true,
            search_dir: Some(tmp.path().to_path_buf()),
            max_size_bytes: 32768,
        };
        let result = discover_agents_md(&config).unwrap().unwrap();
        assert_eq!(result.content, "");
        assert_eq!(result.sources.len(), 1);
    }

    #[test]
    fn test_sources_contain_paths() {
        let tmp = TempDir::new().unwrap();
        let parent = tmp.path();
        let child = parent.join("sub");
        fs::create_dir_all(&child).unwrap();

        fs::write(parent.join("AGENTS.md"), "parent").unwrap();
        fs::write(child.join("AGENTS.md"), "child").unwrap();

        let config = AgentsMdConfig {
            enabled: true,
            search_dir: Some(child.clone()),
            max_size_bytes: 32768,
        };
        let result = discover_agents_md(&config).unwrap().unwrap();
        assert_eq!(result.sources.len(), 2);
        // Sources should be in root-to-leaf order
        let parent_canonical = parent.canonicalize().unwrap();
        let child_canonical = child.canonicalize().unwrap();
        assert_eq!(result.sources[0], parent_canonical.join("AGENTS.md"));
        assert_eq!(result.sources[1], child_canonical.join("AGENTS.md"));
    }

    #[test]
    fn test_root_to_leaf_order() {
        let tmp = TempDir::new().unwrap();
        let level1 = tmp.path();
        let level2 = level1.join("a");
        let level3 = level2.join("b");
        fs::create_dir_all(&level3).unwrap();

        fs::write(level1.join("AGENTS.md"), "L1").unwrap();
        fs::write(level2.join("AGENTS.md"), "L2").unwrap();
        fs::write(level3.join("AGENTS.md"), "L3").unwrap();

        let config = AgentsMdConfig {
            enabled: true,
            search_dir: Some(level3.clone()),
            max_size_bytes: 32768,
        };
        let result = discover_agents_md(&config).unwrap().unwrap();
        let parts: Vec<&str> = result.content.split("\n\n---\n\n").collect();
        assert_eq!(parts, vec!["L1", "L2", "L3"]);
        assert_eq!(result.sources.len(), 3);
    }
}
