use serde::{Deserialize, Serialize};

/// Supported test frameworks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TestFramework {
    Cargo,
    Pytest,
    Jest,
    Vitest,
    Go,
    Mocha,
}

impl TestFramework {
    /// Parse from a string identifier.
    pub fn from_str_id(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "cargo" | "rust" => Some(Self::Cargo),
            "pytest" | "python" => Some(Self::Pytest),
            "jest" => Some(Self::Jest),
            "vitest" => Some(Self::Vitest),
            "go" | "golang" => Some(Self::Go),
            "mocha" => Some(Self::Mocha),
            _ => None,
        }
    }

    /// Display name for this framework.
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Cargo => "cargo test",
            Self::Pytest => "pytest",
            Self::Jest => "jest",
            Self::Vitest => "vitest",
            Self::Go => "go test",
            Self::Mocha => "mocha",
        }
    }

    /// Command to run tests.
    pub fn test_command(&self) -> (&'static str, Vec<&'static str>) {
        match self {
            Self::Cargo => ("cargo", vec!["test"]),
            Self::Pytest => ("pytest", vec!["-v"]),
            Self::Jest => ("npx", vec!["jest", "--json"]),
            Self::Vitest => ("npx", vec!["vitest", "run", "--reporter=json"]),
            Self::Go => ("go", vec!["test", "-json", "./..."]),
            Self::Mocha => ("npx", vec!["mocha", "--reporter", "json"]),
        }
    }

    /// Command to list tests without running them.
    pub fn list_command(&self) -> Option<(&'static str, Vec<&'static str>)> {
        match self {
            Self::Cargo => Some(("cargo", vec!["test", "--", "--list"])),
            Self::Pytest => Some(("pytest", vec!["--collect-only", "-q"])),
            Self::Jest => Some(("npx", vec!["jest", "--listTests"])),
            Self::Vitest => Some(("npx", vec!["vitest", "list"])),
            Self::Go => Some(("go", vec!["test", "-list", ".", "./..."])),
            Self::Mocha => None,
        }
    }

    /// Apply file and test_name filters to command arguments for each framework.
    /// For `run` commands, pass both `file` and `test_name`.
    /// For `list` commands, typically only `file` is used (pass `None` for `test_name`).
    pub fn apply_filters(
        &self,
        args: &mut Vec<String>,
        file: Option<&str>,
        test_name: Option<&str>,
    ) {
        match self {
            Self::Cargo => {
                if let Some(file) = file {
                    args.push("--test".to_string());
                    args.push(file.to_string());
                }
                if let Some(test_name) = test_name {
                    args.push("--".to_string());
                    args.push(test_name.to_string());
                }
            }
            Self::Pytest => {
                if let Some(file) = file {
                    args.push(file.to_string());
                }
                if let Some(test_name) = test_name {
                    args.push("-k".to_string());
                    args.push(test_name.to_string());
                }
            }
            Self::Jest | Self::Vitest => {
                if let Some(file) = file {
                    args.push(file.to_string());
                }
                if let Some(test_name) = test_name {
                    args.push("-t".to_string());
                    args.push(test_name.to_string());
                }
            }
            Self::Go => {
                if let Some(test_name) = test_name {
                    args.push("-run".to_string());
                    args.push(test_name.to_string());
                }
                if let Some(file) = file {
                    // Replace ./... with the specific package path
                    args.retain(|a| *a != "./...");
                    args.push(file.to_string());
                }
            }
            Self::Mocha => {
                if let Some(file) = file {
                    args.push(file.to_string());
                }
                if let Some(test_name) = test_name {
                    args.push("--grep".to_string());
                    args.push(test_name.to_string());
                }
            }
        }
    }
}

/// Status of a single test.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TestStatus {
    Passed,
    Failed,
    Skipped,
    Error,
}

/// Result of a single test.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResult {
    pub name: String,
    pub status: TestStatus,
    pub duration_ms: Option<u64>,
    pub message: Option<String>,
}

/// Summary of a test run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestRunSummary {
    pub framework: TestFramework,
    pub duration_ms: Option<u64>,
    pub tests: Vec<TestResult>,
    pub raw_output: Option<String>,
}

impl TestRunSummary {
    pub fn total(&self) -> usize {
        self.tests.len()
    }

    pub fn passed(&self) -> usize {
        self.tests
            .iter()
            .filter(|t| t.status == TestStatus::Passed)
            .count()
    }

    pub fn failed(&self) -> usize {
        self.tests
            .iter()
            .filter(|t| t.status == TestStatus::Failed)
            .count()
    }

    pub fn skipped(&self) -> usize {
        self.tests
            .iter()
            .filter(|t| t.status == TestStatus::Skipped)
            .count()
    }

    pub fn errors(&self) -> usize {
        self.tests
            .iter()
            .filter(|t| t.status == TestStatus::Error)
            .count()
    }

    pub fn format_output(&self) -> String {
        let mut lines = Vec::new();
        lines.push(format!(
            "Test results ({}): {} total, {} passed, {} failed, {} skipped",
            self.framework.display_name(),
            self.total(),
            self.passed(),
            self.failed(),
            self.skipped(),
        ));

        if let Some(dur) = self.duration_ms {
            lines.push(format!("Duration: {}ms", dur));
        }

        // Show failed tests first
        let failed_tests: Vec<&TestResult> = self
            .tests
            .iter()
            .filter(|t| t.status == TestStatus::Failed || t.status == TestStatus::Error)
            .collect();

        if !failed_tests.is_empty() {
            lines.push(String::new());
            lines.push("Failed tests:".to_string());
            for test in &failed_tests {
                let status = match test.status {
                    TestStatus::Failed => "FAIL",
                    TestStatus::Error => "ERROR",
                    _ => "?",
                };
                lines.push(format!("  [{}] {}", status, test.name));
                if let Some(msg) = &test.message {
                    for line in msg.lines().take(10) {
                        lines.push(format!("    {}", line));
                    }
                }
            }
        }

        // Compact list of passed/skipped
        let passed_count = self.passed();
        let skipped_count = self.skipped();

        if passed_count > 0 && passed_count <= 50 {
            lines.push(String::new());
            lines.push("Passed tests:".to_string());
            for test in self.tests.iter().filter(|t| t.status == TestStatus::Passed) {
                lines.push(format!("  [PASS] {}", test.name));
            }
        } else if passed_count > 50 {
            lines.push(format!(
                "\n({} passed tests omitted for brevity)",
                passed_count
            ));
        }

        if skipped_count > 0 {
            lines.push(format!("\n({} tests skipped)", skipped_count));
        }

        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_framework_from_str() {
        assert_eq!(
            TestFramework::from_str_id("cargo"),
            Some(TestFramework::Cargo)
        );
        assert_eq!(
            TestFramework::from_str_id("rust"),
            Some(TestFramework::Cargo)
        );
        assert_eq!(
            TestFramework::from_str_id("Pytest"),
            Some(TestFramework::Pytest)
        );
        assert_eq!(
            TestFramework::from_str_id("jest"),
            Some(TestFramework::Jest)
        );
        assert_eq!(
            TestFramework::from_str_id("vitest"),
            Some(TestFramework::Vitest)
        );
        assert_eq!(TestFramework::from_str_id("GO"), Some(TestFramework::Go));
        assert_eq!(TestFramework::from_str_id("unknown"), None);
    }

    #[test]
    fn test_display_name() {
        assert_eq!(TestFramework::Cargo.display_name(), "cargo test");
        assert_eq!(TestFramework::Pytest.display_name(), "pytest");
        assert_eq!(TestFramework::Jest.display_name(), "jest");
    }

    #[test]
    fn test_run_summary_format() {
        let summary = TestRunSummary {
            framework: TestFramework::Cargo,
            duration_ms: Some(1234),
            tests: vec![
                TestResult {
                    name: "test_pass_1".to_string(),
                    status: TestStatus::Passed,
                    duration_ms: Some(100),
                    message: None,
                },
                TestResult {
                    name: "test_pass_2".to_string(),
                    status: TestStatus::Passed,
                    duration_ms: Some(200),
                    message: None,
                },
                TestResult {
                    name: "test_fail".to_string(),
                    status: TestStatus::Failed,
                    duration_ms: Some(300),
                    message: Some("assertion failed: expected 1, got 2".to_string()),
                },
            ],
            raw_output: None,
        };
        assert_eq!(summary.total(), 3);
        assert_eq!(summary.passed(), 2);
        assert_eq!(summary.failed(), 1);
        assert_eq!(summary.skipped(), 0);
        assert_eq!(summary.errors(), 0);
        let output = summary.format_output();
        assert!(output.contains("3 total"));
        assert!(output.contains("2 passed"));
        assert!(output.contains("1 failed"));
        assert!(output.contains("Duration: 1234ms"));
        assert!(output.contains("[FAIL] test_fail"));
        assert!(output.contains("assertion failed"));
        assert!(output.contains("[PASS] test_pass_1"));
    }

    #[test]
    fn test_apply_filters_cargo() {
        let mut args = vec!["test".to_string()];
        TestFramework::Cargo.apply_filters(&mut args, Some("my_test"), Some("test_name"));
        assert_eq!(args, vec!["test", "--test", "my_test", "--", "test_name"]);
    }

    #[test]
    fn test_apply_filters_pytest() {
        let mut args = vec!["-v".to_string()];
        TestFramework::Pytest.apply_filters(&mut args, Some("test_file.py"), Some("test_func"));
        assert_eq!(args, vec!["-v", "test_file.py", "-k", "test_func"]);
    }

    #[test]
    fn test_apply_filters_jest() {
        let mut args = vec!["jest".to_string(), "--json".to_string()];
        TestFramework::Jest.apply_filters(&mut args, Some("app.test.js"), Some("adds"));
        assert_eq!(args, vec!["jest", "--json", "app.test.js", "-t", "adds"]);
    }

    #[test]
    fn test_apply_filters_go() {
        let mut args = vec!["test".to_string(), "-json".to_string(), "./...".to_string()];
        TestFramework::Go.apply_filters(&mut args, Some("./pkg"), Some("TestAdd"));
        assert_eq!(args, vec!["test", "-json", "-run", "TestAdd", "./pkg"]);
    }

    #[test]
    fn test_apply_filters_none() {
        let mut args = vec!["test".to_string()];
        TestFramework::Cargo.apply_filters(&mut args, None, None);
        assert_eq!(args, vec!["test"]);
    }

    #[test]
    fn test_test_command() {
        let (cmd, args) = TestFramework::Cargo.test_command();
        assert_eq!(cmd, "cargo");
        assert_eq!(args, vec!["test"]);
    }

    #[test]
    fn test_list_command() {
        let result = TestFramework::Cargo.list_command();
        assert!(result.is_some());
        let (cmd, args) = result.unwrap();
        assert_eq!(cmd, "cargo");
        assert!(args.contains(&"--list"));

        assert!(TestFramework::Mocha.list_command().is_none());
    }
}
