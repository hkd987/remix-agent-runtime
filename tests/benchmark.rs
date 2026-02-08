//! Benchmark comparison: remix-agent vs Claude Code CLI
//!
//! Runs identical tasks on both runtimes and compares performance metrics.
//!
//! Usage:
//! ```bash
//! export $(grep -v '^#' .env | xargs)
//! cargo test --test benchmark -- --ignored --nocapture --test-threads=1
//! ```

use assert_cmd::cargo::cargo_bin_cmd;
use assert_cmd::Command;
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;

// ── Metric structs ────────────────────────────────────────────────────

#[derive(Debug)]
struct RemixMetrics {
    wall_ms: u64,
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
    iterations: u32,
    tool_calls: usize,
    success: bool,
}

#[derive(Debug)]
struct ClaudeMetrics {
    wall_ms: u64,
    api_ms: u64,
    input_tokens: u64,
    output_tokens: u64,
    turns: u64,
    cost_usd: f64,
    success: bool,
}

#[derive(Debug)]
struct ComparisonRow {
    task_name: String,
    remix: Option<RemixMetrics>,
    claude: Option<ClaudeMetrics>,
}

// ── Claude Code JSON output shape ─────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ClaudeCodeOutput {
    #[serde(rename = "type")]
    type_field: Option<String>,
    subtype: Option<String>,
    duration_ms: Option<u64>,
    duration_api_ms: Option<u64>,
    num_turns: Option<u64>,
    total_cost_usd: Option<f64>,
    is_error: Option<bool>,
    usage: Option<ClaudeUsage>,
}

#[derive(Debug, Deserialize)]
struct ClaudeUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
}

// ── Task definition ───────────────────────────────────────────────────

struct BenchmarkTask {
    name: &'static str,
    prompt: &'static str,
    max_iterations: u32,
    setup_fn: fn(&Path),
}

fn tasks() -> Vec<BenchmarkTask> {
    vec![
        BenchmarkTask {
            name: "File Read",
            prompt: "Use the read_file tool to read data.txt and report its contents verbatim",
            max_iterations: 3,
            setup_fn: setup_file_read,
        },
        BenchmarkTask {
            name: "Bash Command",
            prompt: "Run 'echo BENCHMARK_ECHO_42' using bash and report the output verbatim",
            max_iterations: 3,
            setup_fn: setup_noop,
        },
        BenchmarkTask {
            name: "Write + Read Roundtrip",
            prompt: "Create a file called output.txt with the exact content 'BENCHMARK_WRITE_99', then read it back to verify the content matches",
            max_iterations: 5,
            setup_fn: setup_noop,
        },
        BenchmarkTask {
            name: "Grep Search",
            prompt: "Search for the pattern 'NEEDLE_IN_HAYSTACK' across all files in the current directory and report which file contains it",
            max_iterations: 5,
            setup_fn: setup_grep_search,
        },
        BenchmarkTask {
            name: "Multi-step",
            prompt: "List all .txt files, read each one, and provide a brief summary of each file's contents",
            max_iterations: 8,
            setup_fn: setup_multi_step,
        },
    ]
}

// ── Test fixture setup functions ──────────────────────────────────────

fn setup_noop(_dir: &Path) {}

fn setup_file_read(dir: &Path) {
    std::fs::write(
        dir.join("data.txt"),
        "BENCHMARK_DATA_CONTENT: The quick brown fox jumps over the lazy dog.",
    )
    .expect("Failed to write data.txt");
}

fn setup_grep_search(dir: &Path) {
    std::fs::write(
        dir.join("haystack1.txt"),
        "This file contains nothing useful.",
    )
    .expect("write haystack1");
    std::fs::write(dir.join("haystack2.txt"), "More irrelevant content here.")
        .expect("write haystack2");
    std::fs::write(
        dir.join("haystack3.txt"),
        "Hidden inside: NEEDLE_IN_HAYSTACK found!",
    )
    .expect("write haystack3");
    std::fs::write(
        dir.join("haystack4.txt"),
        "Another decoy file with no match.",
    )
    .expect("write haystack4");
}

fn setup_multi_step(dir: &Path) {
    std::fs::write(
        dir.join("alpha.txt"),
        "Alpha file: Contains project configuration data.",
    )
    .expect("write alpha");
    std::fs::write(
        dir.join("beta.txt"),
        "Beta file: Contains user authentication settings.",
    )
    .expect("write beta");
    std::fs::write(
        dir.join("gamma.txt"),
        "Gamma file: Contains deployment instructions.",
    )
    .expect("write gamma");
}

// ── Environment helpers ───────────────────────────────────────────────

fn remix_api_key() -> String {
    std::env::var("REMIX_LLM_API_KEY").expect("REMIX_LLM_API_KEY must be set for benchmarks")
}

fn remix_base_url() -> String {
    std::env::var("REMIX_LLM_BASE_URL").unwrap_or_else(|_| "https://api.anthropic.com".to_string())
}

fn remix_model() -> Option<String> {
    std::env::var("REMIX_LLM_MODEL").ok()
}

fn claude_model() -> String {
    std::env::var("BENCHMARK_CLAUDE_MODEL").unwrap_or_else(|_| "sonnet".to_string())
}

fn claude_cli_path() -> String {
    std::env::var("BENCHMARK_CLAUDE_PATH").unwrap_or_else(|_| "claude".to_string())
}

// ── Runtime invocation helpers ────────────────────────────────────────

fn run_remix_agent(prompt: &str, sandbox_dir: &Path, max_iterations: u32) -> Option<RemixMetrics> {
    let mut cmd: Command = cargo_bin_cmd!("remix-agent");
    cmd.env("REMIX_LLM_API_KEY", remix_api_key());
    cmd.env("REMIX_LLM_BASE_URL", remix_base_url());
    if let Ok(path) = std::env::var("REMIX_BROWSER_PATH") {
        cmd.env("REMIX_BROWSER_PATH", path);
    }

    let mut args: Vec<String> = vec!["run".to_string()];
    if let Some(m) = remix_model() {
        args.push("--model".to_string());
        args.push(m);
    }
    args.push("--sandbox-dir".to_string());
    args.push(sandbox_dir.to_string_lossy().to_string());
    args.push("--max-iterations".to_string());
    args.push(max_iterations.to_string());
    args.push(prompt.to_string());

    let output = cmd
        .args(&args)
        .timeout(std::time::Duration::from_secs(180))
        .output();

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);

            if !out.status.success() {
                eprintln!("  [remix-agent] FAILED (exit {:?})", out.status.code());
                eprintln!("  stderr: {}", truncate_str(&stderr, 200));
                return None;
            }

            match serde_json::from_str::<Value>(&stdout) {
                Ok(json) => {
                    let success = json["status"].as_str() == Some("success");
                    let wall_ms = json["total_duration_ms"].as_u64().unwrap_or(0);
                    let iterations = json["total_iterations"].as_u64().unwrap_or(0) as u32;
                    let tool_calls = json["steps"].as_array().map(|a| a.len()).unwrap_or(0);
                    let input_tokens = json["total_input_tokens"].as_u64().map(|v| v as u32);
                    let output_tokens = json["total_output_tokens"].as_u64().map(|v| v as u32);

                    Some(RemixMetrics {
                        wall_ms,
                        input_tokens,
                        output_tokens,
                        iterations,
                        tool_calls,
                        success,
                    })
                }
                Err(e) => {
                    eprintln!("  [remix-agent] JSON parse error: {e}");
                    eprintln!("  stdout: {}", truncate_str(&stdout, 200));
                    None
                }
            }
        }
        Err(e) => {
            eprintln!("  [remix-agent] Execution error: {e}");
            None
        }
    }
}

fn run_claude_code(prompt: &str, work_dir: &Path) -> Option<ClaudeMetrics> {
    let claude_path = claude_cli_path();
    let model = claude_model();
    let dir_str = work_dir.to_string_lossy().to_string();

    let mut cmd = Command::new(&claude_path);
    let output = cmd
        .args([
            "-p",
            prompt,
            "--output-format",
            "json",
            "--allowedTools",
            "Read,Write,Edit,Bash,Grep,Glob",
            "--add-dir",
            &dir_str,
            "--model",
            &model,
            "--dangerously-skip-permissions",
        ])
        .timeout(std::time::Duration::from_secs(180))
        .output();

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);

            if !out.status.success() {
                eprintln!("  [claude-code] FAILED (exit {:?})", out.status.code());
                eprintln!("  stderr: {}", truncate_str(&stderr, 200));
                return None;
            }

            // Claude Code outputs multiple JSON objects; we want the last "result" type
            let json = parse_claude_result_json(&stdout);
            match json {
                Some(parsed) => {
                    let success = parsed.subtype.as_deref() == Some("success")
                        || parsed.is_error == Some(false);
                    let wall_ms = parsed.duration_ms.unwrap_or(0);
                    let api_ms = parsed.duration_api_ms.unwrap_or(0);
                    let turns = parsed.num_turns.unwrap_or(0);
                    let cost_usd = parsed.total_cost_usd.unwrap_or(0.0);

                    let (input_tokens, output_tokens) = match &parsed.usage {
                        Some(u) => {
                            let input = u.input_tokens.unwrap_or(0)
                                + u.cache_creation_input_tokens.unwrap_or(0)
                                + u.cache_read_input_tokens.unwrap_or(0);
                            let output = u.output_tokens.unwrap_or(0);
                            (input, output)
                        }
                        None => (0, 0),
                    };

                    Some(ClaudeMetrics {
                        wall_ms,
                        api_ms,
                        input_tokens,
                        output_tokens,
                        turns,
                        cost_usd,
                        success,
                    })
                }
                None => {
                    eprintln!("  [claude-code] Could not find result JSON in output");
                    eprintln!("  stdout: {}", truncate_str(&stdout, 300));
                    None
                }
            }
        }
        Err(e) => {
            eprintln!("  [claude-code] Execution error: {e}");
            eprintln!("  Is 'claude' CLI installed and in PATH?");
            None
        }
    }
}

/// Claude Code may output streaming JSON lines. Find the final "result" object.
fn parse_claude_result_json(stdout: &str) -> Option<ClaudeCodeOutput> {
    // Try parsing the whole output as a single JSON object first
    if let Ok(parsed) = serde_json::from_str::<ClaudeCodeOutput>(stdout) {
        if parsed.type_field.as_deref() == Some("result") {
            return Some(parsed);
        }
    }

    // Otherwise, try each line (NDJSON / streaming output)
    let mut last_result: Option<ClaudeCodeOutput> = None;
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(parsed) = serde_json::from_str::<ClaudeCodeOutput>(trimmed) {
            if parsed.type_field.as_deref() == Some("result") {
                last_result = Some(parsed);
            }
        }
    }
    last_result
}

// ── Cost estimation ───────────────────────────────────────────────────

/// Estimate cost using Anthropic Sonnet pricing:
/// Input: $3 per million tokens, Output: $15 per million tokens
fn estimate_cost(input_tokens: u64, output_tokens: u64) -> f64 {
    (input_tokens as f64 * 3.0 / 1_000_000.0) + (output_tokens as f64 * 15.0 / 1_000_000.0)
}

// ── Display helpers ───────────────────────────────────────────────────

fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}

fn format_ms(ms: u64) -> String {
    if ms >= 1000 {
        format!("{},{:03}", ms / 1000, ms % 1000)
    } else {
        format!("{ms}")
    }
}

fn format_tokens(tokens: u64) -> String {
    if tokens >= 1000 {
        format!("{},{:03}", tokens / 1000, tokens % 1000)
    } else {
        format!("{tokens}")
    }
}

fn pct_diff(remix_val: f64, claude_val: f64) -> String {
    if claude_val == 0.0 && remix_val == 0.0 {
        return "same".to_string();
    }
    if claude_val == 0.0 {
        return "N/A".to_string();
    }
    let diff = ((remix_val - claude_val) / claude_val) * 100.0;
    if diff < 0.0 {
        format!("remix {:.1}% less", diff.abs())
    } else if diff > 0.0 {
        format!("remix {:.1}% more", diff)
    } else {
        "same".to_string()
    }
}

fn print_comparison_table(rows: &[ComparisonRow]) {
    let model_name = remix_model().unwrap_or_else(|| "default".to_string());
    let claude_model_name = claude_model();
    let date = chrono::Utc::now().format("%Y-%m-%d");

    println!();
    println!("{}", "=".repeat(78));
    println!(" BENCHMARK: remix-agent vs Claude Code CLI");
    println!(
        " remix model: {} | claude model: {} | Tasks: {} | Date: {}",
        model_name,
        claude_model_name,
        rows.len(),
        date
    );
    println!(" Note: remix-agent includes browser startup overhead (~1-2s per run)");
    println!("{}", "=".repeat(78));

    let mut total_remix_ms: u64 = 0;
    let mut total_claude_ms: u64 = 0;
    let mut total_remix_input: u64 = 0;
    let mut total_claude_input: u64 = 0;
    let mut total_remix_output: u64 = 0;
    let mut total_claude_output: u64 = 0;
    let mut total_remix_cost: f64 = 0.0;
    let mut total_claude_cost: f64 = 0.0;
    let mut remix_pass = 0u32;
    let mut claude_pass = 0u32;
    let task_count = rows.len();

    for row in rows {
        println!();
        println!(" Task: {}", row.task_name);
        println!(" {}", "-".repeat(74));

        match (&row.remix, &row.claude) {
            (Some(r), Some(c)) => {
                // Wall clock
                println!(
                    "  {:<20} {:>10}  {:>10}  {}",
                    "Wall Clock (ms)",
                    format_ms(r.wall_ms),
                    format_ms(c.wall_ms),
                    pct_diff(r.wall_ms as f64, c.wall_ms as f64)
                );

                // API time (claude only)
                println!(
                    "  {:<20} {:>10}  {:>10}",
                    "API Time (ms)",
                    "N/A",
                    format_ms(c.api_ms)
                );

                // Input tokens
                let r_input = r.input_tokens.unwrap_or(0) as u64;
                println!(
                    "  {:<20} {:>10}  {:>10}  {}",
                    "Input Tokens",
                    format_tokens(r_input),
                    format_tokens(c.input_tokens),
                    pct_diff(r_input as f64, c.input_tokens as f64)
                );

                // Output tokens
                let r_output = r.output_tokens.unwrap_or(0) as u64;
                println!(
                    "  {:<20} {:>10}  {:>10}  {}",
                    "Output Tokens",
                    format_tokens(r_output),
                    format_tokens(c.output_tokens),
                    pct_diff(r_output as f64, c.output_tokens as f64)
                );

                // Iterations / turns
                println!(
                    "  {:<20} {:>10}  {:>10}  {}",
                    "Iterations/Turns",
                    r.iterations,
                    c.turns,
                    pct_diff(r.iterations as f64, c.turns as f64)
                );

                // Tool calls (remix only)
                println!("  {:<20} {:>10}  {:>10}", "Tool Calls", r.tool_calls, "N/A");

                // Cost
                let r_cost = estimate_cost(r_input, r_output);
                println!(
                    "  {:<20} {:>10}  {:>10}  {}",
                    "Est. Cost (USD)",
                    format!("${:.4}", r_cost),
                    format!("${:.4}", c.cost_usd),
                    pct_diff(r_cost, c.cost_usd)
                );

                // Success
                println!(
                    "  {:<20} {:>10}  {:>10}",
                    "Success",
                    if r.success { "Yes" } else { "No" },
                    if c.success { "Yes" } else { "No" }
                );

                // Accumulate totals
                total_remix_ms += r.wall_ms;
                total_claude_ms += c.wall_ms;
                total_remix_input += r_input;
                total_claude_input += c.input_tokens;
                total_remix_output += r_output;
                total_claude_output += c.output_tokens;
                total_remix_cost += r_cost;
                total_claude_cost += c.cost_usd;
                if r.success {
                    remix_pass += 1;
                }
                if c.success {
                    claude_pass += 1;
                }
            }
            (Some(r), None) => {
                println!(
                    "  {:<20} {:>10}  {:>10}",
                    "Wall Clock (ms)",
                    format_ms(r.wall_ms),
                    "SKIPPED"
                );
                println!(
                    "  {:<20} {:>10}  {:>10}",
                    "Success",
                    if r.success { "Yes" } else { "No" },
                    "SKIPPED"
                );
                total_remix_ms += r.wall_ms;
                let r_input = r.input_tokens.unwrap_or(0) as u64;
                let r_output = r.output_tokens.unwrap_or(0) as u64;
                total_remix_input += r_input;
                total_remix_output += r_output;
                total_remix_cost += estimate_cost(r_input, r_output);
                if r.success {
                    remix_pass += 1;
                }
            }
            (None, Some(c)) => {
                println!(
                    "  {:<20} {:>10}  {:>10}",
                    "Wall Clock (ms)",
                    "SKIPPED",
                    format_ms(c.wall_ms)
                );
                println!(
                    "  {:<20} {:>10}  {:>10}",
                    "Success",
                    "SKIPPED",
                    if c.success { "Yes" } else { "No" }
                );
                total_claude_ms += c.wall_ms;
                total_claude_input += c.input_tokens;
                total_claude_output += c.output_tokens;
                total_claude_cost += c.cost_usd;
                if c.success {
                    claude_pass += 1;
                }
            }
            (None, None) => {
                println!("  BOTH RUNTIMES FAILED");
            }
        }
    }

    // ── Totals ────────────────────────────────────────────────────────
    println!();
    println!(" {}", "=".repeat(74));
    println!(
        " {:<22} {:>10}  {:>10}  comparison",
        "TOTALS", "remix", "claude"
    );
    println!(" {}", "-".repeat(74));
    println!(
        "  {:<20} {:>10}  {:>10}  {}",
        "Total Time (ms)",
        format_ms(total_remix_ms),
        format_ms(total_claude_ms),
        pct_diff(total_remix_ms as f64, total_claude_ms as f64)
    );
    println!(
        "  {:<20} {:>10}  {:>10}  {}",
        "Total Input Tokens",
        format_tokens(total_remix_input),
        format_tokens(total_claude_input),
        pct_diff(total_remix_input as f64, total_claude_input as f64)
    );
    println!(
        "  {:<20} {:>10}  {:>10}  {}",
        "Total Output Tokens",
        format_tokens(total_remix_output),
        format_tokens(total_claude_output),
        pct_diff(total_remix_output as f64, total_claude_output as f64)
    );
    println!(
        "  {:<20} {:>10}  {:>10}  {}",
        "Total Est. Cost",
        format!("${:.4}", total_remix_cost),
        format!("${:.4}", total_claude_cost),
        pct_diff(total_remix_cost, total_claude_cost)
    );
    println!(
        "  {:<20} {:>10}  {:>10}",
        "Tasks Passed",
        format!("{}/{}", remix_pass, task_count),
        format!("{}/{}", claude_pass, task_count)
    );
    println!(" {}", "=".repeat(74));
    println!();
}

// ── Benchmark test ────────────────────────────────────────────────────

#[test]
#[ignore]
fn benchmark_remix_vs_claude_code() {
    let benchmark_tasks = tasks();
    let mut results: Vec<ComparisonRow> = Vec::new();

    for task in &benchmark_tasks {
        let tmp = tempfile::TempDir::new().expect("Failed to create temp dir");
        let work_dir = tmp.path();

        // Set up test fixtures
        (task.setup_fn)(work_dir);

        println!("\n>>> Running task: {}", task.name);

        // Run remix-agent
        println!("  Running remix-agent...");
        let remix = run_remix_agent(task.prompt, work_dir, task.max_iterations);
        if let Some(ref m) = remix {
            println!("  remix-agent: {}ms, success={}", m.wall_ms, m.success);
        }

        // Run Claude Code CLI
        println!("  Running claude-code...");
        let claude = run_claude_code(task.prompt, work_dir);
        if let Some(ref m) = claude {
            println!("  claude-code: {}ms, success={}", m.wall_ms, m.success);
        }

        results.push(ComparisonRow {
            task_name: task.name.to_string(),
            remix,
            claude,
        });
    }

    // Print the full comparison table
    print_comparison_table(&results);

    // Ensure at least one runtime succeeded for each task
    for row in &results {
        assert!(
            row.remix.is_some() || row.claude.is_some(),
            "Task '{}': both runtimes failed",
            row.task_name
        );
    }
}

// ── Unit tests for helpers ────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_estimate_cost() {
        let cost = estimate_cost(1_000_000, 100_000);
        // Input: 1M * $3/M = $3.00, Output: 100K * $15/M = $1.50 → $4.50
        assert!((cost - 4.50).abs() < 0.001, "Expected ~$4.50, got {cost}");
    }

    #[test]
    fn test_estimate_cost_zero() {
        assert_eq!(estimate_cost(0, 0), 0.0);
    }

    #[test]
    fn test_format_ms() {
        assert_eq!(format_ms(500), "500");
        assert_eq!(format_ms(1200), "1,200");
        assert_eq!(format_ms(15000), "15,000");
        assert_eq!(format_ms(0), "0");
    }

    #[test]
    fn test_format_tokens() {
        assert_eq!(format_tokens(500), "500");
        assert_eq!(format_tokens(1200), "1,200");
        assert_eq!(format_tokens(115000), "115,000");
    }

    #[test]
    fn test_pct_diff() {
        // remix is 50% less
        assert!(pct_diff(50.0, 100.0).contains("50.0% less"));
        // remix is 100% more
        assert!(pct_diff(200.0, 100.0).contains("100.0% more"));
        // same
        assert_eq!(pct_diff(100.0, 100.0), "same");
        // both zero
        assert_eq!(pct_diff(0.0, 0.0), "same");
        // claude is zero
        assert_eq!(pct_diff(100.0, 0.0), "N/A");
    }

    #[test]
    fn test_truncate_str() {
        assert_eq!(truncate_str("hello", 10), "hello");
        assert_eq!(truncate_str("hello world", 5), "hello...");
    }

    #[test]
    fn test_parse_claude_result_json_single_object() {
        let json = r#"{"type":"result","subtype":"success","duration_ms":5000,"duration_api_ms":4500,"num_turns":3,"total_cost_usd":0.003,"is_error":false,"result":"done","usage":{"input_tokens":1200,"output_tokens":400}}"#;
        let parsed = parse_claude_result_json(json);
        assert!(parsed.is_some());
        let p = parsed.unwrap();
        assert_eq!(p.subtype.as_deref(), Some("success"));
        assert_eq!(p.duration_ms, Some(5000));
        assert_eq!(p.num_turns, Some(3));
    }

    #[test]
    fn test_parse_claude_result_json_ndjson() {
        let json = r#"{"type":"assistant","message":"thinking..."}
{"type":"tool_use","name":"Read"}
{"type":"result","subtype":"success","duration_ms":3000,"duration_api_ms":2500,"num_turns":2,"total_cost_usd":0.002,"is_error":false,"result":"ok","usage":{"input_tokens":800,"output_tokens":200}}"#;
        let parsed = parse_claude_result_json(json);
        assert!(parsed.is_some());
        let p = parsed.unwrap();
        assert_eq!(p.duration_ms, Some(3000));
    }

    #[test]
    fn test_parse_claude_result_json_no_result() {
        let json = r#"{"type":"assistant","message":"hello"}"#;
        assert!(parse_claude_result_json(json).is_none());
    }

    #[test]
    fn test_task_setup_file_read() {
        let tmp = tempfile::TempDir::new().unwrap();
        setup_file_read(tmp.path());
        assert!(tmp.path().join("data.txt").exists());
        let content = std::fs::read_to_string(tmp.path().join("data.txt")).unwrap();
        assert!(content.contains("BENCHMARK_DATA_CONTENT"));
    }

    #[test]
    fn test_task_setup_grep_search() {
        let tmp = tempfile::TempDir::new().unwrap();
        setup_grep_search(tmp.path());
        assert!(tmp.path().join("haystack1.txt").exists());
        assert!(tmp.path().join("haystack2.txt").exists());
        assert!(tmp.path().join("haystack3.txt").exists());
        assert!(tmp.path().join("haystack4.txt").exists());
        let content = std::fs::read_to_string(tmp.path().join("haystack3.txt")).unwrap();
        assert!(content.contains("NEEDLE_IN_HAYSTACK"));
    }

    #[test]
    fn test_task_setup_multi_step() {
        let tmp = tempfile::TempDir::new().unwrap();
        setup_multi_step(tmp.path());
        assert!(tmp.path().join("alpha.txt").exists());
        assert!(tmp.path().join("beta.txt").exists());
        assert!(tmp.path().join("gamma.txt").exists());
    }

    #[test]
    fn test_tasks_have_valid_definitions() {
        let t = tasks();
        assert_eq!(t.len(), 5);
        for task in &t {
            assert!(!task.name.is_empty());
            assert!(!task.prompt.is_empty());
            assert!(task.max_iterations >= 1);
        }
    }
}
