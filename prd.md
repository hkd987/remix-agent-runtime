# remix-agent-runtime — Product Requirements Document

**Version:** 0.1.0-draft (implementation is at v0.4.11)
**Date:** February 7, 2026
**Status:** MVP shipped. See [MVP Scope](#mvp-scope) for what was built and what
was deferred; this document otherwise still reads as the pre-implementation spec.

---

## Business Context

remix-agent-runtime is one piece of a three-part product strategy built around [remix-browser](https://github.com/hkd987/remix-browser), a Rust-native MCP server for headless Chrome automation via CDP.

### The Three Products

| Product | License | Purpose |
|---|---|---|
| **remix-browser** | Open source (MIT) | The browser engine. MCP server that gives AI agents full control over Chrome via CDP. Single Rust binary, no Node.js, no Puppeteer. Already built and shipping. |
| **remix-agent-runtime** | Open source (MIT) | The agent execution loop. Takes a task, calls an LLM with remix-browser tools, executes tool calls, loops until done. This document. |
| **remix-agent-platform** | Closed source (proprietary) | The managed service. Hosting, scheduling, scaling, billing, CAPTCHA solving, stealth/anti-detection, session persistence, observability dashboard. Future product. |

### Why This Matters

The market for AI agents that interact with the real web is growing fast (browser-use has 85K+ GitHub stars), but the infrastructure layer is underserved. Existing solutions (Browserless, Browserbase) were built for developers writing automation scripts — they speak WebSocket/REST to Puppeteer/Playwright clients. They are not designed for LLM agents that reason about web pages through tool calls.

remix-browser is the only Rust-native, MCP-native, CDP-direct browser automation tool. remix-agent-runtime completes the stack by providing the agent loop that connects any LLM to remix-browser's tools.

The open source runtime drives adoption and trust. The closed source platform drives revenue. Users who start with the runtime locally graduate to the managed platform when they need scale, scheduling, stealth, and reliability.

### Competitive Positioning

- **vs. Browserless/Browserbase:** They provide remote browsers for developers. We provide an agent platform for LLMs. Different buyer, different integration pattern.
- **vs. browser-use (Python):** They're a library. We're infrastructure. They require Python + Playwright. We're a single binary.
- **vs. Apify:** They run imperative scraping scripts. We run LLM-driven agents. Their Actors are code. Our agents are natural language tasks.

### Key Strategic Decisions for Developers

When making tradeoffs during development, keep these priorities in mind:

1. **Trust over features.** This runtime handles user API keys and site credentials. Every architectural decision should favor transparency and security. If you're unsure whether something should be configurable or hardcoded, make it configurable.
2. **LLM-agnostic over LLM-optimized.** Supporting any Anthropic-compatible API (OpenRouter, Ollama, LiteLLM, etc.) via configurable base URL is more important than deep integration with any single provider.
3. **Language-agnostic interface over Rust-only API.** The runtime is written in Rust for operational benefits (memory, startup, single binary). But the *user interface* is HTTP + CLI + config files. A TypeScript or Python developer should never need to write or read Rust.
4. **Operational efficiency matters.** The managed platform will run thousands of these concurrently. Memory footprint per instance, startup time, and binary size directly affect unit economics. Favor lean implementations.

---

## Product Overview

remix-agent-runtime is a standalone Rust binary that executes LLM-driven browser automation tasks. It connects an LLM (via the Anthropic messages API format) to remix-browser's MCP tools and runs the agent loop: send task → get tool calls → execute tools → return results → repeat until done.

### Core Value Proposition

Users define what they want done in natural language. The runtime figures out how to do it by letting the LLM reason about and use browser tools. No Puppeteer scripts, no CSS selectors, no imperative code required.

### How It Works

```
┌──────────────┐     ┌─────────────────────┐     ┌───────────────┐
│   User Task  │────▶│ remix-agent-runtime  │────▶│ remix-browser │
│  (YAML/API)  │     │   (agent loop)       │     │  (MCP server) │
└──────────────┘     │                      │     │               │
                     │  1. Send task to LLM │     │  - navigate   │
┌──────────────┐     │  2. Receive tool     │     │  - click      │
│  LLM Provider│◀───▶│     calls            │     │  - find       │
│  (any base   │     │  3. Execute via      │────▶│  - type       │
│   URL)       │     │     remix-browser    │     │  - screenshot │
└──────────────┘     │  4. Return results   │     │  - execute_js │
                     │  5. Loop until done  │     │  - network    │
                     └─────────────────────┘     └───────────────┘
```

---

## Requirements

### R1: Agent Execution Loop

The core loop that sends messages to an LLM and executes tool calls against remix-browser.

**Must:**
- Implement the Anthropic messages API request/response cycle with tool use
- Register all remix-browser MCP tools (navigate, click, find_elements, type_text, screenshot, execute_js, get_text, get_html, wait_for, hover, select_option, press_key, scroll, read_console, network_enable, get_network_log, new_tab, close_tab, list_tabs, go_back, go_forward, reload, get_page_info) as tools in the LLM request
- Parse `tool_use` content blocks from LLM responses
- Execute tool calls against a remix-browser instance (spawned as a child process or connected via existing MCP transport)
- Send `tool_result` content blocks back to the LLM
- Loop until the LLM returns a final text response with no tool calls, or a max iteration / timeout limit is hit
- Support streaming responses from the LLM for real-time progress visibility
- Handle errors gracefully — if a tool call fails, send the error back to the LLM as a tool_result so it can retry or adapt

**Should:**
- Support a configurable system prompt that provides the agent with context about its task, constraints, and available tools
- Allow a max_iterations limit (default: 50) and max_duration timeout (default: 5 minutes) to prevent runaway agents
- Emit structured log events for each step (tool call, tool result, LLM response) for observability
- Support passing an initial set of messages (conversation history) to enable multi-turn or resumed sessions

**Won't (v1):**
- Multi-agent orchestration (multiple LLMs collaborating)
- Automatic prompt optimization or retry with different prompts on failure
- Built-in RAG or memory systems

### R2: LLM Provider Configuration

Users must be able to point the runtime at any LLM provider that speaks the Anthropic messages API format.

**Must:**
- Accept a configurable `base_url` (default: `https://api.anthropic.com`)
- Accept an `api_key` via config, environment variable (`REMIX_LLM_API_KEY`), or CLI flag
- Accept a `model` identifier (e.g., `claude-sonnet-4-20250514`, `anthropic/claude-sonnet-4`, `llama3`)
- Never log, persist, or transmit the API key anywhere other than directly to the configured base_url
- Work with: Anthropic API, OpenRouter, Ollama (local), LiteLLM, Together AI, Azure OpenAI (Anthropic-compatible mode), Groq

**Should:**
- Accept optional `max_tokens` configuration (default: 8192)
- Accept optional custom headers for providers that require them
- Validate the base URL and API key with a lightweight request before starting the agent loop, and return a clear error if authentication fails

**Won't (v1):**
- Native support for the OpenAI chat completions format (users should use LiteLLM or OpenRouter as a proxy)
- Token usage tracking or cost estimation

### R3: Task Definition Interface

The runtime must accept task definitions through multiple interfaces to support different user workflows.

#### R3a: CLI Interface

**Must:**
- Accept a task as a direct string argument: `remix-agent run "Find the price of..."`
- Accept a task definition file: `remix-agent run --config agent.yaml`
- Accept LLM configuration via CLI flags: `--base-url`, `--api-key`, `--model`
- Accept LLM configuration via environment variables: `REMIX_LLM_BASE_URL`, `REMIX_LLM_API_KEY`, `REMIX_LLM_MODEL`
- Output results to stdout as JSON
- Exit with appropriate status codes (0 = success, 1 = agent error, 2 = config error)

**Should:**
- Support a `--verbose` flag that streams agent steps to stderr in real-time
- Support a `--headed` flag that passes through to remix-browser for visual debugging
- Support a `--timeout` flag to override default max duration
- Support `--output` flag for writing results to a file

#### R3b: Configuration File (YAML)

**Must:**
- Support this minimal schema:

```yaml
# Required
task: |
  Navigate to https://example.com
  Find the main product price
  Return the price as JSON

# Required (can also be set via env vars or CLI flags)
llm:
  base_url: https://api.anthropic.com
  api_key: ${REMIX_LLM_API_KEY}  # env var interpolation
  model: claude-sonnet-4-20250514

# Optional
browser:
  headless: true          # default: true
  timeout: 120s           # max session duration, default: 300s
  viewport:
    width: 1280           # default: 1280
    height: 720           # default: 720

# Optional
agent:
  max_iterations: 50      # default: 50
  system_prompt: |        # prepended to task context
    You are a price monitoring agent.
    Always return results as JSON.

# Optional
on_complete:
  webhook: https://hooks.slack.com/...
  format: json            # json | text
```

**Should:**
- Support environment variable interpolation in any string field using `${VAR_NAME}` syntax
- Support a `variables` block for user-defined variables that can be referenced in the task:

```yaml
variables:
  urls:
    - https://example.com/product/1
    - https://example.com/product/2
task: |
  Visit each URL in {{urls}} and extract the product price.
```

#### R3c: HTTP API

**Must:**
- Expose a local HTTP server (default port: 9090, configurable via `--port`)
- Accept POST requests to `/run` with a JSON body matching the YAML schema above
- Return results as JSON
- Support a `/health` endpoint for liveness checks

**Should:**
- Support Server-Sent Events (SSE) on `/run/stream` for real-time step-by-step progress
- Support a `/stop` endpoint to cancel a running agent
- Support concurrent agent runs (configurable max concurrency, default: 1)

**Won't (v1):**
- Authentication on the HTTP API (this is a local-only interface; the managed platform adds auth)
- WebSocket transport

### R4: remix-browser Integration

**Must:**
- Spawn remix-browser as a child process if not already running
- Communicate with remix-browser via MCP stdio transport
- Pass through `--headed` flag to remix-browser when requested
- Clean up browser process on agent completion or timeout
- Handle remix-browser crashes gracefully — report the error to the LLM or abort with a clear message

**Should:**
- Support connecting to an existing remix-browser instance via configurable MCP transport
- Pass through browser configuration (viewport size, headless mode) from the agent config

**Won't (v1):**
- Managing multiple remix-browser instances per agent (one browser per agent)
- Remote remix-browser connections over network (local only; the managed platform handles remote)

### R5: Credential Handling

**Must:**
- Accept site credentials via the config file or environment variables
- Credentials must only exist in memory during the agent session — never written to disk, never logged
- Credentials are injected into the agent's system prompt so the LLM can use them in browser interactions (e.g., typing a username/password)
- Support this config format:

```yaml
credentials:
  - name: example_login
    username: ${EXAMPLE_USER}
    password: ${EXAMPLE_PASS}
    url_pattern: "*.example.com"   # optional, for documentation
```

**Should:**
- Support a `--credentials-file` flag pointing to a separate encrypted credentials file
- Clear credential memory on agent completion (zeroize)
- Warn if credentials are provided without HTTPS in the target URLs

**Won't (v1):**
- A built-in credential vault or keychain integration (managed platform feature)
- OAuth flow handling
- 2FA/MFA automation

### R6: Hook System

Pre/post execution hooks allow users to extend the runtime in any language without writing Rust.

**Must:**
- Support `on_complete` webhook: HTTP POST with agent results to a configurable URL
- Support `on_error` webhook: HTTP POST with error details to a configurable URL

**Should:**
- Support `before_task` hook: execute a shell command or HTTP call before the agent loop starts (e.g., setup scripts)
- Support `on_step` hook: HTTP POST after each agent iteration with the current step details (tool call, result)
- Support `after_task` hook: execute a shell command after the agent loop finishes (e.g., cleanup scripts)
- Hook format for shell: `{ command: "./scripts/setup.sh", timeout: 30s }`
- Hook format for HTTP: `{ url: "http://localhost:3000/hook", method: "POST", timeout: 10s }`

**Won't (v1):**
- Hooks that can modify agent behavior mid-execution (hooks are notification-only, not interceptors)
- Built-in integrations (Slack, email, etc.) — users build these via webhooks

### R7: Output and Observability

**Must:**
- Return the final agent result as structured JSON:

```json
{
  "status": "success" | "error" | "timeout" | "max_iterations",
  "result": "...",
  "steps": [
    {
      "iteration": 1,
      "tool": "navigate",
      "input": { "url": "https://example.com" },
      "output": { "success": true },
      "duration_ms": 1200
    }
  ],
  "total_iterations": 5,
  "total_duration_ms": 15000
}
```

- Log all agent activity to stderr with configurable verbosity (via `RUST_LOG` env var)

**Should:**
- Support an optional `--record` flag that saves screenshots at each step to a directory for debugging
- Include token usage per step in the output if the LLM provider returns it
- Support a session recording format (JSON lines) that can be replayed for debugging

**Won't (v1):**
- A web-based debug viewer (managed platform feature)
- Video recording of browser sessions

---

## Technical Architecture

### Language and Runtime

- **Language:** Rust
- **Async runtime:** Tokio
- **HTTP client:** reqwest (for LLM API calls and webhooks)
- **JSON handling:** serde / serde_json
- **MCP communication:** rmcp crate (same as remix-browser)
- **CLI framework:** clap
- **HTTP server:** axum (for the local API)

### Binary Structure

```
remix-agent-runtime (single binary)
├── main.rs              # CLI entry point (clap)
├── config/
│   ├── mod.rs           # Config loading (YAML, env vars, CLI merge)
│   ├── schema.rs        # Config types and validation
│   └── credentials.rs   # Credential handling with zeroize
├── agent/
│   ├── loop.rs          # Core agent execution loop
│   ├── messages.rs      # Anthropic messages API types
│   └── tools.rs         # Tool call parsing and routing
├── llm/
│   ├── client.rs        # HTTP client for LLM provider
│   └── streaming.rs     # SSE stream parsing for streaming responses
├── browser/
│   ├── manager.rs       # remix-browser process lifecycle
│   └── mcp.rs           # MCP client for tool execution
├── hooks/
│   ├── webhook.rs       # HTTP webhook dispatch
│   └── subprocess.rs    # Shell command execution
├── server/
│   ├── mod.rs           # Axum HTTP API
│   ├── routes.rs        # /run, /run/stream, /health, /stop
│   └── sse.rs           # Server-sent events for streaming
└── output/
    ├── result.rs        # Structured result types
    └── recording.rs     # Step recording and screenshot capture
```

### Process Model

```
┌─────────────────────────────────────┐
│         remix-agent-runtime         │
│                                     │
│  ┌─────────┐    ┌────────────────┐  │
│  │  Config  │───▶│  Agent Loop    │  │
│  │  Loader  │    │                │  │
│  └─────────┘    │  LLM Client ───────────▶ LLM Provider (user's key)
│                 │                │  │
│                 │  MCP Client ───────────▶ remix-browser (child process)
│                 │                │  │         │
│                 │  Hook Runner ──────────▶ Webhooks / Scripts
│                 └────────────────┘  │         │
│                                     │    Chrome (child of remix-browser)
│  ┌─────────┐                        │
│  │  HTTP    │ (optional, --serve)    │
│  │  Server  │                        │
│  └─────────┘                        │
└─────────────────────────────────────┘
```

The runtime spawns remix-browser as a child process. remix-browser spawns Chrome. On shutdown or timeout, the runtime kills the remix-browser process, which kills Chrome. Clean process tree teardown.

### Concurrency Model

In CLI mode, one agent runs at a time. In HTTP server mode (`--serve`), multiple agents can run concurrently, each with their own remix-browser instance. Concurrency is bounded by a configurable limit to prevent resource exhaustion.

Each agent run gets:
- Its own remix-browser child process
- Its own Chrome instance
- Its own LLM conversation state
- Complete isolation from other running agents

---

## User Workflows

### Workflow 1: Quick One-Off Task (CLI)

```bash
export REMIX_LLM_API_KEY=sk-ant-...
remix-agent run "Go to news.ycombinator.com and return the top 5 story titles as JSON"
```

### Workflow 2: Configured Agent (YAML + CLI)

```bash
remix-agent run --config price-monitor.yaml
```

### Workflow 3: Programmatic Execution (HTTP API from any language)

```bash
# Start the runtime as a server
remix-agent serve --port 9090

# Call from any language
curl -X POST http://localhost:9090/run \
  -H "Content-Type: application/json" \
  -d '{
    "task": "Find the cheapest flight from CLT to SJU in April",
    "llm": {
      "base_url": "https://openrouter.ai/api/v1",
      "api_key": "sk-or-...",
      "model": "anthropic/claude-sonnet-4"
    },
    "browser": { "timeout": "300s" }
  }'
```

### Workflow 4: Local Development with Ollama (Free)

```bash
# No API key needed — runs fully local
remix-agent run \
  --base-url http://localhost:11434/v1 \
  --model llama3 \
  "Go to example.com and get the page title"
```

### Workflow 5: Visual Debugging

```bash
remix-agent run --config agent.yaml --headed --verbose --record ./debug-screenshots/
```

---

## Open Source Considerations

This runtime is MIT licensed and open source. It will handle user API keys and credentials in memory. Design decisions must reflect this responsibility:

- **No telemetry, no analytics, no phone-home.** The binary should make zero network requests other than to the user-configured LLM base URL, remix-browser (local), and user-configured webhook URLs.
- **No vendor lock-in.** The configurable base URL is not optional — it's a core design requirement. Do not hardcode any provider.
- **Auditable credential handling.** The credential loading, usage, and zeroization paths should be straightforward to audit in the source code. Use the `zeroize` crate for credential memory. Don't get clever with credential storage.
- **Minimal dependencies.** Every dependency is attack surface. Favor well-known, audited crates. The dependency tree should be reviewable.
- **Reproducible builds.** The CI pipeline should produce reproducible binaries so users can verify that released binaries match the source code.

---

## MVP Scope

For the initial v0.1.0 release, ship:

- [x] Core agent loop with tool execution (R1)
- [x] LLM provider configuration with base URL (R2)
- [x] CLI interface with direct task and YAML config (R3a, R3b)
- [x] remix-browser child process management (R4)
- [x] Basic credential handling via env vars (R5, minimal)
- [x] Structured JSON output (R7)
- [x] `on_complete` webhook (R6, minimal)

Deferred from v0.1.0 — status as of v0.4.11:

- HTTP server mode (R3c) — **not built.** No `serve` subcommand exists. The
  only HTTP surface is the SSE event server.
- SSE streaming — shipped, behind the `sse` Cargo feature.
- Step recording and screenshots (`--record`) — **not built.**
- Full hook system — shipped: tool-level and lifecycle hooks, with hooks able
  to deny a call, rewrite its arguments, or inject a message.
- Credential file encryption — delegated to the external `remix-credentials`
  dependency.
- Variable interpolation in tasks — shipped.

---

## Success Metrics

**Adoption (open source):**
- GitHub stars, forks, and contributors
- Number of unique cloners/installers per month
- Community-built agent configs shared publicly

**Platform funnel (future):**
- Percentage of open source users who try the managed platform
- Conversion from free tier to paid

**Technical:**
- Memory footprint per agent run < 30MB (excluding Chrome)
- Cold start to first LLM request < 500ms
- Binary size < 20MB

---

## Reference Implementations

Before building, study these existing implementations to understand patterns, edge cases, and design decisions:

### Claude Code Agent SDK (`@anthropic-ai/claude-code`)

This is the primary reference. It's Anthropic's official TypeScript SDK for building agentic applications with Claude. It implements the exact agent loop pattern remix-agent-runtime needs: send messages with tools → receive tool_use blocks → execute tools → send tool_result blocks → loop until done.

**What to study:**
- How it manages conversation state (message history accumulation across iterations)
- How it handles streaming responses with interleaved text and tool_use blocks
- Error recovery patterns — what happens when a tool call fails, when the API returns a 429 (rate limit), when the connection drops mid-stream
- How it registers and describes tools to the LLM
- How it handles `stop_reason` values (`end_turn`, `tool_use`, `max_tokens`, `stop_sequence`) and what each means for the loop
- How it manages token budget — the conversation history grows with each iteration, and eventually you hit the context window limit

**What NOT to copy:**
- The TypeScript/Node.js architecture — we're building idiomatic Rust with Tokio, not a port
- Any Claude Code-specific tool implementations (terminal, file editing, etc.) — our only tools are remix-browser's MCP tools
- The CLI/UX patterns — Claude Code is an interactive coding assistant, we're a headless task runner

**Repository:** https://github.com/anthropics/claude-code
**NPM:** https://www.npmjs.com/package/@anthropic-ai/claude-code

### rmcp (Rust MCP Framework)

Already used by remix-browser. Study the client-side MCP implementation since remix-agent-runtime will be an MCP *client* (calling remix-browser's tools), whereas remix-browser is an MCP *server* (exposing tools). The rmcp crate supports both roles.

**Repository:** https://github.com/anthropics/rmcp

### Anthropic API Documentation

The canonical reference for the messages API, tool use, and streaming:

- **Messages API:** https://docs.anthropic.com/en/api/messages
- **Tool Use Guide:** https://docs.anthropic.com/en/docs/build-with-claude/tool-use/overview
- **Streaming:** https://docs.anthropic.com/en/api/messages-streaming

### browser-use (Python, for agent pattern reference only)

An open source Python library that does LLM + browser automation. Architecturally different (uses Playwright, not MCP), but useful for understanding how other projects handle the agent-browser interaction loop, particularly how they structure prompts for browser tasks and handle multi-step navigation failures.

**Repository:** https://github.com/browser-use/browser-use

---

## Appendix: Anthropic Messages API Reference

The agent loop implements this request/response cycle. This is the core protocol the runtime must speak.

### Request

```
POST {base_url}/v1/messages
Headers:
  x-api-key: {api_key}
  content-type: application/json
  anthropic-version: 2023-06-01

Body:
{
  "model": "{model}",
  "max_tokens": 8192,
  "system": "{system_prompt}",
  "tools": [
    {
      "name": "navigate",
      "description": "Go to a URL",
      "input_schema": {
        "type": "object",
        "properties": {
          "url": { "type": "string" }
        },
        "required": ["url"]
      }
    }
    // ... all remix-browser tools
  ],
  "messages": [
    { "role": "user", "content": "{task}" },
    // ... conversation history with tool_use and tool_result blocks
  ]
}
```

### Response (with tool use)

```json
{
  "content": [
    {
      "type": "tool_use",
      "id": "toolu_abc123",
      "name": "navigate",
      "input": { "url": "https://example.com" }
    }
  ],
  "stop_reason": "tool_use"
}
```

### Sending Tool Results Back

```json
{
  "messages": [
    // ... previous messages ...
    { "role": "assistant", "content": [{ "type": "tool_use", "id": "toolu_abc123", "name": "navigate", "input": { "url": "https://example.com" } }] },
    { "role": "user", "content": [{ "type": "tool_result", "tool_use_id": "toolu_abc123", "content": "Successfully navigated to https://example.com" }] }
  ]
}
```

### Terminal Response (no more tool calls)

```json
{
  "content": [
    {
      "type": "text",
      "text": "Here are the results: ..."
    }
  ],
  "stop_reason": "end_turn"
}
```

The loop ends when `stop_reason` is `end_turn` (or `max_tokens`), indicating the LLM has finished and is returning its final answer.