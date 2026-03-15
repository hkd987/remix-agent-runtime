use super::types::TestFramework;
use crate::browser::mcp::ToolExecutionResult;
use crate::error::AgentError;

pub async fn execute_detect_test_framework(
    arguments: serde_json::Value,
) -> Result<ToolExecutionResult, AgentError> {
    let path = arguments
        .get("path")
        .and_then(|v| v.as_str())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    let frameworks = detect_frameworks(&path);

    if frameworks.is_empty() {
        return Ok(ToolExecutionResult {
            content: format!("No test frameworks detected in {}", path.display()),
            is_error: false,
        });
    }

    let mut lines = Vec::new();
    lines.push(format!(
        "Detected {} test framework(s) in {}:",
        frameworks.len(),
        path.display()
    ));

    for fw in &frameworks {
        lines.push(format!(
            "  - {} ({})",
            fw.display_name(),
            format!("{:?}", fw).to_lowercase()
        ));
    }

    Ok(ToolExecutionResult {
        content: lines.join("\n"),
        is_error: false,
    })
}

/// Detect test frameworks by scanning for marker files.
pub fn detect_frameworks(dir: &std::path::Path) -> Vec<TestFramework> {
    let mut frameworks = Vec::new();

    // Cargo.toml -> cargo test
    if dir.join("Cargo.toml").exists() {
        frameworks.push(TestFramework::Cargo);
    }

    // package.json -> check devDependencies for jest/vitest/mocha
    if let Some(fw) = detect_js_framework(dir) {
        frameworks.push(fw);
    }

    // pyproject.toml or pytest.ini -> pytest
    if dir.join("pyproject.toml").exists()
        || dir.join("pytest.ini").exists()
        || dir.join("setup.cfg").exists()
    {
        frameworks.push(TestFramework::Pytest);
    }

    // go.mod -> go test
    if dir.join("go.mod").exists() {
        frameworks.push(TestFramework::Go);
    }

    frameworks
}

fn detect_js_framework(dir: &std::path::Path) -> Option<TestFramework> {
    let pkg_path = dir.join("package.json");
    if !pkg_path.exists() {
        return None;
    }

    let content = std::fs::read_to_string(&pkg_path).ok()?;
    let pkg: serde_json::Value = serde_json::from_str(&content).ok()?;

    // Check devDependencies and dependencies
    let check_deps = |deps: &serde_json::Value| -> Option<TestFramework> {
        if deps.get("vitest").is_some() {
            return Some(TestFramework::Vitest);
        }
        if deps.get("jest").is_some() {
            return Some(TestFramework::Jest);
        }
        if deps.get("mocha").is_some() {
            return Some(TestFramework::Mocha);
        }
        None
    };

    if let Some(dev_deps) = pkg.get("devDependencies") {
        if let Some(fw) = check_deps(dev_deps) {
            return Some(fw);
        }
    }

    if let Some(deps) = pkg.get("dependencies") {
        if let Some(fw) = check_deps(deps) {
            return Some(fw);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_detect_cargo() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"test\"").unwrap();
        let frameworks = detect_frameworks(dir.path());
        assert_eq!(frameworks, vec![TestFramework::Cargo]);
    }

    #[test]
    fn test_detect_pytest() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("pyproject.toml"), "[tool.pytest]").unwrap();
        let frameworks = detect_frameworks(dir.path());
        assert_eq!(frameworks, vec![TestFramework::Pytest]);
    }

    #[test]
    fn test_detect_go() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("go.mod"), "module example.com/test").unwrap();
        let frameworks = detect_frameworks(dir.path());
        assert_eq!(frameworks, vec![TestFramework::Go]);
    }

    #[test]
    fn test_detect_jest() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{"devDependencies": {"jest": "^29.0.0"}}"#,
        )
        .unwrap();
        let frameworks = detect_frameworks(dir.path());
        assert_eq!(frameworks, vec![TestFramework::Jest]);
    }

    #[test]
    fn test_detect_vitest() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{"devDependencies": {"vitest": "^1.0.0"}}"#,
        )
        .unwrap();
        let frameworks = detect_frameworks(dir.path());
        assert_eq!(frameworks, vec![TestFramework::Vitest]);
    }

    #[test]
    fn test_detect_vitest_preferred_over_jest() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{"devDependencies": {"vitest": "^1.0.0", "jest": "^29.0.0"}}"#,
        )
        .unwrap();
        let frameworks = detect_frameworks(dir.path());
        // vitest is checked first
        assert_eq!(frameworks, vec![TestFramework::Vitest]);
    }

    #[test]
    fn test_detect_none() {
        let dir = TempDir::new().unwrap();
        let frameworks = detect_frameworks(dir.path());
        assert!(frameworks.is_empty());
    }

    #[test]
    fn test_detect_multiple() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();
        fs::write(dir.path().join("go.mod"), "module test").unwrap();
        let frameworks = detect_frameworks(dir.path());
        assert_eq!(frameworks.len(), 2);
        assert!(frameworks.contains(&TestFramework::Cargo));
        assert!(frameworks.contains(&TestFramework::Go));
    }

    #[tokio::test]
    async fn test_execute_detect_with_cargo() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();
        let result = execute_detect_test_framework(
            serde_json::json!({"path": dir.path().to_str().unwrap()}),
        )
        .await
        .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("cargo test"));
    }

    #[tokio::test]
    async fn test_execute_detect_empty() {
        let dir = TempDir::new().unwrap();
        let result = execute_detect_test_framework(
            serde_json::json!({"path": dir.path().to_str().unwrap()}),
        )
        .await
        .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("No test frameworks detected"));
    }
}
