use std::path::{Path, PathBuf};

use crate::error::AgentError;

/// Validates and resolves paths within a sandbox root directory.
///
/// When `bypass_sandbox` is true, path resolution still canonicalizes paths
/// but does not enforce the `starts_with(root)` containment check. This is
/// used when the agent runs in `BypassPermissions` mode.
#[derive(Debug)]
pub struct PathValidator {
    root: PathBuf,
    bypass_sandbox: bool,
}

impl PathValidator {
    pub fn new(root: PathBuf) -> Result<Self, AgentError> {
        let root = root.canonicalize().map_err(|e| {
            AgentError::LocalTool(format!("Failed to canonicalize sandbox root: {e}"))
        })?;
        Ok(Self {
            root,
            bypass_sandbox: false,
        })
    }

    /// Create a `PathValidator` that skips the sandbox containment check.
    pub fn new_with_bypass(root: PathBuf, bypass_sandbox: bool) -> Result<Self, AgentError> {
        let root = root.canonicalize().map_err(|e| {
            AgentError::LocalTool(format!("Failed to canonicalize sandbox root: {e}"))
        })?;
        Ok(Self {
            root,
            bypass_sandbox,
        })
    }

    /// Resolve a path, ensuring it stays within the sandbox root.
    /// For existing paths, canonicalize fully.
    /// For new paths (e.g., write_file), canonicalize the parent and append the filename.
    ///
    /// When `bypass_sandbox` is true, paths are still canonicalized but the
    /// containment check is skipped, allowing access to any path on the system.
    pub fn resolve_path(&self, path: &str) -> Result<PathBuf, AgentError> {
        let candidate = if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            self.root.join(path)
        };

        // Try full canonicalize first (works for existing paths)
        if let Ok(canonical) = candidate.canonicalize() {
            if self.bypass_sandbox || canonical.starts_with(&self.root) {
                return Ok(canonical);
            }
            return Err(AgentError::LocalTool(format!(
                "Path '{}' resolves outside sandbox root",
                path
            )));
        }

        // For new files: canonicalize parent, append filename
        if let Some(parent) = candidate.parent() {
            if let Ok(canonical_parent) = parent.canonicalize() {
                if self.bypass_sandbox || canonical_parent.starts_with(&self.root) {
                    if let Some(filename) = candidate.file_name() {
                        return Ok(canonical_parent.join(filename));
                    }
                }
            }
        }

        Err(AgentError::LocalTool(format!(
            "Path '{}' is outside sandbox or parent directory does not exist",
            path
        )))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_resolve_relative_path() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("hello.txt");
        fs::write(&file_path, "content").unwrap();

        let validator = PathValidator::new(tmp.path().to_path_buf()).unwrap();
        let resolved = validator.resolve_path("hello.txt").unwrap();
        assert_eq!(resolved, file_path.canonicalize().unwrap());
    }

    #[test]
    fn test_resolve_absolute_path_inside() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("file.txt");
        fs::write(&file_path, "content").unwrap();

        let validator = PathValidator::new(tmp.path().to_path_buf()).unwrap();
        let resolved = validator.resolve_path(file_path.to_str().unwrap()).unwrap();
        assert_eq!(resolved, file_path.canonicalize().unwrap());
    }

    #[test]
    fn test_resolve_absolute_path_outside() {
        let tmp = TempDir::new().unwrap();
        let validator = PathValidator::new(tmp.path().to_path_buf()).unwrap();
        let result = validator.resolve_path("/etc/passwd");
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("outside sandbox"));
    }

    #[test]
    fn test_resolve_with_dot_dot() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("subdir")).unwrap();

        let validator = PathValidator::new(tmp.path().to_path_buf()).unwrap();
        // Attempting to traverse outside via ..
        let result = validator.resolve_path("subdir/../../etc/passwd");
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_new_file() {
        let tmp = TempDir::new().unwrap();
        let validator = PathValidator::new(tmp.path().to_path_buf()).unwrap();

        // File doesn't exist yet but parent does
        let resolved = validator.resolve_path("new_file.txt").unwrap();
        let expected = tmp.path().canonicalize().unwrap().join("new_file.txt");
        assert_eq!(resolved, expected);
    }

    #[test]
    fn test_resolve_new_file_parent_missing() {
        let tmp = TempDir::new().unwrap();
        let validator = PathValidator::new(tmp.path().to_path_buf()).unwrap();

        // Parent directory doesn't exist
        let result = validator.resolve_path("nonexistent_dir/file.txt");
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("outside sandbox or parent directory does not exist"));
    }

    #[cfg(unix)]
    #[test]
    fn test_resolve_symlink_inside() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("target.txt");
        fs::write(&target, "content").unwrap();
        let link = tmp.path().join("link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let validator = PathValidator::new(tmp.path().to_path_buf()).unwrap();
        let resolved = validator.resolve_path("link.txt").unwrap();
        assert_eq!(resolved, target.canonicalize().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn test_resolve_symlink_outside() {
        let tmp = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let outside_file = outside.path().join("secret.txt");
        fs::write(&outside_file, "secret").unwrap();

        let link = tmp.path().join("escape.txt");
        std::os::unix::fs::symlink(&outside_file, &link).unwrap();

        let validator = PathValidator::new(tmp.path().to_path_buf()).unwrap();
        let result = validator.resolve_path("escape.txt");
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("outside sandbox"));
    }

    #[test]
    fn test_root_accessor() {
        let tmp = TempDir::new().unwrap();
        let validator = PathValidator::new(tmp.path().to_path_buf()).unwrap();
        assert_eq!(validator.root(), tmp.path().canonicalize().unwrap());
    }

    #[test]
    fn test_new_with_nonexistent_root() {
        let result = PathValidator::new(PathBuf::from("/nonexistent/path/that/doesnt/exist"));
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Failed to canonicalize sandbox root"));
    }

    // --- bypass_sandbox tests ---

    #[test]
    fn test_bypass_allows_absolute_path_outside_sandbox() {
        let tmp = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let outside_file = outside.path().join("data.txt");
        fs::write(&outside_file, "outside content").unwrap();

        let validator = PathValidator::new_with_bypass(tmp.path().to_path_buf(), true).unwrap();
        let resolved = validator
            .resolve_path(outside_file.to_str().unwrap())
            .unwrap();
        assert_eq!(resolved, outside_file.canonicalize().unwrap());
    }

    #[test]
    fn test_bypass_false_still_blocks_outside_paths() {
        let tmp = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let outside_file = outside.path().join("data.txt");
        fs::write(&outside_file, "outside content").unwrap();

        let validator = PathValidator::new_with_bypass(tmp.path().to_path_buf(), false).unwrap();
        let result = validator.resolve_path(outside_file.to_str().unwrap());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("outside sandbox"));
    }

    #[test]
    fn test_bypass_allows_new_file_outside_sandbox() {
        let tmp = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();

        let validator = PathValidator::new_with_bypass(tmp.path().to_path_buf(), true).unwrap();
        let new_file_path = outside.path().join("new_file.txt");
        let resolved = validator
            .resolve_path(new_file_path.to_str().unwrap())
            .unwrap();
        let expected = outside.path().canonicalize().unwrap().join("new_file.txt");
        assert_eq!(resolved, expected);
    }

    #[cfg(unix)]
    #[test]
    fn test_bypass_allows_symlink_outside_sandbox() {
        let tmp = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let outside_file = outside.path().join("target.txt");
        fs::write(&outside_file, "symlink target").unwrap();

        let link = tmp.path().join("link.txt");
        std::os::unix::fs::symlink(&outside_file, &link).unwrap();

        let validator = PathValidator::new_with_bypass(tmp.path().to_path_buf(), true).unwrap();
        let resolved = validator.resolve_path("link.txt").unwrap();
        assert_eq!(resolved, outside_file.canonicalize().unwrap());
    }

    #[test]
    fn test_bypass_still_canonicalizes_paths() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("subdir")).unwrap();
        let file_path = tmp.path().join("subdir").join("file.txt");
        fs::write(&file_path, "content").unwrap();

        let validator = PathValidator::new_with_bypass(tmp.path().to_path_buf(), true).unwrap();
        // Use a path with ../ that still resolves to an existing file
        let resolved = validator.resolve_path("subdir/../subdir/file.txt").unwrap();
        assert_eq!(resolved, file_path.canonicalize().unwrap());
    }

    #[test]
    fn test_new_with_bypass_defaults_to_false() {
        let tmp = TempDir::new().unwrap();
        let validator = PathValidator::new(tmp.path().to_path_buf()).unwrap();
        // The default constructor should not bypass — verify outside path is blocked
        let outside = TempDir::new().unwrap();
        let outside_file = outside.path().join("file.txt");
        fs::write(&outside_file, "data").unwrap();
        let result = validator.resolve_path(outside_file.to_str().unwrap());
        assert!(result.is_err());
    }
}
