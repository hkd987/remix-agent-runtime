use clap::Parser;
use std::process::ExitCode;
use tracing_subscriber::EnvFilter;

use remix_agent_runtime::agent::AgentRunner;
use remix_agent_runtime::browser::manager::BrowserManager;
use remix_agent_runtime::browser::mcp::McpBrowserClient;
use remix_agent_runtime::browser::mcp::ToolExecutor;
use remix_agent_runtime::cli::{Cli, Commands, SessionsCommand};
use remix_agent_runtime::config::credentials;
use remix_agent_runtime::config::load_config;
use remix_agent_runtime::config::schema::default_session_dir;
use remix_agent_runtime::coordination::{
    CoordinationContext, CoordinationExecutor, DefaultSpawnHandler,
};
use remix_agent_runtime::error::ExitStatus;
use remix_agent_runtime::llm::client::AnthropicClient;
use remix_agent_runtime::output::webhook::WebhookDispatcher;
use remix_agent_runtime::permissions::{PermissionAwareExecutor, PermissionMode, PermissionPolicy};
use remix_agent_runtime::plugins::components::agents::{
    discover_agents_in_files, inject_agents_into_system_prompt,
};
use remix_agent_runtime::plugins::components::hooks::HookRegistry;
use remix_agent_runtime::plugins::components::mcp::{build_mcp_command, parse_mcp_config};
use remix_agent_runtime::plugins::components::skills::merge_plugin_skills;
use remix_agent_runtime::plugins::discovery::discover_all_plugins;
use remix_agent_runtime::plugins::{CompositeToolExecutor, HookAwareExecutor};
use remix_agent_runtime::session::{SessionStorage, SessionStore};

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

            // Create LLM client (wrapped in Arc for sharing with child agents)
            let thinking_config = config
                .llm
                .thinking_budget_tokens
                .map(remix_agent_runtime::llm::types::ThinkingConfig::enabled);
            let llm_client = std::sync::Arc::new(AnthropicClient::new(
                config.llm.base_url.clone(),
                config.llm.api_key.clone(),
                config.llm.model.clone(),
                config.llm.max_tokens,
                config.llm.custom_headers.clone(),
                thinking_config,
                config.llm.enable_prompt_caching,
            ));

            // Spawn browser and connect via MCP (if enabled)
            let mcp_client = if config.browser.enabled {
                let command = BrowserManager::build_command(&config.browser);
                match McpBrowserClient::connect(command).await {
                    Ok(c) => {
                        tracing::info!(
                            tools = c.tool_definitions().len(),
                            "Connected to remix-browser"
                        );
                        Some(c)
                    }
                    Err(e) => {
                        eprintln!("Error: Failed to connect to remix-browser: {e}");
                        return ExitStatus::AgentError.into();
                    }
                }
            } else {
                tracing::info!("Browser disabled, running in terminal-only mode");
                None
            };

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

            // Discover plugins
            let plugin_set = match discover_all_plugins(&config.plugins) {
                Ok(set) => {
                    if !set.is_empty() {
                        tracing::info!(count = set.len(), "Discovered plugins");
                    }
                    set
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Plugin discovery failed, continuing without plugins");
                    remix_agent_runtime::plugins::PluginSet::new()
                }
            };

            // Merge plugin skills into the skill set
            let mut skill_set = skill_set;
            if config.plugins.components.skills {
                if let Err(e) = merge_plugin_skills(&plugin_set, &mut skill_set) {
                    tracing::warn!(error = %e, "Failed to merge plugin skills, continuing");
                }
            }

            // Build hook registry from plugin hooks
            let mut hook_registry = HookRegistry::new();
            if config.plugins.components.hooks {
                for (plugin, hooks_path) in plugin_set.all_hook_configs() {
                    if let Err(e) = hook_registry.load_from_file(hooks_path, &plugin.root_path) {
                        tracing::warn!(
                            plugin = %plugin.name,
                            error = %e,
                            "Failed to load plugin hooks, skipping"
                        );
                    }
                }
            }

            // Discover plugin agents and inject into system prompt
            let mut agent_config = config.agent.clone();
            agent_config.coordination_config = if config.coordination.enabled {
                Some(config.coordination.clone())
            } else {
                None
            };
            if config.plugins.components.agents {
                let agent_file_refs: Vec<&std::path::PathBuf> = plugin_set
                    .all_agent_files()
                    .into_iter()
                    .map(|(_, f)| f)
                    .collect();
                let plugin_agents = discover_agents_in_files(&agent_file_refs);
                if !plugin_agents.is_empty() {
                    tracing::info!(count = plugin_agents.len(), "Discovered plugin agents");
                    if let Some(agents_prompt) = inject_agents_into_system_prompt(&plugin_agents) {
                        let existing = agent_config.system_prompt.take().unwrap_or_default();
                        agent_config.system_prompt = if existing.is_empty() {
                            Some(agents_prompt)
                        } else {
                            Some(format!("{existing}\n\n{agents_prompt}"))
                        };
                    }
                }
            }

            // Build CompositeToolExecutor with browser + plugin MCP servers
            let mut composite = CompositeToolExecutor::new();
            if let Some(client) = mcp_client {
                composite.add_backend(Box::new(client));
            }

            if config.plugins.components.mcp_servers {
                for (plugin, config_path) in plugin_set.all_mcp_configs() {
                    match parse_mcp_config(config_path, &plugin.root_path) {
                        Ok(mcp_config) => {
                            for (server_name, server_config) in &mcp_config.mcpServers {
                                let cmd = build_mcp_command(server_config, &plugin.root_path);
                                match McpBrowserClient::connect(cmd).await {
                                    Ok(client) => {
                                        tracing::info!(
                                            plugin = %plugin.name,
                                            server = %server_name,
                                            tools = client.tool_definitions().len(),
                                            "Connected plugin MCP server"
                                        );
                                        composite.add_backend(Box::new(client));
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            plugin = %plugin.name,
                                            server = %server_name,
                                            error = %e,
                                            "Failed to connect plugin MCP server, skipping"
                                        );
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                plugin = %plugin.name,
                                error = %e,
                                "Failed to parse plugin .mcp.json, skipping"
                            );
                        }
                    }
                }
            }

            // Wrap composite executor with skill-aware executor
            let executor = remix_agent_runtime::skills::SkillAwareExecutor::new(
                composite,
                skill_set.clone(),
                config.skills.script_timeout_secs,
            );

            // Wrap with local tools executor
            let executor = match remix_agent_runtime::local_tools::LocalToolsExecutor::new(
                executor,
                config.local_tools.clone(),
                config.agent.tool_result_max_bytes,
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

            // Wrap with dev tools executor
            let executor = remix_agent_runtime::dev_tools::DevToolsExecutor::new(
                executor,
                config.dev_tools.clone(),
            );
            if config.dev_tools.enabled {
                tracing::info!(
                    tools = executor.tool_definitions().len(),
                    "Dev tools enabled"
                );
            }

            // Wrap with hook-aware executor
            let executor =
                HookAwareExecutor::new(executor, hook_registry, config.plugins.hook_timeout_secs);

            // Wrap with permission-aware executor
            let permission_mode = match config.permissions.mode {
                remix_agent_runtime::config::schema::PermissionModeConfig::Default => {
                    PermissionMode::Default
                }
                remix_agent_runtime::config::schema::PermissionModeConfig::AcceptEdits => {
                    PermissionMode::AcceptEdits
                }
                remix_agent_runtime::config::schema::PermissionModeConfig::BypassPermissions => {
                    PermissionMode::BypassPermissions
                }
                remix_agent_runtime::config::schema::PermissionModeConfig::Plan => {
                    PermissionMode::Plan
                }
                remix_agent_runtime::config::schema::PermissionModeConfig::DontAsk => {
                    PermissionMode::DontAsk
                }
            };
            let permission_policy = PermissionPolicy {
                mode: permission_mode,
                allowed_tools: config.permissions.allowed_tools.clone(),
                denied_tools: config.permissions.denied_tools.clone(),
            };
            let executor = PermissionAwareExecutor::new(executor, permission_policy);

            // Wrap with coordination executor (supersedes SubagentExecutor)
            let coordination_context =
                std::sync::Arc::new(CoordinationContext::from_config(&config.coordination));
            let spawn_handler = std::sync::Arc::new(DefaultSpawnHandler::new(
                llm_client.clone(),
                config.browser.clone(),
                config.local_tools.clone(),
                config.skills.clone(),
                skill_set.clone(),
                config.coordination.clone(),
                coordination_context.clone(),
                config.agent.tool_result_max_bytes,
            ));
            let executor = CoordinationExecutor::new(
                executor,
                config.coordination.clone(),
                coordination_context,
                "lead".to_string(),
            )
            .with_spawn_handler(spawn_handler);

            // Set up session store
            let session_store = if config.session.enabled {
                Some(SessionStore::new(
                    config.session.storage_dir.clone(),
                    config.session.max_sessions,
                ))
            } else {
                None
            };

            // Set up compaction config
            let compaction_config = if config.compaction.enabled {
                Some(config.compaction.clone())
            } else {
                None
            };

            // Set up event bus for real-time streaming
            let event_bus = std::sync::Arc::new(remix_agent_runtime::output::EventBus::new(256));

            // Start SSE server if port is configured
            #[cfg(feature = "sse")]
            if let Some(sse_port) = args.sse_port {
                let bus = event_bus.clone();
                tokio::spawn(async move {
                    if let Err(e) =
                        remix_agent_runtime::output::sse_server::start_sse_server(bus, sse_port)
                            .await
                    {
                        eprintln!("SSE server error: {e}");
                    }
                });
            }
            #[cfg(not(feature = "sse"))]
            if args.sse_port.is_some() {
                eprintln!("Warning: --sse-port requires the 'sse' feature. Rebuild with: cargo build --features sse");
            }

            // Handle session resume/fork
            let thinking_control: std::sync::Arc<
                dyn remix_agent_runtime::llm::client::ThinkingControl,
            > = llm_client.clone();
            let runner = AgentRunner::new(llm_client, executor, agent_config)
                .with_event_bus(event_bus)
                .with_thinking_control(thinking_control);
            let result = if let Some(ref session_id) = args.session_id {
                // Resume existing session
                let store = session_store.as_ref().unwrap_or_else(|| {
                    eprintln!("Error: --session-id requires sessions to be enabled");
                    std::process::exit(2);
                });
                let sid = remix_agent_runtime::session::SessionId(session_id.clone());
                runner
                    .resume(
                        store,
                        &sid,
                        &credential_set,
                        &skill_set,
                        &agents_md,
                        compaction_config.as_ref(),
                        None,
                    )
                    .await
            } else if let Some(ref fork_id) = args.fork_session {
                // Fork from existing session, then run fresh with forked state
                let store = session_store.as_ref().unwrap_or_else(|| {
                    eprintln!("Error: --fork-session requires sessions to be enabled");
                    std::process::exit(2);
                });
                let source_id = remix_agent_runtime::session::SessionId(fork_id.clone());
                match store.fork(&source_id).await {
                    Ok(forked_metadata) => {
                        tracing::info!(
                            forked_session = %forked_metadata.id,
                            source = %source_id,
                            "Forked session"
                        );
                        runner
                            .resume(
                                store,
                                &forked_metadata.id,
                                &credential_set,
                                &skill_set,
                                &agents_md,
                                compaction_config.as_ref(),
                                None,
                            )
                            .await
                    }
                    Err(e) => {
                        eprintln!("Error: Failed to fork session: {e}");
                        return ExitStatus::AgentError.into();
                    }
                }
            } else {
                runner
                    .run_with_options(
                        &task,
                        &credential_set,
                        &skill_set,
                        &agents_md,
                        session_store
                            .as_ref()
                            .map(|s| s as &dyn remix_agent_runtime::session::SessionStorage),
                        compaction_config.as_ref(),
                        None,
                    )
                    .await
            };

            // Wait for any spawned child agents to complete
            let child_results = runner
                .tools_ref()
                .wait_for_children(std::time::Duration::from_secs(
                    config.coordination.worker_timeout_secs,
                ))
                .await;
            for (name, child_result) in &child_results {
                match child_result {
                    Ok(r) => {
                        tracing::info!(agent = %name, status = ?r.status, "Child agent completed")
                    }
                    Err(e) => tracing::warn!(agent = %name, error = %e, "Child agent failed"),
                }
            }

            // Gracefully shut down all MCP connections
            // Chain: CoordinationExecutor -> PermissionAwareExecutor -> HookAwareExecutor -> DevToolsExecutor -> LocalToolsExecutor -> SkillAwareExecutor -> CompositeToolExecutor
            runner
                .into_tools()
                .into_inner()
                .into_inner()
                .into_inner()
                .into_inner()
                .into_inner()
                .into_inner()
                .shutdown_all();

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
        Commands::Sessions(sessions_args) => {
            let default_session_dir = default_session_dir;

            match sessions_args.command {
                SessionsCommand::List { session_dir } => {
                    let dir = session_dir.unwrap_or_else(default_session_dir);
                    let store = SessionStore::new(dir, usize::MAX);
                    match store.list().await {
                        Ok(sessions) => {
                            if sessions.is_empty() {
                                println!("No sessions found.");
                            } else {
                                for meta in &sessions {
                                    println!(
                                        "{}\t{:?}\t{}\t{}",
                                        meta.id,
                                        meta.status,
                                        meta.created_at.format("%Y-%m-%d %H:%M:%S"),
                                        meta.task
                                    );
                                }
                            }
                            ExitStatus::Success.into()
                        }
                        Err(e) => {
                            eprintln!("Error listing sessions: {e}");
                            ExitStatus::AgentError.into()
                        }
                    }
                }
                SessionsCommand::Show { id, session_dir } => {
                    let dir = session_dir.unwrap_or_else(default_session_dir);
                    let store = SessionStore::new(dir, usize::MAX);
                    let session_id = remix_agent_runtime::session::SessionId(id);
                    match store.load(&session_id).await {
                        Ok(snapshot) => {
                            let json = serde_json::to_string_pretty(&snapshot).unwrap();
                            println!("{json}");
                            ExitStatus::Success.into()
                        }
                        Err(e) => {
                            eprintln!("Error loading session: {e}");
                            ExitStatus::AgentError.into()
                        }
                    }
                }
            }
        }
    }
}
