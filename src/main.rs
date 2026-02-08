use clap::Parser;
use std::process::ExitCode;
use tracing_subscriber::EnvFilter;

use remix_agent_runtime::agent::AgentRunner;
use remix_agent_runtime::browser::manager::BrowserManager;
use remix_agent_runtime::browser::mcp::McpBrowserClient;
use remix_agent_runtime::browser::mcp::ToolExecutor;
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

            // Discover skills
            let skill_set = if config.skills.enabled {
                let skill_dirs =
                    remix_agent_runtime::skills::default_skill_dirs(&config.skills.dirs);
                match remix_agent_runtime::skills::discover_all_skills(&skill_dirs) {
                    Ok(set) => {
                        if !set.is_empty() {
                            tracing::info!(count = set.len(), "Discovered skills");
                        }
                        set
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Skill discovery failed, continuing without skills");
                        remix_agent_runtime::skills::SkillSet::new()
                    }
                }
            } else {
                tracing::debug!("Skills disabled");
                remix_agent_runtime::skills::SkillSet::new()
            };

            // Discover AGENTS.md
            let agents_md = if config.agents_md.enabled {
                match remix_agent_runtime::agents_md::discover_agents_md(&config.agents_md) {
                    Ok(Some(content)) => {
                        tracing::info!(
                            sources = ?content.sources,
                            "Discovered AGENTS.md instructions"
                        );
                        Some(content)
                    }
                    Ok(None) => None,
                    Err(e) => {
                        tracing::warn!(error = %e, "AGENTS.md discovery failed, continuing without");
                        None
                    }
                }
            } else {
                tracing::debug!("AGENTS.md discovery disabled");
                None
            };

            // Wrap MCP client with skill-aware executor
            let executor = remix_agent_runtime::skills::SkillAwareExecutor::new(
                mcp_client,
                skill_set.clone(),
                config.skills.script_timeout_secs,
            );

            // Wrap with local tools executor
            let executor = match remix_agent_runtime::local_tools::LocalToolsExecutor::new(
                executor,
                config.local_tools.clone(),
            ) {
                Ok(e) => {
                    if config.local_tools.enabled {
                        tracing::info!(tools = e.tool_definitions().len(), "Local tools enabled");
                    }
                    e
                }
                Err(e) => {
                    eprintln!("Error: Failed to initialize local tools: {e}");
                    return ExitStatus::AgentError.into();
                }
            };

            // Run agent
            let runner = AgentRunner::new(llm_client, executor, config.agent.clone());
            let result = runner
                .run(&task, &credential_set, &skill_set, &agents_md)
                .await;

            // Gracefully shut down the MCP browser connection
            runner.into_tools().into_inner().into_inner().shutdown();

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
