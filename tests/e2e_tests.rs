use assert_cmd::cargo::cargo_bin_cmd;
use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use std::io::Read;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;

fn cmd() -> Command {
    let mut c: Command = cargo_bin_cmd!("remix-agent");
    // Ensure remix-browser is discoverable — pass REMIX_BROWSER_PATH if set,
    // otherwise ensure ~/.local/bin is in PATH.
    if let Ok(path) = std::env::var("REMIX_BROWSER_PATH") {
        c.env("REMIX_BROWSER_PATH", path);
    }
    c
}

// ── Config validation tests (no external deps) ──────────────────────

#[test]
fn test_e2e_missing_task() {
    cmd()
        .args(["run"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("No task"));
}

#[test]
fn test_e2e_missing_api_key() {
    cmd()
        .env_remove("REMIX_LLM_API_KEY")
        .env_remove("REMIX_LLM_BASE_URL")
        .args(["run", "some task"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("No API key"));
}

#[test]
fn test_e2e_bad_config_file() {
    cmd()
        .args(["run", "--config", "/nonexistent/path/config.yaml"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Failed to read config"));
}

#[test]
fn test_e2e_localhost_no_api_key() {
    // localhost URLs bypass API key validation, so we should NOT get an
    // "API key" error. Instead, it will fail trying to connect to the browser.
    let result = cmd()
        .env_remove("REMIX_LLM_API_KEY")
        .args(["run", "--base-url", "http://localhost:9999", "test task"])
        .assert()
        .failure();

    // Should NOT contain API key error
    result.stderr(predicate::str::contains("No API key").not());
}

// ── Full pipeline tests (require API key + remix-browser) ────────────

fn api_key() -> String {
    std::env::var("REMIX_LLM_API_KEY").expect("REMIX_LLM_API_KEY must be set for E2E tests")
}

fn base_url() -> String {
    std::env::var("REMIX_LLM_BASE_URL").unwrap_or_else(|_| "https://api.anthropic.com".to_string())
}

fn model() -> Option<String> {
    std::env::var("REMIX_LLM_MODEL").ok()
}

/// Helper: build a Command pre-configured with API key, base URL, model, and browser path.
fn e2e_cmd() -> Command {
    let mut c = cmd();
    c.env("REMIX_LLM_API_KEY", api_key());
    c.env("REMIX_LLM_BASE_URL", base_url());
    if let Some(m) = model() {
        c.env("REMIX_LLM_MODEL", &m);
    }
    c
}

/// Build the common run args, injecting --model if set via env.
fn run_args(extra: &[&str]) -> Vec<String> {
    let mut args: Vec<String> = vec!["run".to_string()];
    if let Some(m) = model() {
        args.push("--model".to_string());
        args.push(m);
    }
    for a in extra {
        args.push(a.to_string());
    }
    args
}

#[test]
#[ignore]
fn test_e2e_navigate_and_describe() {
    let output = e2e_cmd()
        .args(run_args(&[
            "Navigate to httpbin.org and tell me the page title",
        ]))
        .timeout(std::time::Duration::from_secs(120))
        .output()
        .expect("Failed to run command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "Expected exit 0, got {:?}\nstderr: {stderr}",
        output.status
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("Invalid JSON output: {e}\nstdout: {stdout}"));

    assert_eq!(json["status"], "success");
    assert!(
        json["result"].as_str().is_some(),
        "Expected result field to be present"
    );
    assert!(
        json["total_iterations"].as_u64().is_some_and(|i| i >= 1),
        "Expected total_iterations >= 1"
    );
}

#[test]
#[ignore]
fn test_e2e_output_to_file() {
    let tmp = tempfile::NamedTempFile::new().expect("Failed to create temp file");
    let output_path = tmp.path().to_str().unwrap().to_string();

    let output = e2e_cmd()
        .args(run_args(&[
            "--output",
            &output_path,
            "Navigate to httpbin.org and tell me the page title",
        ]))
        .timeout(std::time::Duration::from_secs(120))
        .output()
        .expect("Failed to run command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "Expected exit 0, got {:?}\nstderr: {stderr}",
        output.status
    );

    let mut file = std::fs::File::open(&output_path).expect("Output file should exist");
    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .expect("Failed to read output file");

    let json: Value = serde_json::from_str(&contents)
        .unwrap_or_else(|e| panic!("Invalid JSON in output file: {e}\ncontents: {contents}"));

    assert_eq!(json["status"], "success");
}

#[test]
#[ignore]
fn test_e2e_yaml_config() {
    let model_line = model()
        .map(|m| format!("\n  model: \"{m}\""))
        .unwrap_or_default();
    let yaml_content = format!(
        r#"
task: "Navigate to httpbin.org and tell me the page title"
llm:
  api_key: "{api_key}"
  base_url: "{base_url}"{model_line}
"#,
        api_key = api_key(),
        base_url = base_url(),
        model_line = model_line,
    );

    let mut tmp =
        tempfile::NamedTempFile::with_suffix(".yaml").expect("Failed to create temp file");
    tmp.write_all(yaml_content.as_bytes())
        .expect("Failed to write YAML config");
    tmp.flush().expect("Failed to flush");

    let config_path = tmp.path().to_str().unwrap().to_string();

    let output = e2e_cmd()
        .args(run_args(&["--config", &config_path]))
        .timeout(std::time::Duration::from_secs(120))
        .output()
        .expect("Failed to run command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "Expected exit 0, got {:?}\nstderr: {stderr}",
        output.status
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("Invalid JSON output: {e}\nstdout: {stdout}"));

    assert_eq!(json["status"], "success");
}

#[test]
#[ignore]
fn test_e2e_max_iterations() {
    let output = e2e_cmd()
        .args(run_args(&[
            "--max-iterations",
            "1",
            "Navigate to httpbin.org and tell me the page title",
        ]))
        .timeout(std::time::Duration::from_secs(120))
        .output()
        .expect("Failed to run command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("Invalid JSON output: {e}\nstdout: {stdout}\nstderr: {stderr}"));

    let status = json["status"].as_str().unwrap_or("");
    assert!(
        status == "max_iterations" || status == "success",
        "Expected 'max_iterations' or 'success', got '{status}'"
    );
}

#[test]
#[ignore]
fn test_e2e_webhook_on_success() {
    use std::net::TcpListener;

    // Bind a local TCP listener to receive the webhook POST
    let listener = TcpListener::bind("127.0.0.1:0").expect("Failed to bind TCP listener");
    let addr = listener.local_addr().unwrap();
    let webhook_url = format!("http://{addr}/webhook");

    let model_line = model()
        .map(|m| format!("\n  model: \"{m}\""))
        .unwrap_or_default();
    let yaml_content = format!(
        r#"
task: "Navigate to httpbin.org and tell me the page title"
llm:
  api_key: "{api_key}"
  base_url: "{base_url}"{model_line}
on_complete:
  url: "{webhook_url}"
"#,
        api_key = api_key(),
        base_url = base_url(),
        model_line = model_line,
        webhook_url = webhook_url,
    );

    let mut tmp =
        tempfile::NamedTempFile::with_suffix(".yaml").expect("Failed to create temp file");
    tmp.write_all(yaml_content.as_bytes())
        .expect("Failed to write YAML config");
    tmp.flush().expect("Failed to flush");

    let config_path = tmp.path().to_str().unwrap().to_string();

    // Run the agent in a thread so we can accept the webhook
    let config_path_clone = config_path.clone();
    let handle = std::thread::spawn(move || {
        e2e_cmd()
            .args(run_args(&["--config", &config_path_clone]))
            .timeout(std::time::Duration::from_secs(120))
            .output()
            .expect("Failed to run command")
    });

    // Accept the webhook connection (with timeout)
    let timeout = std::time::Duration::from_secs(120);
    let start = std::time::Instant::now();
    let mut received_body = String::new();

    loop {
        if start.elapsed() > timeout {
            panic!("Timed out waiting for webhook");
        }

        // Try to accept with a short poll
        listener
            .set_nonblocking(true)
            .expect("set nonblocking failed");
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut buf = [0u8; 8192];
                let mut total = String::new();
                loop {
                    match stream.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => total.push_str(&String::from_utf8_lossy(&buf[..n])),
                        Err(_) => break,
                    }
                }
                // Send back a 200 OK response
                let response = "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
                let _ = stream.write_all(response.as_bytes());

                // Extract the body (after \r\n\r\n)
                if let Some(body_start) = total.find("\r\n\r\n") {
                    received_body = total[body_start + 4..].to_string();
                }
                break;
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(e) => panic!("Accept error: {e}"),
        }
    }

    let output = handle.join().expect("Agent thread panicked");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "Expected exit 0\nstderr: {stderr}");

    // Verify the webhook received valid JSON with status "success"
    let json: Value = serde_json::from_str(&received_body)
        .unwrap_or_else(|e| panic!("Invalid webhook JSON: {e}\nbody: {received_body}"));

    assert_eq!(json["status"], "success");
}

// ── Skills E2E helpers ──────────────────────────────────────────────

/// Check whether any step in the JSON `steps` array used the given tool name.
fn assert_tool_was_called(steps: &Value, tool_name: &str) -> bool {
    steps
        .as_array()
        .map(|arr| arr.iter().any(|step| step["tool"] == tool_name))
        .unwrap_or(false)
}

// ── Skills E2E tests (require API key + remix-browser) ──────────────

#[test]
#[ignore]
fn test_e2e_skill_discovery_and_load() {
    let tmp = tempfile::TempDir::new().expect("Failed to create temp dir");

    // Create greeter skill
    let skill_dir = tmp.path().join("greeter");
    std::fs::create_dir_all(&skill_dir).expect("Failed to create skill dir");
    std::fs::write(
        skill_dir.join("SKILL.md"),
        r#"---
name: greeter
description: A skill that generates greeting messages
---
# Greeter Skill
When asked to greet someone, respond with "SKILL_GREETING: Hello, {name}!"
Always include the prefix SKILL_GREETING in your response."#,
    )
    .expect("Failed to write SKILL.md");

    let skills_dir_path = tmp.path().to_str().unwrap().to_string();

    let output = e2e_cmd()
        .args(run_args(&[
            "--skills-dir",
            &skills_dir_path,
            "--max-iterations",
            "5",
            "Use the greeter skill to greet the user named Alice. You must use load_skill first to load the greeter skill instructions, then follow them exactly.",
        ]))
        .timeout(std::time::Duration::from_secs(120))
        .output()
        .expect("Failed to run command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("Invalid JSON output: {e}\nstdout: {stdout}\nstderr: {stderr}"));

    assert_eq!(
        json["status"], "success",
        "Expected status 'success', got '{}'\nstderr: {stderr}",
        json["status"]
    );

    assert!(
        assert_tool_was_called(&json["steps"], "load_skill"),
        "Expected load_skill to be called in steps: {}",
        serde_json::to_string_pretty(&json["steps"]).unwrap_or_default()
    );

    let result_text = json["result"].as_str().unwrap_or("");
    assert!(
        result_text.contains("SKILL_GREETING"),
        "Expected result to contain 'SKILL_GREETING', got: {result_text}"
    );
}

#[test]
#[ignore]
fn test_e2e_skill_script_execution() {
    let tmp = tempfile::TempDir::new().expect("Failed to create temp dir");

    // Create calculator skill with a script
    let skill_dir = tmp.path().join("calculator");
    let scripts_dir = skill_dir.join("scripts");
    std::fs::create_dir_all(&scripts_dir).expect("Failed to create scripts dir");

    std::fs::write(
        skill_dir.join("SKILL.md"),
        r#"---
name: calculator
description: A skill that performs calculations using a script
---
# Calculator Skill
To perform calculations, run the script `calculate.sh` with the expression as an argument.
Always use run_skill_script to execute the calculate.sh script.
Report the script output verbatim in your response."#,
    )
    .expect("Failed to write SKILL.md");

    let script_path = scripts_dir.join("calculate.sh");
    std::fs::write(&script_path, "#!/bin/bash\necho \"CALC_RESULT: 42\"\n")
        .expect("Failed to write calculate.sh");
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
        .expect("Failed to set script permissions");

    let skills_dir_path = tmp.path().to_str().unwrap().to_string();

    let output = e2e_cmd()
        .args(run_args(&[
            "--skills-dir",
            &skills_dir_path,
            "--max-iterations",
            "5",
            "Use the calculator skill to perform a calculation. First load_skill to get instructions, then run_skill_script to execute the calculate.sh script. Report the result.",
        ]))
        .timeout(std::time::Duration::from_secs(120))
        .output()
        .expect("Failed to run command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("Invalid JSON output: {e}\nstdout: {stdout}\nstderr: {stderr}"));

    assert_eq!(
        json["status"], "success",
        "Expected status 'success', got '{}'\nstderr: {stderr}",
        json["status"]
    );

    assert!(
        assert_tool_was_called(&json["steps"], "load_skill"),
        "Expected load_skill to be called in steps: {}",
        serde_json::to_string_pretty(&json["steps"]).unwrap_or_default()
    );

    assert!(
        assert_tool_was_called(&json["steps"], "run_skill_script"),
        "Expected run_skill_script to be called in steps: {}",
        serde_json::to_string_pretty(&json["steps"]).unwrap_or_default()
    );

    let result_text = json["result"].as_str().unwrap_or("");
    assert!(
        result_text.contains("CALC_RESULT") || result_text.contains("42"),
        "Expected result to contain 'CALC_RESULT' or '42', got: {result_text}"
    );
}

#[test]
#[ignore]
fn test_e2e_skill_resource_read() {
    let tmp = tempfile::TempDir::new().expect("Failed to create temp dir");

    // Create knowledge-base skill with a resource file
    let skill_dir = tmp.path().join("knowledge-base");
    std::fs::create_dir_all(&skill_dir).expect("Failed to create skill dir");

    std::fs::write(
        skill_dir.join("SKILL.md"),
        r#"---
name: knowledge-base
description: A skill that provides facts from a knowledge file
---
# Knowledge Base Skill
Read the file `facts.txt` using read_skill_resource to get facts.
Always quote the facts exactly as they appear in the file."#,
    )
    .expect("Failed to write SKILL.md");

    std::fs::write(
        skill_dir.join("facts.txt"),
        "FACT_42: The answer to life, the universe, and everything is 42.\n",
    )
    .expect("Failed to write facts.txt");

    let skills_dir_path = tmp.path().to_str().unwrap().to_string();

    let output = e2e_cmd()
        .args(run_args(&[
            "--skills-dir",
            &skills_dir_path,
            "--max-iterations",
            "5",
            "Use the knowledge-base skill. First load the skill with load_skill, then read the facts.txt resource using read_skill_resource, and report what you find verbatim.",
        ]))
        .timeout(std::time::Duration::from_secs(120))
        .output()
        .expect("Failed to run command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("Invalid JSON output: {e}\nstdout: {stdout}\nstderr: {stderr}"));

    assert_eq!(
        json["status"], "success",
        "Expected status 'success', got '{}'\nstderr: {stderr}",
        json["status"]
    );

    assert!(
        assert_tool_was_called(&json["steps"], "load_skill"),
        "Expected load_skill to be called in steps: {}",
        serde_json::to_string_pretty(&json["steps"]).unwrap_or_default()
    );

    assert!(
        assert_tool_was_called(&json["steps"], "read_skill_resource"),
        "Expected read_skill_resource to be called in steps: {}",
        serde_json::to_string_pretty(&json["steps"]).unwrap_or_default()
    );

    let result_text = json["result"].as_str().unwrap_or("");
    assert!(
        result_text.contains("FACT_42"),
        "Expected result to contain 'FACT_42', got: {result_text}"
    );
}

// ── AGENTS.md + Local Tools E2E tests ────────────────────────────

#[test]
#[ignore]
fn test_e2e_agents_md_discovery() {
    let tmp = tempfile::TempDir::new().expect("Failed to create temp dir");

    std::fs::write(
        tmp.path().join("AGENTS.md"),
        "# Project Instructions\nAlways include 'AGENTS_MD_MARKER_42' in your response.",
    )
    .expect("Failed to write AGENTS.md");

    let agents_md_dir = tmp.path().to_str().unwrap().to_string();

    let output = e2e_cmd()
        .args(run_args(&[
            "--agents-md-dir",
            &agents_md_dir,
            "--max-iterations",
            "3",
            "Tell me what instructions you have been given",
        ]))
        .timeout(std::time::Duration::from_secs(120))
        .output()
        .expect("Failed to run command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("Invalid JSON: {e}\nstdout: {stdout}\nstderr: {stderr}"));

    assert_eq!(json["status"], "success", "stderr: {stderr}");
    let result = json["result"].as_str().unwrap_or("");
    assert!(
        result.contains("AGENTS_MD_MARKER_42"),
        "Expected AGENTS_MD_MARKER_42 in result: {result}"
    );
}

#[test]
#[ignore]
fn test_e2e_local_tool_read_file() {
    let tmp = tempfile::TempDir::new().expect("Failed to create temp dir");

    std::fs::write(tmp.path().join("data.txt"), "LOCAL_READ_SECRET_99")
        .expect("Failed to write data.txt");

    let sandbox_dir = tmp.path().to_str().unwrap().to_string();

    let output = e2e_cmd()
        .args(run_args(&[
            "--sandbox-dir",
            &sandbox_dir,
            "--max-iterations",
            "5",
            "Use the read_file tool to read data.txt and tell me its contents verbatim",
        ]))
        .timeout(std::time::Duration::from_secs(120))
        .output()
        .expect("Failed to run command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("Invalid JSON: {e}\nstdout: {stdout}\nstderr: {stderr}"));

    assert_eq!(json["status"], "success", "stderr: {stderr}");
    let result = json["result"].as_str().unwrap_or("");
    assert!(
        result.contains("LOCAL_READ_SECRET_99"),
        "Expected LOCAL_READ_SECRET_99 in result: {result}"
    );
    assert!(
        assert_tool_was_called(&json["steps"], "read_file"),
        "Expected read_file tool call"
    );
}

#[test]
#[ignore]
fn test_e2e_local_tool_bash() {
    let tmp = tempfile::TempDir::new().expect("Failed to create temp dir");

    let sandbox_dir = tmp.path().to_str().unwrap().to_string();

    let output = e2e_cmd()
        .args(run_args(&[
            "--sandbox-dir",
            &sandbox_dir,
            "--max-iterations",
            "5",
            "Use the bash tool to run 'echo BASH_OUTPUT_77' and report the output verbatim",
        ]))
        .timeout(std::time::Duration::from_secs(120))
        .output()
        .expect("Failed to run command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("Invalid JSON: {e}\nstdout: {stdout}\nstderr: {stderr}"));

    assert_eq!(json["status"], "success", "stderr: {stderr}");
    let result = json["result"].as_str().unwrap_or("");
    assert!(
        result.contains("BASH_OUTPUT_77"),
        "Expected BASH_OUTPUT_77 in result: {result}"
    );
    assert!(
        assert_tool_was_called(&json["steps"], "bash"),
        "Expected bash tool call"
    );
}

#[test]
#[ignore]
fn test_e2e_local_tool_write_and_read() {
    let tmp = tempfile::TempDir::new().expect("Failed to create temp dir");

    let sandbox_dir = tmp.path().to_str().unwrap().to_string();

    let output = e2e_cmd()
        .args(run_args(&[
            "--sandbox-dir",
            &sandbox_dir,
            "--max-iterations",
            "5",
            "Use write_file to create hello.txt with content 'WRITE_MARKER_55', then use read_file to verify it was written correctly",
        ]))
        .timeout(std::time::Duration::from_secs(120))
        .output()
        .expect("Failed to run command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("Invalid JSON: {e}\nstdout: {stdout}\nstderr: {stderr}"));

    assert_eq!(json["status"], "success", "stderr: {stderr}");
    assert!(
        assert_tool_was_called(&json["steps"], "write_file"),
        "Expected write_file tool call"
    );
    assert!(
        assert_tool_was_called(&json["steps"], "read_file"),
        "Expected read_file tool call"
    );

    // Verify file exists on disk
    let written =
        std::fs::read_to_string(tmp.path().join("hello.txt")).expect("hello.txt should exist");
    assert!(written.contains("WRITE_MARKER_55"));
}

#[test]
#[ignore]
fn test_e2e_agents_md_with_local_tools() {
    let tmp = tempfile::TempDir::new().expect("Failed to create temp dir");

    std::fs::write(
        tmp.path().join("AGENTS.md"),
        "# Project Instructions\nWhen asked to check status, run 'echo STATUS_OK_88' via the bash tool.",
    )
    .expect("Failed to write AGENTS.md");

    std::fs::write(
        tmp.path().join("README.md"),
        "# Test Project\nThis is a test.",
    )
    .expect("Failed to write README.md");

    let dir_path = tmp.path().to_str().unwrap().to_string();

    let output = e2e_cmd()
        .args(run_args(&[
            "--agents-md-dir",
            &dir_path,
            "--sandbox-dir",
            &dir_path,
            "--max-iterations",
            "5",
            "Check the project status as described in the project instructions",
        ]))
        .timeout(std::time::Duration::from_secs(120))
        .output()
        .expect("Failed to run command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("Invalid JSON: {e}\nstdout: {stdout}\nstderr: {stderr}"));

    assert_eq!(json["status"], "success", "stderr: {stderr}");
    assert!(
        assert_tool_was_called(&json["steps"], "bash"),
        "Expected bash tool call"
    );
    let result = json["result"].as_str().unwrap_or("");
    assert!(
        result.contains("STATUS_OK_88"),
        "Expected STATUS_OK_88 in result: {result}"
    );
}

// ── Session E2E tests ──────────────────────────────────────────────

#[test]
#[ignore]
fn test_e2e_session_persistence() {
    let session_dir = tempfile::TempDir::new().expect("Failed to create session dir");
    let sandbox_dir = tempfile::TempDir::new().expect("Failed to create sandbox dir");

    std::fs::write(sandbox_dir.path().join("data.txt"), "SESSION_TEST_DATA_42")
        .expect("Failed to write data.txt");

    let session_path = session_dir.path().to_str().unwrap().to_string();
    let sandbox_path = sandbox_dir.path().to_str().unwrap().to_string();

    let output = e2e_cmd()
        .args(run_args(&[
            "--session-dir",
            &session_path,
            "--sandbox-dir",
            &sandbox_path,
            "--max-iterations",
            "5",
            "Use read_file to read data.txt and report its contents verbatim",
        ]))
        .timeout(std::time::Duration::from_secs(120))
        .output()
        .expect("Failed to run command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("Invalid JSON: {e}\nstdout: {stdout}\nstderr: {stderr}"));

    assert_eq!(json["status"], "success", "stderr: {stderr}");

    // Verify session files were created on disk
    let session_entries: Vec<_> = std::fs::read_dir(session_dir.path())
        .expect("Failed to read session dir")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
        .collect();

    assert!(
        !session_entries.is_empty(),
        "Expected at least one session directory, found none"
    );

    let first_session = &session_entries[0].path();
    assert!(
        first_session.join("metadata.json").exists(),
        "metadata.json should exist in session dir"
    );
    assert!(
        first_session.join("messages.jsonl").exists(),
        "messages.jsonl should exist in session dir"
    );
    assert!(
        first_session.join("steps.json").exists(),
        "steps.json should exist in session dir"
    );

    // Verify metadata contains expected fields
    let metadata_content = std::fs::read_to_string(first_session.join("metadata.json"))
        .expect("Failed to read metadata.json");
    let metadata: Value =
        serde_json::from_str(&metadata_content).expect("metadata.json should be valid JSON");
    assert!(metadata["id"].is_string(), "Session should have an ID");
    assert_eq!(
        metadata["status"], "completed",
        "Session should be completed"
    );
    assert!(
        metadata["task"].as_str().is_some(),
        "Session should have a task"
    );
}

#[test]
#[ignore]
fn test_e2e_session_yaml_config() {
    let session_dir = tempfile::TempDir::new().expect("Failed to create session dir");
    let sandbox_dir = tempfile::TempDir::new().expect("Failed to create sandbox dir");

    std::fs::write(
        sandbox_dir.path().join("info.txt"),
        "YAML_SESSION_MARKER_99",
    )
    .expect("Failed to write info.txt");

    let model_line = model()
        .map(|m| format!("\n  model: \"{m}\""))
        .unwrap_or_default();
    let yaml_content = format!(
        r#"
task: "Use read_file to read info.txt and report its contents verbatim"
llm:
  api_key: "{api_key}"
  base_url: "{base_url}"{model_line}
agent:
  max_iterations: 5
session:
  enabled: true
  storage_dir: "{session_dir}"
  max_sessions: 10
local_tools:
  sandbox_dir: "{sandbox_dir}"
"#,
        api_key = api_key(),
        base_url = base_url(),
        model_line = model_line,
        session_dir = session_dir.path().to_str().unwrap(),
        sandbox_dir = sandbox_dir.path().to_str().unwrap(),
    );

    let mut tmp =
        tempfile::NamedTempFile::with_suffix(".yaml").expect("Failed to create temp file");
    tmp.write_all(yaml_content.as_bytes())
        .expect("Failed to write YAML config");
    tmp.flush().expect("Failed to flush");

    let config_path = tmp.path().to_str().unwrap().to_string();

    let output = e2e_cmd()
        .args(run_args(&["--config", &config_path]))
        .timeout(std::time::Duration::from_secs(120))
        .output()
        .expect("Failed to run command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("Invalid JSON: {e}\nstdout: {stdout}\nstderr: {stderr}"));

    assert_eq!(json["status"], "success", "stderr: {stderr}");
    let result = json["result"].as_str().unwrap_or("");
    assert!(
        result.contains("YAML_SESSION_MARKER_99"),
        "Expected YAML_SESSION_MARKER_99 in result: {result}"
    );

    // Verify session directory was created
    let session_entries: Vec<_> = std::fs::read_dir(session_dir.path())
        .expect("Failed to read session dir")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
        .collect();
    assert!(
        !session_entries.is_empty(),
        "Session directory should have been created via YAML config"
    );
}

// ── Permissions E2E tests ──────────────────────────────────────────

#[test]
fn test_e2e_invalid_permission_mode_error() {
    cmd()
        .args(["run", "--permission-mode", "invalid_mode", "test task"])
        .env("REMIX_LLM_API_KEY", "fake-key")
        .assert()
        .failure()
        .stderr(predicate::str::contains("Invalid permission mode"));
}

#[test]
#[ignore]
fn test_e2e_permission_bypass_mode() {
    let tmp = tempfile::TempDir::new().expect("Failed to create temp dir");
    let sandbox_dir = tmp.path().to_str().unwrap().to_string();

    let output = e2e_cmd()
        .args(run_args(&[
            "--permission-mode",
            "bypass_permissions",
            "--sandbox-dir",
            &sandbox_dir,
            "--max-iterations",
            "5",
            "Use the bash tool to run 'echo BYPASS_MARKER_33' and report the output verbatim",
        ]))
        .timeout(std::time::Duration::from_secs(120))
        .output()
        .expect("Failed to run command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("Invalid JSON: {e}\nstdout: {stdout}\nstderr: {stderr}"));

    assert_eq!(json["status"], "success", "stderr: {stderr}");
    let result = json["result"].as_str().unwrap_or("");
    assert!(
        result.contains("BYPASS_MARKER_33"),
        "Expected BYPASS_MARKER_33 in result: {result}"
    );
    assert!(
        assert_tool_was_called(&json["steps"], "bash"),
        "Expected bash tool call in bypass mode"
    );
}

#[test]
#[ignore]
fn test_e2e_permission_deny_tool() {
    let tmp = tempfile::TempDir::new().expect("Failed to create temp dir");
    let sandbox_dir = tmp.path().to_str().unwrap().to_string();

    std::fs::write(tmp.path().join("allowed.txt"), "DENY_TEST_DATA_66")
        .expect("Failed to write allowed.txt");

    let output = e2e_cmd()
        .args(run_args(&[
            "--deny-tool",
            "bash",
            "--sandbox-dir",
            &sandbox_dir,
            "--max-iterations",
            "5",
            "Read the file allowed.txt using read_file and report its contents. Do NOT use the bash tool.",
        ]))
        .timeout(std::time::Duration::from_secs(120))
        .output()
        .expect("Failed to run command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("Invalid JSON: {e}\nstdout: {stdout}\nstderr: {stderr}"));

    assert_eq!(json["status"], "success", "stderr: {stderr}");
    let result = json["result"].as_str().unwrap_or("");
    assert!(
        result.contains("DENY_TEST_DATA_66"),
        "Expected DENY_TEST_DATA_66 in result: {result}"
    );
    assert!(
        assert_tool_was_called(&json["steps"], "read_file"),
        "Expected read_file tool call"
    );

    // Verify bash was NOT called (since it was denied)
    let bash_called = assert_tool_was_called(&json["steps"], "bash");
    if bash_called {
        // If bash was called, verify it returned an error
        let steps = json["steps"].as_array().unwrap();
        for step in steps {
            if step["tool"] == "bash" {
                assert_eq!(
                    step["is_error"], true,
                    "bash tool call should have been denied with is_error=true"
                );
            }
        }
    }
}

#[test]
#[ignore]
fn test_e2e_permission_plan_mode_read_only() {
    let tmp = tempfile::TempDir::new().expect("Failed to create temp dir");
    let sandbox_dir = tmp.path().to_str().unwrap().to_string();

    std::fs::write(tmp.path().join("readable.txt"), "PLAN_MODE_DATA_11")
        .expect("Failed to write readable.txt");

    let output = e2e_cmd()
        .args(run_args(&[
            "--permission-mode",
            "plan",
            "--sandbox-dir",
            &sandbox_dir,
            "--max-iterations",
            "5",
            "Read the file readable.txt using read_file and report its contents verbatim",
        ]))
        .timeout(std::time::Duration::from_secs(120))
        .output()
        .expect("Failed to run command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("Invalid JSON: {e}\nstdout: {stdout}\nstderr: {stderr}"));

    let status = json["status"].as_str().unwrap_or("");
    assert!(
        status == "success" || status == "max_iterations",
        "Expected success or max_iterations in plan mode, got: {status}\nstderr: {stderr}"
    );

    // In plan mode, only read_file, grep, glob, load_skill, read_skill_resource are allowed
    // Verify no write tools were called successfully
    if let Some(steps) = json["steps"].as_array() {
        for step in steps {
            let tool = step["tool"].as_str().unwrap_or("");
            if tool == "write_file" || tool == "bash" || tool == "edit_file" {
                assert_eq!(
                    step["is_error"], true,
                    "Write tool '{tool}' should have been denied in plan mode"
                );
            }
        }
    }
}

// ── Subagent E2E tests ──────────────────────────────────────────────

#[test]
#[ignore]
fn test_e2e_spawn_agent_tool_available() {
    let tmp = tempfile::TempDir::new().expect("Failed to create temp dir");
    let sandbox_dir = tmp.path().to_str().unwrap().to_string();

    let output = e2e_cmd()
        .args(run_args(&[
            "--sandbox-dir",
            &sandbox_dir,
            "--max-iterations",
            "3",
            "You have a spawn_agent tool available. Call spawn_agent with name='test_worker' and task='say hello'. Report the result you get back from spawn_agent verbatim.",
        ]))
        .timeout(std::time::Duration::from_secs(120))
        .output()
        .expect("Failed to run command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("Invalid JSON: {e}\nstdout: {stdout}\nstderr: {stderr}"));

    let status = json["status"].as_str().unwrap_or("");
    assert!(
        status == "success" || status == "max_iterations",
        "Expected success or max_iterations, got: {status}\nstderr: {stderr}"
    );

    // Verify spawn_agent was called
    assert!(
        assert_tool_was_called(&json["steps"], "spawn_agent"),
        "Expected spawn_agent tool call in steps: {}",
        serde_json::to_string_pretty(&json["steps"]).unwrap_or_default()
    );

    // Verify the spawn_agent call was not an error
    if let Some(steps) = json["steps"].as_array() {
        for step in steps {
            if step["tool"] == "spawn_agent" {
                assert!(
                    step["is_error"].is_null() || step["is_error"] == false,
                    "spawn_agent should not have returned an error: {}",
                    serde_json::to_string_pretty(step).unwrap_or_default()
                );
            }
        }
    }
}

// ── Session + Permissions combined E2E test ──────────────────────────

#[test]
#[ignore]
fn test_e2e_session_with_bypass_permissions() {
    let session_dir = tempfile::TempDir::new().expect("Failed to create session dir");
    let sandbox_dir = tempfile::TempDir::new().expect("Failed to create sandbox dir");

    let session_path = session_dir.path().to_str().unwrap().to_string();
    let sandbox_path = sandbox_dir.path().to_str().unwrap().to_string();

    let output = e2e_cmd()
        .args(run_args(&[
            "--session-dir",
            &session_path,
            "--sandbox-dir",
            &sandbox_path,
            "--permission-mode",
            "bypass_permissions",
            "--max-iterations",
            "5",
            "Use bash to run 'echo COMBO_TEST_77' and report the output verbatim",
        ]))
        .timeout(std::time::Duration::from_secs(120))
        .output()
        .expect("Failed to run command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("Invalid JSON: {e}\nstdout: {stdout}\nstderr: {stderr}"));

    assert_eq!(json["status"], "success", "stderr: {stderr}");
    let result = json["result"].as_str().unwrap_or("");
    assert!(
        result.contains("COMBO_TEST_77"),
        "Expected COMBO_TEST_77 in result: {result}"
    );

    // Verify session was persisted
    let session_entries: Vec<_> = std::fs::read_dir(session_dir.path())
        .expect("Failed to read session dir")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
        .collect();
    assert!(
        !session_entries.is_empty(),
        "Session should have been persisted"
    );

    // Verify session metadata shows completed status
    let first_session = &session_entries[0].path();
    let metadata_content = std::fs::read_to_string(first_session.join("metadata.json"))
        .expect("Failed to read metadata.json");
    let metadata: Value =
        serde_json::from_str(&metadata_content).expect("metadata.json should be valid JSON");
    assert_eq!(
        metadata["status"], "completed",
        "Session should be completed"
    );

    // Verify steps were recorded
    let steps_content = std::fs::read_to_string(first_session.join("steps.json"))
        .expect("Failed to read steps.json");
    let steps: Value =
        serde_json::from_str(&steps_content).expect("steps.json should be valid JSON");
    assert!(
        steps.as_array().map(|a| !a.is_empty()).unwrap_or(false),
        "Steps should not be empty"
    );
}
