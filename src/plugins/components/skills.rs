use std::path::PathBuf;

use crate::error::AgentError;
use crate::plugins::types::PluginSet;
use crate::skills::discovery::discover_skills_in_dir;
use crate::skills::types::SkillSet;

/// Merge skills discovered from plugins into an existing SkillSet.
///
/// For each plugin, scans its `skills/` directories for SKILL.md files
/// and inserts them into the SkillSet. First-found-wins on name collisions
/// (existing skills take priority over plugin skills).
pub fn merge_plugin_skills(
    plugin_set: &PluginSet,
    skill_set: &mut SkillSet,
) -> Result<usize, AgentError> {
    let mut count = 0;

    for plugin in plugin_set.iter() {
        let skills_dir = plugin.root_path.join("skills");
        if !skills_dir.is_dir() {
            continue;
        }

        match discover_skills_in_dir(&skills_dir) {
            Ok(entries) => {
                for entry in entries {
                    match skill_set.insert(entry) {
                        Ok(()) => count += 1,
                        Err(msg) => {
                            tracing::warn!(
                                plugin = %plugin.name,
                                "{msg}"
                            );
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    plugin = %plugin.name,
                    error = %e,
                    "Failed to scan plugin skills directory"
                );
            }
        }
    }

    Ok(count)
}

/// Collect all skill directories from plugins for discovery.
/// Returns paths to each plugin's `skills/` directory that exists.
pub fn collect_plugin_skill_dirs(plugin_set: &PluginSet) -> Vec<PathBuf> {
    plugin_set
        .iter()
        .filter_map(|plugin| {
            let skills_dir = plugin.root_path.join("skills");
            if skills_dir.is_dir() {
                Some(skills_dir)
            } else {
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::types::{PluginSource, ResolvedPlugin};
    use std::collections::HashMap;
    use std::fs;
    use tempfile::TempDir;

    fn make_skill_md(name: &str, description: &str) -> String {
        format!(
            "---\nname: {name}\ndescription: {description}\n---\n# {name}\n\nInstructions for {name}.\n"
        )
    }

    fn make_plugin(name: &str, root_path: PathBuf) -> ResolvedPlugin {
        let components = crate::plugins::types::resolve_plugin_at_path(&root_path);
        ResolvedPlugin {
            name: name.to_string(),
            source: PluginSource::LocalDir {
                path: root_path.clone(),
            },
            root_path,
            components,
        }
    }

    // --- merge_plugin_skills ---

    #[test]
    fn test_merge_with_valid_plugin_containing_skills() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join("skills").join("login-flow");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            make_skill_md("login-flow", "Automates login"),
        )
        .unwrap();

        let mut plugin_set = PluginSet::new();
        plugin_set.push(make_plugin("my-plugin", dir.path().to_path_buf()));

        let mut skill_set = SkillSet::new();
        let count = merge_plugin_skills(&plugin_set, &mut skill_set).unwrap();

        assert_eq!(count, 1);
        assert_eq!(skill_set.len(), 1);
        assert!(skill_set.get("login-flow").is_some());
        assert_eq!(
            skill_set.get("login-flow").unwrap().metadata.description,
            "Automates login"
        );
    }

    #[test]
    fn test_merge_with_multiple_skills_in_one_plugin() {
        let dir = TempDir::new().unwrap();

        let skill_a = dir.path().join("skills").join("skill-a");
        fs::create_dir_all(&skill_a).unwrap();
        fs::write(
            skill_a.join("SKILL.md"),
            make_skill_md("skill-a", "First skill"),
        )
        .unwrap();

        let skill_b = dir.path().join("skills").join("skill-b");
        fs::create_dir_all(&skill_b).unwrap();
        fs::write(
            skill_b.join("SKILL.md"),
            make_skill_md("skill-b", "Second skill"),
        )
        .unwrap();

        let mut plugin_set = PluginSet::new();
        plugin_set.push(make_plugin("multi-skill", dir.path().to_path_buf()));

        let mut skill_set = SkillSet::new();
        let count = merge_plugin_skills(&plugin_set, &mut skill_set).unwrap();

        assert_eq!(count, 2);
        assert_eq!(skill_set.len(), 2);
        assert!(skill_set.get("skill-a").is_some());
        assert!(skill_set.get("skill-b").is_some());
    }

    #[test]
    fn test_merge_with_plugin_without_skills_dir() {
        let dir = TempDir::new().unwrap();
        // No skills/ directory created

        let mut plugin_set = PluginSet::new();
        plugin_set.push(make_plugin("no-skills", dir.path().to_path_buf()));

        let mut skill_set = SkillSet::new();
        let count = merge_plugin_skills(&plugin_set, &mut skill_set).unwrap();

        assert_eq!(count, 0);
        assert!(skill_set.is_empty());
    }

    #[test]
    fn test_merge_with_empty_skills_dir() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("skills")).unwrap();

        let mut plugin_set = PluginSet::new();
        plugin_set.push(make_plugin("empty-skills", dir.path().to_path_buf()));

        let mut skill_set = SkillSet::new();
        let count = merge_plugin_skills(&plugin_set, &mut skill_set).unwrap();

        assert_eq!(count, 0);
        assert!(skill_set.is_empty());
    }

    #[test]
    fn test_merge_duplicate_skill_name_existing_wins() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join("skills").join("dupe-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            make_skill_md("dupe-skill", "Plugin version"),
        )
        .unwrap();

        let mut plugin_set = PluginSet::new();
        plugin_set.push(make_plugin("p1", dir.path().to_path_buf()));

        // Pre-populate skill_set with a skill of the same name
        let mut skill_set = SkillSet::new();
        skill_set
            .insert(crate::skills::types::SkillEntry {
                metadata: crate::skills::types::SkillMetadata {
                    name: "dupe-skill".to_string(),
                    description: "Existing version".to_string(),
                    license: None,
                    compatibility: vec![],
                    metadata: HashMap::new(),
                    allowed_tools: vec![],
                },
                body: "Existing body".to_string(),
                dir_path: PathBuf::from("/existing/dupe-skill"),
                skill_md_path: PathBuf::from("/existing/dupe-skill/SKILL.md"),
            })
            .unwrap();

        let count = merge_plugin_skills(&plugin_set, &mut skill_set).unwrap();

        assert_eq!(count, 0);
        assert_eq!(skill_set.len(), 1);
        assert_eq!(
            skill_set.get("dupe-skill").unwrap().metadata.description,
            "Existing version"
        );
    }

    #[test]
    fn test_merge_duplicate_across_plugins_first_plugin_wins() {
        let dir1 = TempDir::new().unwrap();
        let skill1 = dir1.path().join("skills").join("shared-skill");
        fs::create_dir_all(&skill1).unwrap();
        fs::write(
            skill1.join("SKILL.md"),
            make_skill_md("shared-skill", "From plugin 1"),
        )
        .unwrap();

        let dir2 = TempDir::new().unwrap();
        let skill2 = dir2.path().join("skills").join("shared-skill");
        fs::create_dir_all(&skill2).unwrap();
        fs::write(
            skill2.join("SKILL.md"),
            make_skill_md("shared-skill", "From plugin 2"),
        )
        .unwrap();

        let mut plugin_set = PluginSet::new();
        plugin_set.push(make_plugin("p1", dir1.path().to_path_buf()));
        plugin_set.push(make_plugin("p2", dir2.path().to_path_buf()));

        let mut skill_set = SkillSet::new();
        let count = merge_plugin_skills(&plugin_set, &mut skill_set).unwrap();

        assert_eq!(count, 1);
        assert_eq!(skill_set.len(), 1);
        assert_eq!(
            skill_set.get("shared-skill").unwrap().metadata.description,
            "From plugin 1"
        );
    }

    #[test]
    fn test_merge_multiple_plugins_with_unique_skills() {
        let dir1 = TempDir::new().unwrap();
        let skill1 = dir1.path().join("skills").join("alpha");
        fs::create_dir_all(&skill1).unwrap();
        fs::write(
            skill1.join("SKILL.md"),
            make_skill_md("alpha", "Alpha skill"),
        )
        .unwrap();

        let dir2 = TempDir::new().unwrap();
        let skill2 = dir2.path().join("skills").join("beta");
        fs::create_dir_all(&skill2).unwrap();
        fs::write(skill2.join("SKILL.md"), make_skill_md("beta", "Beta skill")).unwrap();

        let mut plugin_set = PluginSet::new();
        plugin_set.push(make_plugin("p1", dir1.path().to_path_buf()));
        plugin_set.push(make_plugin("p2", dir2.path().to_path_buf()));

        let mut skill_set = SkillSet::new();
        let count = merge_plugin_skills(&plugin_set, &mut skill_set).unwrap();

        assert_eq!(count, 2);
        assert_eq!(skill_set.len(), 2);
        assert!(skill_set.get("alpha").is_some());
        assert!(skill_set.get("beta").is_some());
    }

    #[test]
    fn test_merge_with_empty_plugin_set() {
        let plugin_set = PluginSet::new();
        let mut skill_set = SkillSet::new();
        let count = merge_plugin_skills(&plugin_set, &mut skill_set).unwrap();

        assert_eq!(count, 0);
        assert!(skill_set.is_empty());
    }

    #[test]
    fn test_merge_skips_invalid_skill_md() {
        let dir = TempDir::new().unwrap();

        // Valid skill
        let valid = dir.path().join("skills").join("valid-skill");
        fs::create_dir_all(&valid).unwrap();
        fs::write(
            valid.join("SKILL.md"),
            make_skill_md("valid-skill", "A valid skill"),
        )
        .unwrap();

        // Invalid skill (bad frontmatter)
        let invalid = dir.path().join("skills").join("bad-skill");
        fs::create_dir_all(&invalid).unwrap();
        fs::write(invalid.join("SKILL.md"), "not valid frontmatter").unwrap();

        let mut plugin_set = PluginSet::new();
        plugin_set.push(make_plugin("mixed", dir.path().to_path_buf()));

        let mut skill_set = SkillSet::new();
        let count = merge_plugin_skills(&plugin_set, &mut skill_set).unwrap();

        assert_eq!(count, 1);
        assert!(skill_set.get("valid-skill").is_some());
    }

    // --- collect_plugin_skill_dirs ---

    #[test]
    fn test_collect_plugin_skill_dirs_with_mix() {
        let dir_with_skills = TempDir::new().unwrap();
        fs::create_dir_all(dir_with_skills.path().join("skills")).unwrap();

        let dir_without_skills = TempDir::new().unwrap();

        let mut plugin_set = PluginSet::new();
        plugin_set.push(make_plugin(
            "with-skills",
            dir_with_skills.path().to_path_buf(),
        ));
        plugin_set.push(make_plugin(
            "without-skills",
            dir_without_skills.path().to_path_buf(),
        ));

        let dirs = collect_plugin_skill_dirs(&plugin_set);

        assert_eq!(dirs.len(), 1);
        assert_eq!(dirs[0], dir_with_skills.path().join("skills"));
    }

    #[test]
    fn test_collect_plugin_skill_dirs_empty_set() {
        let plugin_set = PluginSet::new();
        let dirs = collect_plugin_skill_dirs(&plugin_set);
        assert!(dirs.is_empty());
    }

    #[test]
    fn test_collect_plugin_skill_dirs_all_have_skills() {
        let dir1 = TempDir::new().unwrap();
        fs::create_dir_all(dir1.path().join("skills")).unwrap();

        let dir2 = TempDir::new().unwrap();
        fs::create_dir_all(dir2.path().join("skills")).unwrap();

        let mut plugin_set = PluginSet::new();
        plugin_set.push(make_plugin("p1", dir1.path().to_path_buf()));
        plugin_set.push(make_plugin("p2", dir2.path().to_path_buf()));

        let dirs = collect_plugin_skill_dirs(&plugin_set);

        assert_eq!(dirs.len(), 2);
        assert_eq!(dirs[0], dir1.path().join("skills"));
        assert_eq!(dirs[1], dir2.path().join("skills"));
    }

    #[test]
    fn test_collect_plugin_skill_dirs_none_have_skills() {
        let dir1 = TempDir::new().unwrap();
        let dir2 = TempDir::new().unwrap();

        let mut plugin_set = PluginSet::new();
        plugin_set.push(make_plugin("p1", dir1.path().to_path_buf()));
        plugin_set.push(make_plugin("p2", dir2.path().to_path_buf()));

        let dirs = collect_plugin_skill_dirs(&plugin_set);
        assert!(dirs.is_empty());
    }
}
