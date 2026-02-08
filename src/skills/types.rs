use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Parsed YAML frontmatter from a skill.md file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMetadata {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub compatibility: Vec<String>,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
}

/// Full skill data: parsed metadata, prompt body, and filesystem paths.
#[derive(Debug, Clone)]
pub struct SkillEntry {
    pub metadata: SkillMetadata,
    pub body: String,
    pub dir_path: PathBuf,
    pub skill_md_path: PathBuf,
}

/// A collection of skills keyed by name. Enforces uniqueness on insert.
#[derive(Debug, Clone, Default)]
pub struct SkillSet {
    skills: HashMap<String, SkillEntry>,
}

impl SkillSet {
    pub fn new() -> Self {
        Self {
            skills: HashMap::new(),
        }
    }

    /// Insert a skill entry. Returns an error if a skill with the same name
    /// already exists.
    pub fn insert(&mut self, entry: SkillEntry) -> Result<(), String> {
        let name = entry.metadata.name.clone();
        if self.skills.contains_key(&name) {
            return Err(format!("duplicate skill name: '{}'", name));
        }
        self.skills.insert(name, entry);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&SkillEntry> {
        self.skills.get(name)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &SkillEntry)> {
        self.skills.iter()
    }

    /// Returns skill names in sorted (alphabetical) order.
    pub fn sorted_names(&self) -> Vec<&String> {
        let mut names: Vec<&String> = self.skills.keys().collect();
        names.sort();
        names
    }

    pub fn len(&self) -> usize {
        self.skills.len()
    }

    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }
}

/// Validate a skill name. Names must be 1-64 characters, containing only
/// lowercase `a-z`, digits `0-9`, and hyphens. No leading, trailing, or
/// consecutive hyphens are allowed.
pub fn validate_skill_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("skill name must not be empty".to_string());
    }
    if name.len() > 64 {
        return Err(format!(
            "skill name must be at most 64 characters, got {}",
            name.len()
        ));
    }
    if name.starts_with('-') {
        return Err("skill name must not start with a hyphen".to_string());
    }
    if name.ends_with('-') {
        return Err("skill name must not end with a hyphen".to_string());
    }
    if name.contains("--") {
        return Err("skill name must not contain consecutive hyphens".to_string());
    }
    for ch in name.chars() {
        if !ch.is_ascii_lowercase() && !ch.is_ascii_digit() && ch != '-' {
            return Err(format!(
                "skill name contains invalid character '{}'; only lowercase a-z, 0-9, and hyphens are allowed",
                ch
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(name: &str) -> SkillEntry {
        SkillEntry {
            metadata: SkillMetadata {
                name: name.to_string(),
                description: format!("Skill: {}", name),
                license: None,
                compatibility: vec![],
                metadata: HashMap::new(),
                allowed_tools: vec![],
            },
            body: format!("Body for {}", name),
            dir_path: PathBuf::from(format!("/skills/{}", name)),
            skill_md_path: PathBuf::from(format!("/skills/{}/skill.md", name)),
        }
    }

    // --- SkillSet basic operations ---

    #[test]
    fn test_skillset_new_is_empty() {
        let set = SkillSet::new();
        assert!(set.is_empty());
        assert_eq!(set.len(), 0);
    }

    #[test]
    fn test_skillset_insert_and_get() {
        let mut set = SkillSet::new();
        set.insert(make_entry("login-flow")).unwrap();
        assert_eq!(set.len(), 1);
        assert!(!set.is_empty());

        let entry = set.get("login-flow").unwrap();
        assert_eq!(entry.metadata.name, "login-flow");
        assert_eq!(entry.body, "Body for login-flow");
    }

    #[test]
    fn test_skillset_get_missing_returns_none() {
        let set = SkillSet::new();
        assert!(set.get("nonexistent").is_none());
    }

    #[test]
    fn test_skillset_duplicate_insert_errors() {
        let mut set = SkillSet::new();
        set.insert(make_entry("dup-skill")).unwrap();
        let result = set.insert(make_entry("dup-skill"));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("duplicate skill name"));
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn test_skillset_sorted_names() {
        let mut set = SkillSet::new();
        set.insert(make_entry("zebra")).unwrap();
        set.insert(make_entry("alpha")).unwrap();
        set.insert(make_entry("middle")).unwrap();

        let names = set.sorted_names();
        assert_eq!(names, vec!["alpha", "middle", "zebra"]);
    }

    #[test]
    fn test_skillset_iter() {
        let mut set = SkillSet::new();
        set.insert(make_entry("one")).unwrap();
        set.insert(make_entry("two")).unwrap();

        let mut collected: Vec<String> = set.iter().map(|(k, _)| k.clone()).collect();
        collected.sort();
        assert_eq!(collected, vec!["one", "two"]);
    }

    // --- validate_skill_name ---

    #[test]
    fn test_validate_valid_names() {
        assert!(validate_skill_name("login-flow").is_ok());
        assert!(validate_skill_name("a").is_ok());
        assert!(validate_skill_name("my-skill").is_ok());
        assert!(validate_skill_name("abc-def-ghi").is_ok());
    }

    #[test]
    fn test_validate_name_with_digits() {
        assert!(validate_skill_name("skill1").is_ok());
        assert!(validate_skill_name("v2-login").is_ok());
        assert!(validate_skill_name("test-42-flow").is_ok());
    }

    #[test]
    fn test_validate_empty_name() {
        let result = validate_skill_name("");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty"));
    }

    #[test]
    fn test_validate_name_too_long() {
        let long_name = "a".repeat(65);
        let result = validate_skill_name(&long_name);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("64"));
    }

    #[test]
    fn test_validate_uppercase_rejected() {
        let result = validate_skill_name("Login");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid character"));
    }

    #[test]
    fn test_validate_special_chars_rejected() {
        assert!(validate_skill_name("my_skill").is_err());
        assert!(validate_skill_name("my.skill").is_err());
        assert!(validate_skill_name("my skill").is_err());
    }

    #[test]
    fn test_validate_leading_hyphen_rejected() {
        let result = validate_skill_name("-leading");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("start with a hyphen"));
    }

    #[test]
    fn test_validate_trailing_hyphen_rejected() {
        let result = validate_skill_name("trailing-");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("end with a hyphen"));
    }

    #[test]
    fn test_validate_consecutive_hyphens_rejected() {
        let result = validate_skill_name("bad--name");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("consecutive"));
    }

    // --- SkillMetadata serialization ---

    #[test]
    fn test_skill_metadata_serde_roundtrip() {
        let meta = SkillMetadata {
            name: "test-skill".to_string(),
            description: "A test skill".to_string(),
            license: Some("MIT".to_string()),
            compatibility: vec!["remix-browser".to_string()],
            metadata: {
                let mut m = HashMap::new();
                m.insert("author".to_string(), "tester".to_string());
                m
            },
            allowed_tools: vec!["navigate".to_string(), "click".to_string()],
        };

        let json = serde_json::to_string(&meta).unwrap();
        let deserialized: SkillMetadata = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.name, "test-skill");
        assert_eq!(deserialized.description, "A test skill");
        assert_eq!(deserialized.license, Some("MIT".to_string()));
        assert_eq!(deserialized.compatibility, vec!["remix-browser"]);
        assert_eq!(deserialized.metadata.get("author").unwrap(), "tester");
        assert_eq!(deserialized.allowed_tools, vec!["navigate", "click"]);
    }
}
