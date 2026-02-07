use clap::Parser;
use std::process::ExitCode;
use tracing_subscriber::EnvFilter;

use remix_agent_runtime::agent::AgentRunner;
use remix_agent_runtime::browser::manager::BrowserManager;
use remix_agent_runtime::browser::mcp::McpBrowserClient;
use remix_agent_runtime::browser::ToolExecutor;
use remix_agent_runtime::cli::{Cli, Commands};
use remix_agent_runtime::config::credentials;
use remix_agent_runtime::config::load_config;
use remix_agent_runtime::error::ExitStatus;
use remix_agent_runtime::llm::client::AnthropicClient;
use remix_agent_runtime::output::webhook::WebhookDispatcher;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run(args) => {
            // Set up logging
            let filter = if args.verbose {
                EnvFilter::new("remix_agent_runtime=debug")
            } else {
                EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"))
            };
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_writer(std::io::stderr)
                .init();

            // Load config
            let config = match load_config(&args) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Error: {e}");
                    return ExitStatus::ConfigError.into();
                }
            };

            // Validate task exists
            let task = match config.task.as_deref() {
                Some(t) => t.to_string(),
                None => {
                    eprintln!("Error: No task provided. Use a positional argument or --config with a YAML file containing a task.");
                    return ExitStatus::ConfigError.into();
                }
            };

            // Validate API key (skip validation for localhost URLs like Ollama)
            if config.llm.api_key.is_empty()
                && !config.llm.base_url.contains("localhost")
                && !config.llm.base_url.contains("127.0.0.1")
            {
                eprintln!("Error: No API key provided. Use --api-key, REMIX_LLM_API_KEY env var, or config file.");
                return ExitStatus::ConfigError.into();
            }

            tracing::info!("Starting agent with config: {}", config.llm);

            // Create LLM client
            let llm_client = AnthropicClient::new(
                config.llm.base_url.clone(),
                config.llm.api_key.clone(),
                config.llm.model.clone(),
                config.llm.max_tokens,
                config.llm.custom_headers.clone(),
            );

            // Spawn browser and connect via MCP
            let command = BrowserManager::build_command(&config.browser);
            let mcp_client = match McpBrowserClient::connect(command).await {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Error: Failed to connect to remix-browser: {e}");
                    return ExitStatus::AgentError.into();
                }
            };

            tracing::info!(
                tools = mcp_client.tool_definitions().len(),
                "Connected to remix-browser"
            );

            // Create webhook dispatcher
            let webhook =
                WebhookDispatcher::new(config.on_complete.as_ref(), config.on_error.as_ref());

            // Convert raw credentials to real CredentialSet
            let credential_set = match credentials::convert_raw_credentials(&config.credentials) {
                Ok(cs) => cs,
                Err(e) => {
                    eprintln!("Error: {e}");
                    return ExitStatus::ConfigError.into();
                }
            };

            // Run agent
            let runner = AgentRunner::new(llm_client, mcp_client, config.agent.clone());
            let result = runner.run(&task, &credential_set).await;

            // Gracefully shut down the MCP browser connection
            runner.into_tools().shutdown();

            match result {
                Ok(agent_result) => {
                    // Output JSON to stdout (or file)
                    let json = serde_json::to_string_pretty(&agent_result).unwrap();

                    if let Some(ref output_path) = args.output {
                        if let Err(e) = std::fs::write(output_path, &json) {
                            eprintln!("Error writing output file: {e}");
                        }
                    } else {
                        println!("{json}");
                    }

                    // Fire webhooks
                    match agent_result.status {
                        remix_agent_runtime::output::AgentStatus::Success => {
                            webhook.send_completion(&agent_result).await;
                            ExitStatus::Success.into()
                        }
                        _ => {
                            webhook.send_error(&agent_result).await;
                            ExitStatus::AgentError.into()
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Error: {e}");

                    // Create error result for webhook
                    let error_result =
                        remix_agent_runtime::output::AgentResult::error(e.to_string(), vec![], 0);
                    webhook.send_error(&error_result).await;

                    e.exit_status().into()
                }
            }
        }
    }
}
