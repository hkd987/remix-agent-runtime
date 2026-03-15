use crate::dev_tools::test_harness::types::{
    TestFramework, TestResult, TestRunSummary, TestStatus,
};

/// Parse pytest -v output into structured results.
pub fn parse_pytest_output(output: &str) -> TestRunSummary {
    let mut tests = Vec::new();
    let mut current_failure_name: Option<String> = None;
    let mut failure_messages: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut in_failures = false;

    for line in output.lines() {
        let trimmed = line.trim();

        // Parse test result lines: "test_file.py::test_name PASSED"
        if trimmed.contains("::") {
            if let Some(status) = parse_pytest_test_line(trimmed) {
                tests.push(status);
                continue;
            }
        }

        // Track FAILURES section
        if trimmed.starts_with("=") && trimmed.contains("FAILURES") {
            in_failures = true;
            continue;
        }

        if in_failures {
            if trimmed.starts_with("_") && trimmed.ends_with("_") {
                // New failure header: "_____ test_name _____"
                let name = trimmed.trim_matches('_').trim().to_string();
                current_failure_name = Some(name);
            } else if trimmed.starts_with("=")
                && (trimmed.contains("short test summary")
                    || trimmed.contains("passed")
                    || trimmed.contains("failed"))
            {
                in_failures = false;
                current_failure_name = None;
            } else if let Some(ref name) = current_failure_name {
                let entry = failure_messages.entry(name.clone()).or_default();
                if !entry.is_empty() {
                    entry.push('\n');
                }
                entry.push_str(trimmed);
            }
        }
    }

    // Attach failure messages
    // Failure headers may use short names (e.g. "test_div") while test results
    // use full names (e.g. "test_math.py::test_div"), so match by suffix.
    for test in &mut tests {
        if test.status == TestStatus::Failed {
            if let Some(msg) = failure_messages.remove(&test.name) {
                test.message = Some(msg);
            } else {
                // Try matching by short name: the part after the last "::"
                let short_name = test.name.rsplit("::").next().unwrap_or(&test.name);
                if let Some(msg) = failure_messages.remove(short_name) {
                    test.message = Some(msg);
                }
            }
        }
    }

    TestRunSummary {
        framework: TestFramework::Pytest,
        duration_ms: None,
        tests,
        raw_output: Some(output.to_string()),
    }
}

fn parse_pytest_test_line(line: &str) -> Option<TestResult> {
    // Formats:
    // "test_file.py::test_name PASSED"
    // "test_file.py::TestClass::test_name FAILED"
    // "test_file.py::test_name SKIPPED (reason)"
    // "test_file.py::test_name PASSED                           [ 50%]"

    // Strip trailing percentage indicator like "[ 50%]" or "[100%]" before parsing
    let stripped = line.trim_end();
    let stripped = if let Some(bracket_start) = stripped.rfind('[') {
        if stripped.ends_with(']') {
            stripped[..bracket_start].trim_end()
        } else {
            stripped
        }
    } else {
        stripped
    };

    let parts: Vec<&str> = stripped.rsplitn(2, ' ').collect();
    if parts.len() < 2 {
        return None;
    }

    let status_str = parts[0].trim();
    let rest = parts[1].trim();

    if !rest.contains("::") {
        return None;
    }

    let status = match status_str {
        "PASSED" => TestStatus::Passed,
        "FAILED" => TestStatus::Failed,
        "SKIPPED" => TestStatus::Skipped,
        "ERROR" => TestStatus::Error,
        _ => return None,
    };

    Some(TestResult {
        name: rest.to_string(),
        status,
        duration_ms: None,
        message: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_pytest_simple() {
        let output = r#"
============================= test session starts =============================
collected 3 items

test_math.py::test_add PASSED
test_math.py::test_sub PASSED
test_math.py::test_div FAILED

============================= FAILURES =============================
_____ test_div _____
    def test_div():
>       assert 1/0 == 0
E       ZeroDivisionError: division by zero

test_math.py:10: ZeroDivisionError
========================= short test summary info ==========================
FAILED test_math.py::test_div - ZeroDivisionError: division by zero
========================= 2 passed, 1 failed in 0.05s =========================
"#;

        let summary = parse_pytest_output(output);
        assert_eq!(summary.framework, TestFramework::Pytest);
        assert_eq!(summary.total(), 3);
        assert_eq!(summary.passed(), 2);
        assert_eq!(summary.failed(), 1);

        let failed = summary
            .tests
            .iter()
            .find(|t| t.name.contains("test_div"))
            .unwrap();
        assert_eq!(failed.status, TestStatus::Failed);
        assert!(failed
            .message
            .as_ref()
            .unwrap()
            .contains("ZeroDivisionError"));
    }

    #[test]
    fn test_parse_pytest_all_pass() {
        let output = "test_a.py::test_one PASSED\ntest_a.py::test_two PASSED\n";
        let summary = parse_pytest_output(output);
        assert_eq!(summary.total(), 2);
        assert_eq!(summary.passed(), 2);
        assert_eq!(summary.failed(), 0);
    }

    #[test]
    fn test_parse_pytest_with_percentage() {
        let output = "test_a.py::test_one PASSED                           [ 50%]\ntest_a.py::test_two PASSED                           [100%]\n";
        let summary = parse_pytest_output(output);
        assert_eq!(summary.total(), 2);
        assert_eq!(summary.passed(), 2);
    }

    #[test]
    fn test_parse_pytest_empty() {
        let summary = parse_pytest_output("");
        assert_eq!(summary.total(), 0);
    }

    #[test]
    fn test_parse_pytest_test_line() {
        let result = parse_pytest_test_line("test_file.py::test_name PASSED");
        assert!(result.is_some());
        let r = result.unwrap();
        assert_eq!(r.name, "test_file.py::test_name");
        assert_eq!(r.status, TestStatus::Passed);
    }

    #[test]
    fn test_parse_pytest_test_line_failed() {
        let result = parse_pytest_test_line("test_file.py::TestClass::test_method FAILED");
        assert!(result.is_some());
        let r = result.unwrap();
        assert_eq!(r.name, "test_file.py::TestClass::test_method");
        assert_eq!(r.status, TestStatus::Failed);
    }
}
