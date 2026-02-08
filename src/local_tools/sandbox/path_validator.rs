use std::path::{Path, PathBuf};

use crate::error::AgentError;

/// Validates and resolves paths within a sandbox root directory.
#[derive(Debug)]
pub struct PathValidator {
    root: PathBuf,
}

impl PathValidator {
    pub fn new(root: PathBuf) -> Result<Self, AgentError> {
        let root = root.canonicalize().map_err(|e| {
            AgentError::LocalTool(format!("Failed to canonicalize sandbox root: {e}"))
        })?;
        Ok(Self { root })
    }

    /// Resolve a path, ensuring it stays within the sandbox root.
    /// For existing paths, canonicalize fully.
    /// For new paths (e.g., write_file), canonicalize the parent and append the filename.
    pub fn resolve_path(&self, path: &str) -> Result<PathBuf, AgentError> {
        let candidate = if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            self.root.join(path)
        };

        // Try full canonicalize first (works for existing paths)
        if let Ok(canonical) = candidate.canonicalize() {
            if canonical.starts_with(&self.root) {
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
                if canonical_parent.starts_with(&self.root) {
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
}
