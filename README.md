# remix-agent-runtime

LLM-driven browser automation agent runtime. Give it a task in plain English, and it uses an LLM to control a real Chrome browser until the job is done.

`remix-agent-runtime` is the orchestration layer that connects [remix-browser](https://github.com/hkd987/remix-browser) (headless Chrome via MCP) with any LLM provider to create an autonomous browser automation agent. It works with Anthropic, OpenRouter, AWS Bedrock, or any provider compatible with the Anthropic Messages API format. Credentials are secured by [remix-credentials](https://github.com/hkd987/remix-credentials).

## How it works

```
                         remix-agent-runtime
 ┌───────────────────────────────────────────────────────────┐
 │                                                           │
 │   "Log into GitHub and star the remix-browser repo"       │
 │                          │                                │
 │                          ▼                                │
 │  AGENTS.md ──► ┌────────────────┐ ◄── Credentials         │
 │  instructions  │   Agent Loop   │     (remix-credentials) │
 │                └───────┬────────┘                         │
 │                   ▲    │                                  │
 │          results  │    │ tool calls                        │
 │                   │    ▼                                  │
 │    ┌──────────────┴──────────────────────────────────┐    │
 │    │             Decorator Chain                      │    │
 │    │                                                  │    │
 │    │  ┌─────────────────────────────────────────┐    │    │
 │    │  │ CoordinationExecutor (7 coord tools)    │    │    │
 │    │  └──────────────────┬──────────────────────┘    │    │
 │    │                     │                            │    │
 │    │  ┌──────────────────┴──────────────────────┐    │    │
 │    │  │ PermissionAwareExecutor (4 modes)       │    │    │
 │    │  └──────────────────┬──────────────────────┘    │    │
 │    │                     │                            │    │
 │    │  ┌──────────────────┴──────────────────────┐    │    │
 │    │  │ HookAwareExecutor (pre/post tool hooks) │    │    │
 │    │  └──────────────────┬──────────────────────┘    │    │
 │    │                     │                            │    │
 │    │  ┌──────────────────┴──────────────────────┐    │    │
 │    │  │ LocalToolsExecutor (6 sandboxed tools)  │    │    │
 │    │  └──────────────────┬──────────────────────┘    │    │
 │    │                     │                            │    │
 │    │  ┌──────────────────┴──────────────────────┐    │    │
 │    │  │ SkillAwareExecutor (3 skill tools)      │    │    │
 │    │  └──────────────────┬──────────────────────┘    │    │
 │    │                     │                            │    │
 │    │  ┌──────────────────┴──────────────────────┐    │    │
 │    │  │ CompositeToolExecutor (MCP backends)    │    │    │
 │    │  └────┬────────────────────────────────────┘    │    │
 │    └───────┼────────────────────────────────────────┘    │
 │            │                                             │
 │  ┌─────────┘     ┌──────────────────────┐                │
 │  │               │ Sandboxed filesystem │                │
 │  ▼               │ (Seatbelt/Landlock)  │                │
 │  remix-browser   └──────────────────────┘                │
 │  (MCP Server)                                            │
 │       │                                                  │
 └───────┼──────────────────────────────────────────────────┘
         │ CDP
         ▼
   ┌──────────────┐
   │    Chrome     │
   └──────────────┘
```

1. You provide a task in natural language
2. The agent sends the task + available tools to the LLM
3. The LLM decides which tools to call (navigate, click, type, read_file, bash, etc.)
4. Tool calls pass through the decorator chain: hooks fire, local tools and skills are intercepted, everything else routes to the browser MCP backend
5. Results go back to the LLM, which decides the next action
6. Loop continues until the task is complete or a stopping condition is hit
7. Structured JSON output with every step recorded

## The remix ecosystem

| Project | Role |
|---------|------|
| [remix-browser](https://github.com/hkd987/remix-browser) | Rust-native MCP server for Chrome automation -- 18+ tools for navigation, clicking, typing, screenshots, network monitoring, and more |
| [remix-credentials](https://github.com/hkd987/remix-credentials) | Secure credential management with AES-256-GCM encryption, Argon2id key derivation, and zeroizable memory |
| **remix-agent-runtime** (this project) | The agent loop that ties it all together -- connects an LLM to browser tools and runs autonomously |

## Quick start

### Prerequisites

- Google Chrome or Chromium
- An API key from a supported LLM provider (Anthropic, OpenRouter, AWS Bedrock, etc.)

### Install

One command installs both `remix-agent` and `remix-browser` -- no Rust toolchain needed:

```bash
curl -fsSL https://raw.githubusercontent.com/hkd987/remix-agent-runtime/main/scripts/install.sh | sh
```

If you already have `remix-browser` installed, the script detects it and only installs the agent.

<details>
<summary>From source</summary>

Requires [Rust](https://rustup.rs/) 1.88+:

```bash
# Install remix-browser
curl -fsSL https://raw.githubusercontent.com/hkd987/remix-browser/main/scripts/install.sh | sh

# Build remix-agent from source
git clone https://github.com/hkd987/remix-agent-runtime.git
cd remix-agent-runtime && cargo build --release
cp target/release/remix-agent /usr/local/bin/
```

</details>

Pre-built binaries are available for macOS (Apple Silicon & Intel), Linux x86_64, and Windows x86_64. See [Releases](https://github.com/hkd987/remix-agent-runtime/releases) for all downloads.

### Run your first task

```bash
export REMIX_LLM_API_KEY=sk-ant-your-key-here

remix-agent run "Navigate to example.com and tell me what's on the page"
```

## Usage

### CLI

```bash
remix-agent run [OPTIONS] [TASK]
```

| Flag | Short | Env Var | Description |
|------|-------|---------|-------------|
| `--config <PATH>` | `-c` | -- | Path to YAML configuration file |
| `--api-key <KEY>` | -- | `REMIX_LLM_API_KEY` | LLM provider API key |
| `--base-url <URL>` | -- | `REMIX_LLM_BASE_URL` | LLM provider base URL (default: Anthropic) |
| `--model <NAME>` | -- | `REMIX_LLM_MODEL` | Model ID (default: `claude-sonnet-4-20250514`) |
| `--max-tokens <N>` | -- | -- | Max tokens per response (default: 8192) |
| `--timeout <SECS>` | -- | -- | Max duration in seconds |
| `--max-iterations <N>` | -- | -- | Max agent loop iterations (default: 50) |
| `--headed` | -- | -- | Show the browser window |
| `--verbose` | `-v` | -- | Debug logging to stderr |
| `--output <PATH>` | `-o` | -- | Write JSON results to file |
| `--browser-path <PATH>` | -- | `REMIX_BROWSER_PATH` | Path to remix-browser binary |
| `--agents-md-dir <PATH>` | -- | `REMIX_AGENTS_MD_DIR` | Override AGENTS.md search directory |
| `--no-agents-md` | -- | -- | Disable AGENTS.md discovery |
| `--no-local-tools` | -- | -- | Disable local filesystem tools |
| `--sandbox-dir <PATH>` | -- | `REMIX_SANDBOX_DIR` | Sandbox root for local tools |
| `--skills-dir <PATH>` | -- | `REMIX_SKILLS_DIR` | Additional skills directory |
| `--no-skills` | -- | -- | Disable skill discovery |
| `--no-plugins` | -- | -- | Disable all plugin discovery |
| `--plugins-dir <PATH>` | -- | `REMIX_PLUGINS_DIR` | Additional plugin directory |
| `--no-claude-plugins` | -- | -- | Disable Claude Code plugin cache |
| `--session-id <ID>` | -- | -- | Resume an existing session |
| `--fork-session <ID>` | -- | -- | Fork from an existing session |
| `--session-dir <PATH>` | -- | `REMIX_SESSION_DIR` | Override session storage directory |
| `--permission-mode <MODE>` | -- | -- | Permission mode: `default`, `accept_edits`, `bypass_permissions`, `plan` |
| `--allow-tool <PATTERN>` | -- | -- | Regex pattern for auto-allowed tools (repeatable) |
| `--deny-tool <PATTERN>` | -- | -- | Regex pattern for denied tools (repeatable) |
| `--no-coordination` | -- | -- | Disable multi-agent coordination |
| `--max-workers <N>` | -- | -- | Maximum concurrent worker agents (default: 5) |
| `--coordination-dir <PATH>` | -- | `REMIX_COORDINATION_DIR` | Override coordination storage directory |

### Examples

```bash
# Simple task
remix-agent run "Take a screenshot of hacker news"

# Watch the browser work (headed mode)
remix-agent run --headed "Fill out the contact form on example.com"

# Use a specific model
remix-agent run --model claude-opus-4-20250805 "Complex multi-step task here"

# Save structured output
remix-agent run --output results.json "Find the price of item X on site Y"

# Full config file
remix-agent run --config task.yaml --verbose

# With a local plugin
remix-agent run --plugins-dir ./my-plugin "Run my custom workflow"
```

### Using different LLM providers

The runtime works with any provider that exposes an Anthropic Messages API-compatible endpoint. Just change the `--base-url` and `--model`:

```bash
# Anthropic (default)
remix-agent run --api-key sk-ant-xxx "Your task"

# OpenRouter
remix-agent run \
  --base-url https://openrouter.ai/api \
  --api-key sk-or-xxx \
  --model anthropic/claude-sonnet-4 \
  "Your task"

# AWS Bedrock (via proxy)
remix-agent run \
  --base-url https://your-bedrock-proxy.com \
  --api-key your-key \
  --model anthropic.claude-sonnet-4-20250514-v1:0 \
  "Your task"

# Any compatible provider
remix-agent run \
  --base-url https://your-provider.com \
  --model your-model-id \
  --api-key your-key \
  "Your task"
```

Custom headers can be added via the YAML config for providers that need them:

```yaml
llm:
  base_url: "https://your-provider.com"
  api_key: "your-key"
  model: "your-model"
  custom_headers:
    X-Provider-Key: "value"
    HTTP-Referer: "https://your-app.com"
```

## Configuration

CLI flags override environment variables, which override the YAML config, which overrides defaults.

### YAML config file

```yaml
task: "Log into the dashboard and export the monthly report"

llm:
  api_key: "${ANTHROPIC_API_KEY}"
  model: "claude-sonnet-4-20250514"
  max_tokens: 8192

agent:
  max_iterations: 50
  timeout_secs: 300
  system_prompt: |
    You are an expert browser automation agent.
    Complete the task efficiently and report what you find.

browser:
  headless: true
  viewport_width: 1280
  viewport_height: 720

credentials:
  - name: "dashboard_login"
    credential_type: username_password
    username: "${DASHBOARD_USER}"
    password: "${DASHBOARD_PASS}"
    url_pattern: "*.internal.company.com"

agents_md:
  enabled: true
  search_dir: "/path/to/project"
  max_size_bytes: 32768

local_tools:
  enabled: true
  sandbox_dir: "/path/to/sandbox"
  bash_timeout_secs: 120
  read_max_bytes: 1048576
  write_max_bytes: 10485760

skills:
  dirs:
    - "/path/to/skills"
  enabled: true
  script_timeout_secs: 60

plugins:
  enabled: true
  claude_code_cache: true
  hook_timeout_secs: 30
  sources:
    - path: "/path/to/local-plugin"
    - github: "owner/repo"
      git_ref: "v1.0"
  components:
    skills: true
    mcp_servers: true
    hooks: true
    agents: true

session:
  enabled: true
  storage_dir: "~/.remix/sessions"
  max_sessions: 100

compaction:
  enabled: true
  trigger_threshold: 0.95
  context_window_tokens: 200000
  preserve_recent_n: 4

permissions:
  mode: default
  allowed_tools:
    - "navigate|click|screenshot"
  denied_tools:
    - "bash"

coordination:
  enabled: true
  max_workers: 5
  max_worker_iterations: 10
  worker_timeout_secs: 120
  storage_dir: "~/.remix/coordination"

on_complete:
  url: "https://hooks.slack.com/your-webhook"
  format: "json"

on_error:
  url: "https://hooks.slack.com/your-error-webhook"
  format: "json"
```

Environment variables can be interpolated in YAML using `${VAR_NAME}` syntax.

### Credentials

Credentials are securely managed via [remix-credentials](https://github.com/hkd987/remix-credentials) -- values use zeroizable memory and are redacted from logs.

```yaml
credentials:
  # Username/password login
  - name: "site_login"
    credential_type: username_password
    username: "admin"
    password: "secret"
    url_pattern: "*.example.com"

  # API key
  - name: "api_auth"
    credential_type: api_key
    fields:
      api_key: "sk-xxxxx"

  # Custom fields
  - name: "oauth_creds"
    credential_type: custom
    fields:
      client_id: "id123"
      client_secret: "secret456"
      tenant: "acme"
```

Supported credential types: `username_password`, `api_key`, `token`, `cookie`, `custom`.

### AGENTS.md

The agent supports the [AGENTS.md](https://github.com/anthropics/agents-md) standard for project-level instructions. When enabled, the agent walks from the search directory (or current working directory) up to the filesystem root, collecting all `AGENTS.md` files it finds.

- Files are ordered root-to-leaf (general instructions first, project-specific last)
- Concatenated content is capped at 32KB by default (`max_size_bytes`)
- Injected into the system prompt wrapped in `<project_instructions>` tags
- Override the search directory with `--agents-md-dir` or `REMIX_AGENTS_MD_DIR`
- Disable with `--no-agents-md`

### Local tools

When enabled, the agent has access to six sandboxed filesystem and shell tools:

| Tool | Description |
|------|-------------|
| `read_file` | Read file contents with optional offset/limit |
| `write_file` | Write content to a file (creates parent dirs) |
| `edit_file` | Find-and-replace exact string in a file |
| `bash` | Execute a shell command in the sandbox |
| `grep` | Regex search across files with context |
| `glob` | Find files matching a glob pattern |

All file operations are restricted to the sandbox directory. Use `--sandbox-dir` or `REMIX_SANDBOX_DIR` to set the root. Disable with `--no-local-tools`.

### Sandboxing

Local tools are sandboxed at the OS level:

- **macOS**: Seatbelt profiles restrict file access and network to the sandbox directory
- **Linux**: Landlock LSM restricts filesystem access (with fallback for older kernels)
- **Path validation**: All file tool paths are resolved and checked against the sandbox root
- **Timeouts**: Bash commands are killed after the configured timeout (default: 120s)

### Skills

Skills follow the [AgentSkills.io](https://agentskills.io) standard. They provide reusable instructions and scripts the agent can load on demand.

Discovery searches these directories in order:
1. `./skills/` (project-local)
2. `~/.remix/skills/` (user-global)
3. `--skills-dir` CLI flag or `REMIX_SKILLS_DIR` env var
4. YAML `skills.dirs` entries
5. Skills contributed by plugins

Three virtual tools are added when skills are discovered:
- `load_skill` -- Load a skill's instructions into context
- `run_skill_script` -- Execute a script from a skill's `scripts/` directory
- `read_skill_resource` -- Read a file from a skill's directory

Disable with `--no-skills`.

### Plugins

The plugin system extends the agent with additional skills, MCP servers, hooks, and agents from external sources. Plugins are discovered from three sources:

1. **Claude Code cache** (`~/.claude/plugins/installed_plugins.json`) -- automatically discovers plugins installed by Claude Code
2. **Local directories** -- point to a plugin directory on disk via config or `--plugins-dir`
3. **GitHub repositories** -- clone and cache a plugin repo via config

A plugin is a directory containing any combination of:

```
my-plugin/
├── skills/           # Skill definitions (merged into SkillSet)
│   └── my-skill/
│       └── SKILL.md
├── hooks/            # Pre/post tool-use hooks
│   └── hooks.json
├── agents/           # Agent definitions (injected into system prompt)
│   └── researcher.md
└── .mcp.json         # MCP server configuration
```

Each component type can be individually enabled or disabled via `plugins.components` in the YAML config.

#### Hooks

Hooks fire shell commands before and/or after tool calls. They receive JSON context via stdin containing the tool name, input arguments, and (for post-hooks) the tool output. Hook failures are logged and never block the agent loop.

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "navigate|click",
        "hooks": [{ "type": "command", "command": "echo pre-hook ran" }]
      }
    ],
    "PostToolUse": [
      {
        "matcher": "screenshot",
        "hooks": [{ "type": "command", "command": "./process-screenshot.sh" }]
      }
    ]
  }
}
```

Matchers use regex patterns (pipe-separated alternatives, anchored to full tool name).

#### Plugin agents

Agent definitions are markdown files with YAML frontmatter. Discovered agents are injected into the system prompt so the LLM knows they are available:

```markdown
---
name: researcher
description: Searches the web for information
model: claude-sonnet-4-20250514
tools:
  - web_search
  - read_file
---
# Researcher Agent

You are a research specialist...
```

#### CLI flags

| Flag | Env Var | Description |
|------|---------|-------------|
| `--no-plugins` | -- | Disable all plugin discovery |
| `--plugins-dir <PATH>` | `REMIX_PLUGINS_DIR` | Additional plugin directory |
| `--no-claude-plugins` | -- | Disable Claude Code plugin cache discovery |

### Multi-agent coordination

The agent can spawn and coordinate multiple child agents to work on tasks in parallel. A lead agent breaks work into subtasks, assigns them to workers, and collects results -- all through seven virtual tools:

| Tool | Description |
|------|-------------|
| `task_create` | Create a new task with subject, description, and metadata |
| `task_list` | List all tasks with their status and ownership |
| `task_get` | Get full details for a specific task |
| `task_update` | Update task status, subject, description, or dependencies |
| `team_create` | Create a named team of agents |
| `send_message` | Send a message to another agent's inbox |
| `spawn_agent` | Spawn a new worker agent with a name, task, and optional tool filter |

**Workflow**: The lead agent creates a team, creates tasks, spawns workers to claim and execute them, communicates via `send_message`, and workers mark tasks complete when done. Workers check their inbox between loop iterations and receive messages as injected context.

All coordination state (tasks, teams, inboxes) is persisted to disk with atomic writes for crash safety.

```yaml
coordination:
  enabled: true
  max_workers: 5
  max_worker_iterations: 10
  worker_timeout_secs: 120
  storage_dir: ~/.remix/coordination
```

Disable with `--no-coordination`. Override worker limits with `--max-workers` and storage location with `--coordination-dir`.

### Sessions

Sessions persist the full conversation history so you can resume or fork previous runs.

Each session is stored at `~/.remix/sessions/{session_id}/` containing:
- `metadata.json` -- session ID, status, timestamps, task description
- `messages.jsonl` -- append-only log of all LLM messages
- `steps.json` -- structured record of every tool call and result

| Action | CLI |
|--------|-----|
| Resume a session | `remix-agent run --session-id <ID> "continue the task"` |
| Fork from a session | `remix-agent run --fork-session <ID> "try a different approach"` |
| Custom storage dir | `remix-agent run --session-dir /path/to/sessions` |

```yaml
session:
  enabled: true
  storage_dir: ~/.remix/sessions
  max_sessions: 100
```

### Permissions

Permissions control which tools the agent can call without user confirmation.

| Mode | Description |
|------|-------------|
| `default` | Ask the user before each tool call |
| `accept_edits` | Auto-allow write tools (write_file, edit_file, bash), ask for others |
| `bypass_permissions` | Allow all tools without asking |
| `plan` | Read-only mode -- only allows read_file, grep, glob, load_skill, read_skill_resource |

Policy evaluation order: `bypass_permissions` > `plan` mode > `denied_tools` (regex) > `allowed_tools` (regex) > ask user.

```bash
# Run in plan mode (read-only exploration)
remix-agent run --permission-mode plan "Analyze the codebase structure"

# Auto-allow specific tools
remix-agent run --allow-tool "navigate|click|screenshot" "Take screenshots of each page"

# Deny dangerous tools
remix-agent run --deny-tool "bash|write_file" "Read and summarize the logs"
```

```yaml
permissions:
  mode: default
  allowed_tools:
    - "navigate|click|screenshot"
  denied_tools:
    - "bash"
```

### Context compaction

When the conversation approaches the model's context window limit, the agent automatically compacts older messages into a summary. This allows long-running tasks to continue without hitting token limits.

- **Trigger**: When `total_input_tokens >= trigger_threshold * context_window_tokens`
- **Process**: Older messages are summarized by the LLM and replaced with a compact `<summary>` block
- **Preservation**: The most recent N messages are always kept intact

```yaml
compaction:
  enabled: true
  trigger_threshold: 0.95
  context_window_tokens: 200000
  preserve_recent_n: 4
```

### Webhooks

Get notified when tasks complete or fail:

```yaml
on_complete:
  url: "https://your-server.com/task-done"
  format: "json"

on_error:
  url: "https://your-server.com/task-failed"
  format: "json"
```

## Output

The agent produces structured JSON output with a full record of every step:

```json
{
  "status": "success",
  "result": "Found the login button and signed in successfully",
  "total_iterations": 3,
  "total_duration_ms": 8420,
  "steps": [
    {
      "iteration": 1,
      "tool": "navigate",
      "input": { "url": "https://example.com" },
      "output": { "title": "Example" },
      "duration_ms": 3200
    },
    {
      "iteration": 2,
      "tool": "click",
      "input": { "selector": "#login-btn" },
      "output": { "success": true },
      "duration_ms": 1890
    }
  ]
}
```

## Available browser tools

The agent has access to all tools exposed by [remix-browser](https://github.com/hkd987/remix-browser):

| Category | Tools |
|----------|-------|
| **Navigation** | `navigate`, `go_back`, `go_forward`, `reload`, `get_page_info` |
| **DOM** | `find_elements`, `get_text`, `get_html`, `wait_for` |
| **Interaction** | `click`, `type_text`, `hover`, `select_option`, `press_key`, `scroll` |
| **Screenshots** | `screenshot` (viewport, full page, or element) |
| **JavaScript** | `execute_js`, `read_console` |
| **Network** | `network_enable`, `get_network_log` |
| **Tabs** | `new_tab`, `close_tab`, `list_tabs` |

Elements can be targeted with CSS selectors, text content, or XPath expressions.

## Development

```bash
# Build
cargo build --release

# Run all tests (sequential to avoid env var conflicts)
cargo test -- --test-threads=1

# Lint
cargo clippy -- -D warnings

# Format check
cargo fmt --check
```

## Architecture

The runtime uses a **decorator chain** pattern where each layer intercepts tool calls it owns and delegates everything else to the next layer:

```
CoordinationExecutor          ← multi-agent coordination (7 tools)
  └─ PermissionAwareExecutor  ← permission checking (4 modes)
       └─ HookAwareExecutor   ← fires pre/post hooks around every tool call
            └─ LocalToolsExecutor  ← intercepts read_file, write_file, edit_file, bash, grep, glob
                 └─ SkillAwareExecutor  ← intercepts load_skill, run_skill_script, read_skill_resource
                      └─ CompositeToolExecutor  ← routes to MCP backends (remix-browser, plugins)
```

All components implement the `ToolExecutor` trait, making every layer independently testable with mocks. The `LlmProvider` trait abstracts the LLM HTTP client for the same reason.

```
src/
├── main.rs                    # CLI entry point, decorator chain wiring
├── cli.rs                     # Argument parsing (clap)
├── lib.rs                     # Public module re-exports
├── error.rs                   # Error types and exit codes
├── agent/
│   ├── loop_impl.rs           # Core agent loop (AgentRunner)
│   ├── state.rs               # Message history + step recording
│   ├── compaction.rs          # Context compaction logic
│   └── compaction_prompt.rs   # Compaction system prompt
├── agents_md/
│   ├── mod.rs                 # Public API re-exports
│   └── discovery.rs           # AGENTS.md walk + injection
├── browser/
│   ├── mcp.rs                 # MCP client + ToolExecutor trait definition
│   ├── manager.rs             # Browser process lifecycle
│   └── convert.rs             # MCP → Anthropic schema conversion
├── config/
│   ├── mod.rs                 # Config merging (CLI > env > YAML > defaults)
│   ├── schema.rs              # AppConfig, LlmConfig, PluginsConfig, etc.
│   ├── credentials.rs         # Credential adapter (RawCredential → CredentialSet)
│   └── env.rs                 # ${VAR} interpolation
├── coordination/
│   ├── mod.rs                 # Public API re-exports
│   ├── context.rs             # CoordinationContext (shared state)
│   ├── executor.rs            # CoordinationExecutor decorator
│   ├── shared_executor.rs     # SharedToolExecutor for worker agents
│   ├── task_types.rs          # Task, TaskStatus, TaskId
│   ├── task_store.rs          # TaskStore (RwLock + file persistence)
│   ├── team_types.rs          # Team, TeamId, WorkerInfo
│   ├── team_store.rs          # TeamStore (RwLock + file persistence)
│   ├── inbox_types.rs         # InboxMessage, InboxId
│   └── inbox_store.rs         # InboxStore (RwLock + file persistence)
├── llm/
│   ├── client.rs              # Anthropic HTTP client with retry
│   └── types.rs               # Message, ContentBlock, ToolDefinition
├── local_tools/
│   ├── mod.rs                 # Public API re-exports
│   ├── executor.rs            # LocalToolsExecutor decorator
│   ├── sandbox/
│   │   ├── mod.rs             # BashSandbox trait + factory
│   │   ├── path_validator.rs  # Sandbox path enforcement
│   │   ├── seatbelt.rs        # macOS sandbox-exec wrapper
│   │   └── landlock.rs        # Linux Landlock LSM wrapper
│   └── tools/
│       ├── mod.rs             # Tool module re-exports
│       ├── read_file.rs       # read_file tool
│       ├── write_file.rs      # write_file tool
│       ├── edit_file.rs       # edit_file tool
│       ├── bash.rs            # bash tool
│       ├── grep.rs            # grep tool
│       └── glob_tool.rs       # glob tool
├── output/
│   ├── result.rs              # AgentResult, StepRecord
│   └── webhook.rs             # Webhook dispatcher
├── permissions/
│   ├── mod.rs                 # Public re-exports
│   ├── types.rs               # PermissionMode, PermissionPolicy
│   └── executor.rs            # PermissionAwareExecutor decorator
├── plugins/
│   ├── mod.rs                 # Public re-exports
│   ├── types.rs               # PluginSet, ResolvedPlugin, PluginComponents
│   ├── discovery.rs           # discover_all_plugins, resolve_local_dir
│   ├── github.rs              # Git clone/update for GitHub plugins
│   ├── composite_executor.rs  # CompositeToolExecutor (multi-backend routing)
│   ├── hook_executor.rs       # HookAwareExecutor decorator
│   └── components/
│       ├── skills.rs          # merge_plugin_skills into SkillSet
│       ├── hooks.rs           # HookRegistry, hooks.json parsing
│       ├── agents.rs          # Agent .md parsing + system prompt injection
│       └── mcp.rs             # Plugin MCP server configuration
├── session/
│   ├── mod.rs                 # Public re-exports
│   ├── types.rs               # SessionId, SessionMetadata, SessionSnapshot
│   └── store.rs               # SessionStore (create, load, fork, append)
├── skills/
│   ├── mod.rs                 # Public API re-exports
│   ├── discovery.rs           # Skill discovery + SKILL.md parsing
│   ├── executor.rs            # SkillAwareExecutor decorator
│   └── types.rs               # SkillSet, SkillEntry, SkillMetadata
└── subagent/
    ├── mod.rs                 # Public re-exports
    ├── types.rs               # SubagentDefinition, SpawnRequest
    ├── executor.rs            # SubagentExecutor decorator
    └── filtered_executor.rs   # FilteredToolExecutor (regex tool filtering)
```

## License

MIT
