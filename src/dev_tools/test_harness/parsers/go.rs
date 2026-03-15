use crate::dev_tools::test_harness::types::{
    TestFramework, TestResult, TestRunSummary, TestStatus,
};

/// Parse `go test -json` output into structured results.
/// Each line is a JSON object with Action, Test, Package, Output, Elapsed fields.
pub fn parse_go_test_output(output: &str) -> TestRunSummary {
    let mut tests = Vec::new();
    let mut test_outputs: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    let mut total_elapsed: Option<f64> = None;

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let event: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let action = event.get("Action").and_then(|v| v.as_str()).unwrap_or("");
        let test_name = event.get("Test").and_then(|v| v.as_str());
        let package = event.get("Package").and_then(|v| v.as_str()).unwrap_or("");

        match action {
            "pass" | "fail" | "skip" => {
                if let Some(name) = test_name {
                    let full_name = if package.is_empty() {
                        name.to_string()
                    } else {
                        format!("{}/{}", package, name)
                    };

                    let status = match action {
                        "pass" => TestStatus::Passed,
                        "fail" => TestStatus::Failed,
                        "skip" => TestStatus::Skipped,
                        _ => TestStatus::Error,
                    };

                    let elapsed = event.get("Elapsed").and_then(|v| v.as_f64());
                    let duration_ms = elapsed.map(|e| (e * 1000.0) as u64);

                    let message = test_outputs.remove(&full_name).map(|lines| lines.join(""));

                    tests.push(TestResult {
                        name: full_name,
                        status,
                        duration_ms,
                        message,
                    });
                } else if action == "pass" || action == "fail" {
                    // Package-level result
                    if let Some(elapsed) = event.get("Elapsed").and_then(|v| v.as_f64()) {
                        total_elapsed = Some(total_elapsed.unwrap_or(0.0) + elapsed);
                    }
                }
            }
            "output" => {
                if let Some(name) = test_name {
                    let full_name = if package.is_empty() {
                        name.to_string()
                    } else {
                        format!("{}/{}", package, name)
                    };
                    if let Some(output_text) = event.get("Output").and_then(|v| v.as_str()) {
                        test_outputs
                            .entry(full_name)
                            .or_default()
                            .push(output_text.to_string());
                    }
                }
            }
            _ => {}
        }
    }

    TestRunSummary {
        framework: TestFramework::Go,
        duration_ms: total_elapsed.map(|e| (e * 1000.0) as u64),
        tests,
        raw_output: Some(output.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_go_json_output() {
        let output = r#"{"Time":"2024-01-01T00:00:00Z","Action":"run","Package":"example.com/pkg","Test":"TestAdd"}
{"Time":"2024-01-01T00:00:00Z","Action":"output","Package":"example.com/pkg","Test":"TestAdd","Output":"=== RUN   TestAdd\n"}
{"Time":"2024-01-01T00:00:00Z","Action":"output","Package":"example.com/pkg","Test":"TestAdd","Output":"--- PASS: TestAdd (0.00s)\n"}
{"Time":"2024-01-01T00:00:00Z","Action":"pass","Package":"example.com/pkg","Test":"TestAdd","Elapsed":0.001}
{"Time":"2024-01-01T00:00:00Z","Action":"run","Package":"example.com/pkg","Test":"TestSub"}
{"Time":"2024-01-01T00:00:00Z","Action":"output","Package":"example.com/pkg","Test":"TestSub","Output":"=== RUN   TestSub\n"}
{"Time":"2024-01-01T00:00:00Z","Action":"output","Package":"example.com/pkg","Test":"TestSub","Output":"    sub_test.go:10: expected 1, got 2\n"}
{"Time":"2024-01-01T00:00:00Z","Action":"fail","Package":"example.com/pkg","Test":"TestSub","Elapsed":0.002}
{"Time":"2024-01-01T00:00:00Z","Action":"pass","Package":"example.com/pkg","Elapsed":0.5}"#;

        let summary = parse_go_test_output(output);
        assert_eq!(summary.framework, TestFramework::Go);
        assert_eq!(summary.total(), 2);
        assert_eq!(summary.passed(), 1);
        assert_eq!(summary.failed(), 1);

        let failed = summary
            .tests
            .iter()
            .find(|t| t.name.contains("TestSub"))
            .unwrap();
        assert_eq!(failed.status, TestStatus::Failed);
        assert!(failed
            .message
            .as_ref()
            .unwrap()
            .contains("expected 1, got 2"));
    }

    #[test]
    fn test_parse_go_skipped() {
        let output =
            r#"{"Action":"skip","Package":"example.com/pkg","Test":"TestSkipped","Elapsed":0.0}"#;
        let summary = parse_go_test_output(output);
        assert_eq!(summary.skipped(), 1);
    }

    #[test]
    fn test_parse_go_empty() {
        let summary = parse_go_test_output("");
        assert_eq!(summary.total(), 0);
    }

    #[test]
    fn test_parse_go_non_json_lines() {
        let output = "not json\n{\"Action\":\"pass\",\"Package\":\"pkg\",\"Test\":\"Test1\",\"Elapsed\":0.1}\nalso not json";
        let summary = parse_go_test_output(output);
        assert_eq!(summary.total(), 1);
        assert_eq!(summary.passed(), 1);
    }
}
