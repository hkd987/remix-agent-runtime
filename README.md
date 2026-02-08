# remix-agent-runtime

LLM-driven browser automation agent runtime. Give it a task in plain English, and it uses an LLM to control a real Chrome browser until the job is done.

`remix-agent-runtime` is the orchestration layer that connects [remix-browser](https://github.com/hkd987/remix-browser) (headless Chrome via MCP) with any LLM provider to create an autonomous browser automation agent. It works with Anthropic, OpenRouter, AWS Bedrock, or any provider compatible with the Anthropic Messages API format. Credentials are secured by [remix-credentials](https://github.com/hkd987/remix-credentials).

## How it works

```
┌─────────────────────────────────────────────────────────┐
│                    remix-agent-runtime                   │
│                                                         │
│   "Log into GitHub and star the remix-browser repo"     │
│                          │                              │
│                          ▼                              │
│  AGENTS.md ──► ┌────────────────┐ ◄── Credentials       │
│  instructions  │   Agent Loop   │     (remix-           │
│                └───────┬────────┘      credentials)     │
│                   ▲    │                                │
│          results  │    │ tool calls                      │
│                   │    ▼                                │
│    ┌──────────────┴──────────────────────┐              │
│    │          Tool Router                 │              │
│    │  ┌─────────┐  ┌──────┐  ┌────────┐ │              │
│    │  │ Browser  │  │Local │  │ Skills │ │              │
│    │  │  (MCP)   │  │Tools │  │        │ │              │
│    │  └────┬─────┘  └──┬───┘  └────────┘ │              │
│    └───────┼────────────┼────────────────┘              │
│            │            │                               │
│  ┌─────────┘     ┌──────┘                              │
│  │               │                                      │
│  ▼               ▼                                      │
│  remix-browser   Sandboxed filesystem                   │
│  (MCP Server)    (Seatbelt/Landlock)                   │
│       │                                                 │
└───────┼─────────────────────────────────────────────────┘
        │ CDP
        ▼
  ┌──────────────┐
  │    Chrome     │
  └──────────────┘
```

1. You provide a task in natural language
2. The agent sends the task + available browser tools to the LLM
3. The LLM decides which tools to call (navigate, click, type, screenshot, etc.)
4. The agent executes those tools against a real Chrome browser via [remix-browser](https://github.com/hkd987/remix-browser)
5. Results go back to the LLM, which decides the next action
6. Loop continues until the task is complete or a stopping condition is hit
7. Structured JSON output with every step recorded

## The remix ecosystem

| Project | Role |
|---------|------|
| [remix-browser](https://github.com/hkd987/remix-browser) | Rust-native MCP server for Chrome automation — 18+ tools for navigation, clicking, typing, screenshots, network monitoring, and more |
| [remix-credentials](https://github.com/hkd987/remix-credentials) | Secure credential management with AES-256-GCM encryption, Argon2id key derivation, and zeroizable memory |
| **remix-agent-runtime** (this project) | The agent loop that ties it all together — connects an LLM to browser tools and runs autonomously |

## Quick start

### Prerequisites

- Google Chrome or Chromium
- An API key from a supported LLM provider (Anthropic, OpenRouter, AWS Bedrock, etc.)

### Install

One command installs both `remix-agent` and `remix-browser` — no Rust toolchain needed:

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
| `--config <PATH>` | `-c` | — | Path to YAML configuration file |
| `--api-key <KEY>` | — | `REMIX_LLM_API_KEY` | LLM provider API key |
| `--base-url <URL>` | — | `REMIX_LLM_BASE_URL` | LLM provider base URL (default: Anthropic) |
| `--model <NAME>` | — | `REMIX_LLM_MODEL` | Model ID (default: `claude-sonnet-4-20250514`) |
| `--max-tokens <N>` | — | — | Max tokens per response (default: 8192) |
| `--timeout <SECS>` | — | — | Max duration in seconds |
| `--max-iterations <N>` | — | — | Max agent loop iterations (default: 50) |
| `--headed` | — | — | Show the browser window |
| `--verbose` | `-v` | — | Debug logging to stderr |
| `--output <PATH>` | `-o` | — | Write JSON results to file |
| `--browser-path <PATH>` | — | `REMIX_BROWSER_PATH` | Path to remix-browser binary |
| `--agents-md-dir <PATH>` | — | `REMIX_AGENTS_MD_DIR` | Override AGENTS.md search directory |
| `--no-agents-md` | — | — | Disable AGENTS.md discovery |
| `--no-local-tools` | — | — | Disable local filesystem tools |
| `--sandbox-dir <PATH>` | — | `REMIX_SANDBOX_DIR` | Sandbox root for local tools |
| `--skills-dir <PATH>` | — | `REMIX_SKILLS_DIR` | Additional skills directory |
| `--no-skills` | — | — | Disable skill discovery |

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

on_complete:
  url: "https://hooks.slack.com/your-webhook"
  format: "json"

on_error:
  url: "https://hooks.slack.com/your-error-webhook"
  format: "json"
```

Environment variables can be interpolated in YAML using `${VAR_NAME}` syntax.

### Credentials

Credentials are securely managed via [remix-credentials](https://github.com/hkd987/remix-credentials) — values use zeroizable memory and are redacted from logs.

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

Three virtual tools are added when skills are discovered:
- `load_skill` -- Load a skill's instructions into context
- `run_skill_script` -- Execute a script from a skill's `scripts/` directory
- `read_skill_resource` -- Read a file from a skill's directory

Disable with `--no-skills`.

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

```
src/
├── main.rs                 # CLI entry point, config loading
├── cli.rs                  # Argument parsing (clap)
├── error.rs                # Error types and exit codes
├── agents_md/
│   ├── mod.rs              # Public API re-exports
│   └── discovery.rs        # AGENTS.md walk + injection
├── agent/
│   ├── loop_impl.rs        # Core agent loop (AgentRunner)
│   └── state.rs            # Message history + step recording
├── browser/
│   ├── mcp.rs              # MCP client (tool discovery + execution)
│   ├── manager.rs          # Browser process lifecycle
│   └── convert.rs          # MCP → Anthropic schema conversion
├── llm/
│   ├── client.rs           # Anthropic HTTP client with retry
│   └── types.rs            # Message, ContentBlock, ToolDefinition
├── config/
│   ├── mod.rs              # Config merging (CLI > env > YAML > defaults)
│   ├── schema.rs           # AppConfig, LlmConfig, BrowserConfig
│   ├── credentials.rs      # Credential adapter (RawCredential → CredentialSet)
│   └── env.rs              # ${VAR} interpolation
├── local_tools/
│   ├── mod.rs              # Public API re-exports
│   ├── executor.rs         # LocalToolsExecutor decorator
│   ├── sandbox/
│   │   ├── mod.rs          # BashSandbox trait + factory
│   │   ├── path_validator.rs  # Sandbox path enforcement
│   │   ├── seatbelt.rs     # macOS sandbox-exec wrapper
│   │   └── landlock.rs     # Linux Landlock LSM wrapper
│   └── tools/
│       ├── mod.rs           # Tool module re-exports
│       ├── read_file.rs     # read_file tool
│       ├── write_file.rs    # write_file tool
│       ├── edit_file.rs     # edit_file tool
│       ├── bash.rs          # bash tool
│       ├── grep.rs          # grep tool
│       └── glob_tool.rs     # glob tool
├── skills/
│   ├── mod.rs              # Public API re-exports
│   ├── discovery.rs        # Skill discovery + parsing
│   ├── executor.rs         # SkillAwareExecutor decorator
│   └── types.rs            # SkillSet, SkillEntry, SkillMetadata
└── output/
    ├── result.rs           # AgentResult, StepRecord
    └── webhook.rs          # Webhook dispatcher
```

The agent loop uses `LlmProvider` and `ToolExecutor` traits, making every component independently testable with mocks.

## License

MIT
