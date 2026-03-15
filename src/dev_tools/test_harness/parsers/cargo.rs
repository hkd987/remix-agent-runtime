use crate::dev_tools::test_harness::types::{
    TestFramework, TestResult, TestRunSummary, TestStatus,
};

/// Parse cargo test human-readable output into structured results.
pub fn parse_cargo_test_output(output: &str) -> TestRunSummary {
    let mut tests = Vec::new();
    // Track current failing test for capturing failure messages
    let mut current_failure: Option<String> = None;
    let mut failure_messages: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut in_failures_section = false;

    for line in output.lines() {
        let trimmed = line.trim();

        // Parse test result lines: "test module::test_name ... ok"
        if trimmed.starts_with("test ")
            && (trimmed.ends_with("... ok")
                || trimmed.ends_with("... FAILED")
                || trimmed.ends_with("... ignored"))
        {
            let name = trimmed.strip_prefix("test ").unwrap_or(trimmed);

            if let Some(name) = name.strip_suffix(" ... ok") {
                tests.push(TestResult {
                    name: name.to_string(),
                    status: TestStatus::Passed,
                    duration_ms: None,
                    message: None,
                });
            } else if let Some(name) = name.strip_suffix(" ... FAILED") {
                tests.push(TestResult {
                    name: name.to_string(),
                    status: TestStatus::Failed,
                    duration_ms: None,
                    message: None,
                });
            } else if let Some(name) = name.strip_suffix(" ... ignored") {
                tests.push(TestResult {
                    name: name.to_string(),
                    status: TestStatus::Skipped,
                    duration_ms: None,
                    message: None,
                });
            }
        }

        // Track failures section for error messages
        if trimmed == "failures:" {
            in_failures_section = true;
            continue;
        }

        if in_failures_section {
            if trimmed.starts_with("---- ") && trimmed.ends_with(" stdout ----") {
                let name = trimmed
                    .strip_prefix("---- ")
                    .and_then(|s| s.strip_suffix(" stdout ----"))
                    .unwrap_or("")
                    .to_string();
                current_failure = Some(name);
            } else if trimmed == "failures:" || trimmed.starts_with("test result:") {
                current_failure = None;
                in_failures_section = trimmed == "failures:";
            } else if let Some(ref name) = current_failure {
                let entry = failure_messages.entry(name.clone()).or_default();
                if !entry.is_empty() {
                    entry.push('\n');
                }
                entry.push_str(trimmed);
            }
        }
    }

    // Attach failure messages to test results
    for test in &mut tests {
        if test.status == TestStatus::Failed {
            if let Some(msg) = failure_messages.remove(&test.name) {
                test.message = Some(msg);
            }
        }
    }

    // Handle case where no individual tests were parsed but summary exists
    if tests.is_empty() && !output.is_empty() {
        for line in output.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("test result:") {
                if let Some(counts) = parse_summary_line(trimmed) {
                    // Create synthetic test entries from summary counts
                    for i in 0..counts.1 {
                        tests.push(TestResult {
                            name: format!("(passed #{})", i + 1),
                            status: TestStatus::Passed,
                            duration_ms: None,
                            message: None,
                        });
                    }
                    for i in 0..counts.2 {
                        tests.push(TestResult {
                            name: format!("(failed #{})", i + 1),
                            status: TestStatus::Failed,
                            duration_ms: None,
                            message: None,
                        });
                    }
                    for i in 0..counts.3 {
                        tests.push(TestResult {
                            name: format!("(ignored #{})", i + 1),
                            status: TestStatus::Skipped,
                            duration_ms: None,
                            message: None,
                        });
                    }
                }
            }
        }
    }

    TestRunSummary {
        framework: TestFramework::Cargo,
        duration_ms: None,
        tests,
        raw_output: Some(output.to_string()),
    }
}

fn parse_summary_line(line: &str) -> Option<(usize, usize, usize, usize)> {
    // "test result: ok. 10 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out"
    let mut passed = 0;
    let mut failed = 0;
    let mut ignored = 0;

    for part in line.split(';') {
        let part = part.trim();
        if let Some(n) = extract_count(part, "passed") {
            passed = n;
        } else if let Some(n) = extract_count(part, "failed") {
            failed = n;
        } else if let Some(n) = extract_count(part, "ignored") {
            ignored = n;
        }
    }

    let total = passed + failed + ignored;
    if total > 0 {
        Some((total, passed, failed, ignored))
    } else {
        None
    }
}

fn extract_count(part: &str, suffix: &str) -> Option<usize> {
    let part = part.trim();
    if part.ends_with(suffix) {
        let num_str = part.strip_suffix(suffix)?.trim();
        // Handle "ok. 10" pattern
        let num_str = if let Some(after_dot) = num_str.strip_prefix("ok.") {
            after_dot.trim()
        } else if let Some(after_dot) = num_str.strip_prefix("FAILED.") {
            after_dot.trim()
        } else {
            num_str
        };
        num_str.parse().ok()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_cargo_output() {
        let output = r#"
running 3 tests
test tests::test_one ... ok
test tests::test_two ... ok
test tests::test_three ... FAILED

failures:

---- tests::test_three stdout ----
thread 'tests::test_three' panicked at 'assertion failed: false'

failures:
    tests::test_three

test result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
"#;
        let summary = parse_cargo_test_output(output);
        assert_eq!(summary.framework, TestFramework::Cargo);
        assert_eq!(summary.total(), 3);
        assert_eq!(summary.passed(), 2);
        assert_eq!(summary.failed(), 1);
        assert_eq!(summary.skipped(), 0);
        assert_eq!(summary.tests.len(), 3);

        let failed = summary
            .tests
            .iter()
            .find(|t| t.name == "tests::test_three")
            .unwrap();
        assert_eq!(failed.status, TestStatus::Failed);
        assert!(failed
            .message
            .as_ref()
            .unwrap()
            .contains("assertion failed"));
    }

    #[test]
    fn test_parse_all_pass() {
        let output = r#"
running 2 tests
test tests::test_a ... ok
test tests::test_b ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
"#;
        let summary = parse_cargo_test_output(output);
        assert_eq!(summary.total(), 2);
        assert_eq!(summary.passed(), 2);
        assert_eq!(summary.failed(), 0);
    }

    #[test]
    fn test_parse_with_ignored() {
        let output = r#"
running 3 tests
test tests::test_a ... ok
test tests::test_b ... ignored
test tests::test_c ... ok

test result: ok. 2 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out
"#;
        let summary = parse_cargo_test_output(output);
        assert_eq!(summary.total(), 3);
        assert_eq!(summary.passed(), 2);
        assert_eq!(summary.skipped(), 1);
    }

    #[test]
    fn test_parse_empty_output() {
        let summary = parse_cargo_test_output("");
        assert_eq!(summary.total(), 0);
        assert_eq!(summary.tests.len(), 0);
    }

    #[test]
    fn test_extract_count() {
        assert_eq!(extract_count("2 passed", "passed"), Some(2));
        assert_eq!(extract_count("0 failed", "failed"), Some(0));
        assert_eq!(extract_count("1 ignored", "ignored"), Some(1));
        assert_eq!(extract_count("not a count", "passed"), None);
    }
}
