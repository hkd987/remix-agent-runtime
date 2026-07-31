use assert_cmd::cargo::cargo_bin_cmd;
use assert_cmd::Command;
use predicates::prelude::*;

fn cmd() -> Command {
    cargo_bin_cmd!("remix-agent").into()
}

#[test]
fn test_help_output() {
    cmd()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("LLM-driven browser automation"));
}

#[test]
fn test_run_help_output() {
    cmd()
        .args(["run", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Run an agent task"));
}

#[test]
fn test_version_output() {
    cmd()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("remix-agent"));
}

#[test]
fn test_no_subcommand_fails() {
    cmd().assert().failure();
}

#[test]
fn test_run_no_task_fails() {
    // Running with no task and no config should fail with config error (exit 2)
    cmd()
        .args(["run"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("No task provided"));
}

#[test]
fn test_run_no_api_key_fails() {
    // Running with task but no API key should fail
    cmd()
        .env_remove("REMIX_LLM_API_KEY")
        .env_remove("REMIX_LLM_BASE_URL")
        .args(["run", "test task"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("No API key"));
}

#[test]
fn test_run_missing_config_file_fails() {
    cmd()
        .args(["run", "--config", "/nonexistent/file.yaml"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Failed to read config"));
}

/// The shipped example config must actually deserialize into `AppConfig`.
///
/// Nothing validated it, so it could rot silently — and with `deny_unknown_fields` now
/// in place, a stale key in the example would be an error rather than a no-op.
#[test]
fn example_benchmark_config_parses() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/examples/harness/coding-benchmark.yaml"
    );
    let yaml = std::fs::read_to_string(path).expect("example config should exist");
    let config: remix_agent_runtime::config::schema::AppConfig =
        serde_yaml::from_str(&yaml).expect("example config should deserialize");
    config
        .validate()
        .expect("example config should pass validation");
    assert!(config.task.is_some(), "example should define a task");
}
