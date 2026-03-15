use crate::dev_tools::test_harness::types::{
    TestFramework, TestResult, TestRunSummary, TestStatus,
};

/// Parse jest --json output into structured results.
pub fn parse_jest_output(output: &str) -> TestRunSummary {
    // Jest --json outputs a JSON object
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(output) {
        return parse_jest_json(&json);
    }

    // Try to find JSON in the output (jest sometimes prints warnings before JSON)
    if let Some(json_start) = output.find('{') {
        let json_str = &output[json_start..];
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(json_str) {
            return parse_jest_json(&json);
        }
    }

    // Fallback: return raw output as a summary
    TestRunSummary {
        framework: TestFramework::Jest,
        duration_ms: None,
        tests: Vec::new(),
        raw_output: Some(output.to_string()),
    }
}

fn parse_jest_json(json: &serde_json::Value) -> TestRunSummary {
    let mut tests = Vec::new();

    // Parse test suites
    if let Some(test_results) = json.get("testResults").and_then(|v| v.as_array()) {
        for suite in test_results {
            if let Some(assertion_results) =
                suite.get("assertionResults").and_then(|v| v.as_array())
            {
                for test in assertion_results {
                    let title = test
                        .get("fullName")
                        .or_else(|| test.get("title"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string();

                    let status_str = test.get("status").and_then(|v| v.as_str()).unwrap_or("");
                    let status = match status_str {
                        "passed" => TestStatus::Passed,
                        "failed" => TestStatus::Failed,
                        "pending" | "skipped" | "todo" => TestStatus::Skipped,
                        _ => TestStatus::Error,
                    };

                    let duration = test.get("duration").and_then(|v| v.as_u64());

                    let message = if status == TestStatus::Failed {
                        test.get("failureMessages")
                            .and_then(|v| v.as_array())
                            .map(|msgs| {
                                msgs.iter()
                                    .filter_map(|m| m.as_str())
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            })
                    } else {
                        None
                    };

                    tests.push(TestResult {
                        name: title,
                        status,
                        duration_ms: duration,
                        message,
                    });
                }
            }
        }
    }

    TestRunSummary {
        framework: TestFramework::Jest,
        duration_ms: None,
        tests,
        raw_output: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_jest_json_output() {
        let json = r#"{
            "numTotalTests": 3,
            "numPassedTests": 2,
            "numFailedTests": 1,
            "numPendingTests": 0,
            "testResults": [
                {
                    "assertionResults": [
                        {"fullName": "adds 1 + 2", "status": "passed", "duration": 5},
                        {"fullName": "subtracts", "status": "passed", "duration": 3},
                        {"fullName": "divides by zero", "status": "failed", "duration": 2, "failureMessages": ["Error: Division by zero"]}
                    ]
                }
            ]
        }"#;

        let summary = parse_jest_output(json);
        assert_eq!(summary.framework, TestFramework::Jest);
        assert_eq!(summary.total(), 3);
        assert_eq!(summary.passed(), 2);
        assert_eq!(summary.failed(), 1);
        assert_eq!(summary.tests.len(), 3);

        let failed = summary
            .tests
            .iter()
            .find(|t| t.name == "divides by zero")
            .unwrap();
        assert_eq!(failed.status, TestStatus::Failed);
        assert!(failed
            .message
            .as_ref()
            .unwrap()
            .contains("Division by zero"));
    }

    #[test]
    fn test_parse_jest_with_prefix() {
        let output = "Some warning text\n{\"numTotalTests\": 1, \"numPassedTests\": 1, \"numFailedTests\": 0, \"numPendingTests\": 0, \"testResults\": [{\"assertionResults\": [{\"fullName\": \"test1\", \"status\": \"passed\"}]}]}";
        let summary = parse_jest_output(output);
        assert_eq!(summary.total(), 1);
        assert_eq!(summary.passed(), 1);
    }

    #[test]
    fn test_parse_jest_invalid() {
        let summary = parse_jest_output("not json at all");
        assert_eq!(summary.total(), 0);
        assert!(summary.raw_output.is_some());
    }

    #[test]
    fn test_parse_jest_pending_tests() {
        let json = r#"{
            "numTotalTests": 2,
            "numPassedTests": 1,
            "numFailedTests": 0,
            "numPendingTests": 1,
            "testResults": [
                {
                    "assertionResults": [
                        {"fullName": "test1", "status": "passed", "duration": 1},
                        {"fullName": "test2", "status": "pending"}
                    ]
                }
            ]
        }"#;
        let summary = parse_jest_output(json);
        assert_eq!(summary.skipped(), 1);
        let skipped = summary.tests.iter().find(|t| t.name == "test2").unwrap();
        assert_eq!(skipped.status, TestStatus::Skipped);
    }
}
