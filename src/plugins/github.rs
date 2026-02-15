use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::AgentError;

/// Check if `git` is available on the system PATH.
pub fn check_git_available() -> Result<(), AgentError> {
    let output = Command::new("git")
        .arg("--version")
        .output()
        .map_err(|e| AgentError::Plugin(format!("git is not available: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AgentError::Plugin(format!(
            "git --version failed: {stderr}"
        )));
    }

    Ok(())
}

/// Parse a GitHub repo string in "owner/repo" format.
/// Returns (owner, repo) or an error if the format is invalid.
fn parse_repo(repo: &str) -> Result<(&str, &str), AgentError> {
    let parts: Vec<&str> = repo.split('/').collect();
    if parts.len() != 2 {
        return Err(AgentError::Plugin(format!(
            "Invalid GitHub repo format: '{repo}'. Expected 'owner/repo'"
        )));
    }

    let owner = parts[0];
    let repo_name = parts[1];

    if owner.is_empty() || repo_name.is_empty() {
        return Err(AgentError::Plugin(format!(
            "Invalid GitHub repo format: '{repo}'. Owner and repo name must not be empty"
        )));
    }

    Ok((owner, repo_name))
}

/// Construct the cache path for a GitHub plugin.
/// Format: {cache_dir}/{owner}/{repo}/
pub fn github_cache_path(cache_dir: &Path, repo: &str) -> Result<PathBuf, AgentError> {
    let (owner, repo_name) = parse_repo(repo)?;
    Ok(cache_dir.join(owner).join(repo_name))
}

/// Clone or update a GitHub repository to the local cache.
///
/// - If `.git` exists at target: `git fetch origin && git checkout {ref}`
/// - Otherwise: `git clone --depth 1 --branch {ref} https://github.com/{owner}/{repo}.git {target}`
/// - Fallback: clone without --branch, then `git checkout {ref}` (for commit hashes)
/// - If no ref specified, default to cloning HEAD
pub fn clone_or_update_plugin(
    repo: &str,
    git_ref: Option<&str>,
    cache_dir: &Path,
) -> Result<PathBuf, AgentError> {
    check_git_available()?;
    let target = github_cache_path(cache_dir, repo)?;

    let git_dir = target.join(".git");
    if git_dir.exists() {
        update_existing_repo(&target, git_ref)?;
    } else {
        clone_new_repo(repo, git_ref, &target)?;
    }

    Ok(target)
}

/// Update an existing cloned repository by fetching and checking out the desired ref.
fn update_existing_repo(target: &Path, git_ref: Option<&str>) -> Result<(), AgentError> {
    // Fetch latest from origin
    let fetch_output = Command::new("git")
        .args(["fetch", "origin"])
        .current_dir(target)
        .output()
        .map_err(|e| AgentError::Plugin(format!("Failed to run git fetch: {e}")))?;

    if !fetch_output.status.success() {
        let stderr = String::from_utf8_lossy(&fetch_output.stderr);
        return Err(AgentError::Plugin(format!("git fetch failed: {stderr}")));
    }

    // Checkout the specified ref, or default branch if none
    if let Some(r) = git_ref {
        let checkout_output = Command::new("git")
            .args(["checkout", r])
            .current_dir(target)
            .output()
            .map_err(|e| AgentError::Plugin(format!("Failed to run git checkout: {e}")))?;

        if !checkout_output.status.success() {
            let stderr = String::from_utf8_lossy(&checkout_output.stderr);
            return Err(AgentError::Plugin(format!(
                "git checkout '{r}' failed: {stderr}"
            )));
        }
    }

    Ok(())
}

/// Clone a new repository from GitHub.
fn clone_new_repo(repo: &str, git_ref: Option<&str>, target: &Path) -> Result<(), AgentError> {
    let url = format!("https://github.com/{repo}.git");

    // Ensure parent directory exists
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            AgentError::Plugin(format!(
                "Failed to create cache directory '{}': {e}",
                parent.display()
            ))
        })?;
    }

    match git_ref {
        Some(r) => clone_with_ref(&url, r, target),
        None => clone_head(&url, target),
    }
}

/// Clone with a specific ref. First tries `--branch` (works for tags and branches),
/// then falls back to a full clone + checkout (works for commit SHAs).
fn clone_with_ref(url: &str, git_ref: &str, target: &Path) -> Result<(), AgentError> {
    let branch_output = Command::new("git")
        .args([
            "clone",
            "--depth",
            "1",
            "--branch",
            git_ref,
            url,
            &target.to_string_lossy(),
        ])
        .output()
        .map_err(|e| AgentError::Plugin(format!("Failed to run git clone: {e}")))?;

    if branch_output.status.success() {
        return Ok(());
    }

    // Fallback: clone without --branch (full history needed for arbitrary SHAs),
    // then checkout the specific ref.
    let clone_output = Command::new("git")
        .args(["clone", url, &target.to_string_lossy()])
        .output()
        .map_err(|e| AgentError::Plugin(format!("Failed to run git clone (fallback): {e}")))?;

    if !clone_output.status.success() {
        let stderr = String::from_utf8_lossy(&clone_output.stderr);
        return Err(AgentError::Plugin(format!(
            "git clone '{url}' failed: {stderr}"
        )));
    }

    let checkout_output = Command::new("git")
        .args(["checkout", git_ref])
        .current_dir(target)
        .output()
        .map_err(|e| AgentError::Plugin(format!("Failed to run git checkout '{git_ref}': {e}")))?;

    if !checkout_output.status.success() {
        let stderr = String::from_utf8_lossy(&checkout_output.stderr);
        return Err(AgentError::Plugin(format!(
            "git checkout '{git_ref}' failed: {stderr}"
        )));
    }

    Ok(())
}

/// Clone HEAD (default branch) with shallow depth.
fn clone_head(url: &str, target: &Path) -> Result<(), AgentError> {
    let output = Command::new("git")
        .args(["clone", "--depth", "1", url, &target.to_string_lossy()])
        .output()
        .map_err(|e| AgentError::Plugin(format!("Failed to run git clone: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AgentError::Plugin(format!(
            "git clone '{url}' failed: {stderr}"
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // -------------------------------------------------------
    // parse_repo tests
    // -------------------------------------------------------

    #[test]
    fn test_parse_repo_valid() {
        let (owner, repo) = parse_repo("octocat/hello-world").unwrap();
        assert_eq!(owner, "octocat");
        assert_eq!(repo, "hello-world");
    }

    #[test]
    fn test_parse_repo_with_hyphens_and_underscores() {
        let (owner, repo) = parse_repo("my-org/my_repo-name").unwrap();
        assert_eq!(owner, "my-org");
        assert_eq!(repo, "my_repo-name");
    }

    #[test]
    fn test_parse_repo_no_slash() {
        let err = parse_repo("just-a-name").unwrap_err();
        assert!(matches!(err, AgentError::Plugin(_)));
        assert!(err.to_string().contains("Invalid GitHub repo format"));
    }

    #[test]
    fn test_parse_repo_too_many_slashes() {
        let err = parse_repo("a/b/c").unwrap_err();
        assert!(matches!(err, AgentError::Plugin(_)));
        assert!(err.to_string().contains("Invalid GitHub repo format"));
    }

    #[test]
    fn test_parse_repo_empty_owner() {
        let err = parse_repo("/repo").unwrap_err();
        assert!(matches!(err, AgentError::Plugin(_)));
        assert!(err.to_string().contains("must not be empty"));
    }

    #[test]
    fn test_parse_repo_empty_repo_name() {
        let err = parse_repo("owner/").unwrap_err();
        assert!(matches!(err, AgentError::Plugin(_)));
        assert!(err.to_string().contains("must not be empty"));
    }

    #[test]
    fn test_parse_repo_empty_string() {
        let err = parse_repo("").unwrap_err();
        assert!(matches!(err, AgentError::Plugin(_)));
    }

    #[test]
    fn test_parse_repo_just_slash() {
        let err = parse_repo("/").unwrap_err();
        assert!(matches!(err, AgentError::Plugin(_)));
        assert!(err.to_string().contains("must not be empty"));
    }

    // -------------------------------------------------------
    // github_cache_path tests
    // -------------------------------------------------------

    #[test]
    fn test_github_cache_path_valid() {
        let cache_dir = Path::new("/tmp/plugins");
        let path = github_cache_path(cache_dir, "octocat/hello-world").unwrap();
        assert_eq!(path, PathBuf::from("/tmp/plugins/octocat/hello-world"));
    }

    #[test]
    fn test_github_cache_path_preserves_case() {
        let cache_dir = Path::new("/home/user/.cache");
        let path = github_cache_path(cache_dir, "MyOrg/MyRepo").unwrap();
        assert_eq!(path, PathBuf::from("/home/user/.cache/MyOrg/MyRepo"));
    }

    #[test]
    fn test_github_cache_path_with_nested_cache_dir() {
        let cache_dir = Path::new("/a/b/c/d");
        let path = github_cache_path(cache_dir, "owner/repo").unwrap();
        assert_eq!(path, PathBuf::from("/a/b/c/d/owner/repo"));
    }

    #[test]
    fn test_github_cache_path_invalid_repo() {
        let cache_dir = Path::new("/tmp");
        let err = github_cache_path(cache_dir, "invalid").unwrap_err();
        assert!(matches!(err, AgentError::Plugin(_)));
    }

    #[test]
    fn test_github_cache_path_relative_cache_dir() {
        let cache_dir = Path::new("relative/cache");
        let path = github_cache_path(cache_dir, "owner/repo").unwrap();
        assert_eq!(path, PathBuf::from("relative/cache/owner/repo"));
    }

    // -------------------------------------------------------
    // check_git_available test
    // -------------------------------------------------------

    #[test]
    fn test_check_git_available_succeeds() {
        // git should be available in CI and dev environments
        let result = check_git_available();
        assert!(
            result.is_ok(),
            "git should be available: {:?}",
            result.err()
        );
    }

    // -------------------------------------------------------
    // clone_or_update_plugin tests (filesystem-based, no network)
    // -------------------------------------------------------

    #[test]
    fn test_clone_or_update_creates_parent_dirs() {
        // This test validates that clone_new_repo creates parent directories.
        // We use a temp dir and a fake repo URL that will fail, but we can
        // verify the parent directory structure was created.
        let tmp = TempDir::new().unwrap();
        let cache_dir = tmp.path().join("deep/nested/cache");

        // This will fail at the git clone step (invalid repo URL), but
        // the parent directories should be created.
        let result = clone_or_update_plugin("fake-owner/fake-repo", None, &cache_dir);
        assert!(result.is_err());

        // Parent directory should exist (owner dir)
        let owner_dir = cache_dir.join("fake-owner");
        assert!(owner_dir.exists(), "Parent directory should be created");
    }

    #[test]
    fn test_clone_or_update_invalid_repo_format() {
        let tmp = TempDir::new().unwrap();
        let err = clone_or_update_plugin("not-valid", None, tmp.path()).unwrap_err();
        assert!(matches!(err, AgentError::Plugin(_)));
        assert!(err.to_string().contains("Invalid GitHub repo format"));
    }

    #[test]
    fn test_clone_or_update_with_existing_git_dir_fetches() {
        // Set up a fake git repo at the target path so it triggers the update branch
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("test-owner").join("test-repo");
        fs::create_dir_all(&target).unwrap();

        // Initialize a real git repo so fetch/checkout can be attempted
        let init_output = Command::new("git")
            .args(["init"])
            .current_dir(&target)
            .output()
            .unwrap();
        assert!(init_output.status.success());

        // The update will fail because there's no remote "origin", but we
        // verify the code takes the update path (not clone path)
        let result = clone_or_update_plugin("test-owner/test-repo", None, tmp.path());
        // With no ref, update_existing_repo fetches but no checkout needed.
        // fetch will fail since there's no remote, which is expected.
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("git fetch failed"),
            "Should attempt fetch on existing repo, got: {err_msg}"
        );
    }

    #[test]
    fn test_clone_or_update_existing_repo_with_ref() {
        // Set up a local git repo with a commit so we can test checkout
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("owner").join("repo");
        fs::create_dir_all(&target).unwrap();

        // Initialize git repo with a commit
        Command::new("git")
            .args(["init"])
            .current_dir(&target)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(&target)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(&target)
            .output()
            .unwrap();
        fs::write(target.join("README.md"), "# Test").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(&target)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(&target)
            .output()
            .unwrap();

        // The fetch will fail (no remote), so we expect an error about fetch
        let result = clone_or_update_plugin("owner/repo", Some("main"), tmp.path());
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("git fetch failed"));
    }

    #[test]
    fn test_clone_nonexistent_github_repo() {
        // Attempt to clone a repo that doesn't exist on GitHub
        let tmp = TempDir::new().unwrap();
        let result = clone_or_update_plugin(
            "this-owner-definitely-does-not-exist-abc123/nonexistent-repo-xyz789",
            None,
            tmp.path(),
        );
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("git clone") || err_msg.contains("failed"),
            "Should report clone failure, got: {err_msg}"
        );
    }

    // -------------------------------------------------------
    // Error variant tests
    // -------------------------------------------------------

    #[test]
    fn test_all_errors_are_plugin_variant() {
        // Verify that all errors produced by this module are AgentError::Plugin
        let cases: Vec<Result<_, AgentError>> = vec![
            parse_repo("invalid").map(|_| ()),
            parse_repo("a/b/c").map(|_| ()),
            parse_repo("/repo").map(|_| ()),
            parse_repo("owner/").map(|_| ()),
            github_cache_path(Path::new("/tmp"), "bad").map(|_| ()),
        ];

        for result in cases {
            let err = result.unwrap_err();
            assert!(
                matches!(err, AgentError::Plugin(_)),
                "Expected Plugin variant, got: {err:?}"
            );
        }
    }

    #[test]
    fn test_error_messages_are_descriptive() {
        let err = parse_repo("noslash").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("noslash"),
            "Error should include the input: {msg}"
        );
        assert!(
            msg.contains("owner/repo"),
            "Error should include expected format: {msg}"
        );
    }
}
