"""Harbor agent adapter for remix-agent runtime."""

from __future__ import annotations

import json
import logging
import os
from pathlib import Path

from harbor.agents.installed.base import BaseInstalledAgent, ExecInput
from harbor.models.agent.context import AgentContext

logger = logging.getLogger(__name__)

LOGS_DIR = Path("/logs/agent")
OUTPUT_FILE = LOGS_DIR / "output.json"
TRAJECTORY_FILE = LOGS_DIR / "trajectory.jsonl"


def _strip_provider_prefix(model_name: str) -> str:
    """Strip provider prefix from model name.

    Harbor uses 'anthropic/claude-sonnet-4-6' format,
    remix-agent uses 'claude-sonnet-4-6'.
    """
    if "/" in model_name:
        return model_name.split("/", maxsplit=1)[1]
    return model_name


class RemixAgent(BaseInstalledAgent):
    """Harbor adapter for the remix-agent runtime."""

    @staticmethod
    def name() -> str:
        return "remix-agent"

    @property
    def _install_agent_template_path(self) -> Path:
        return Path(__file__).parent / "templates" / "install-remix-agent.sh.j2"

    def create_run_agent_commands(self, instruction: str) -> list[ExecInput]:
        # Support both REMIX_LLM_API_KEY (OpenRouter) and ANTHROPIC_API_KEY (direct)
        api_key = os.environ.get("REMIX_LLM_API_KEY") or os.environ.get("ANTHROPIC_API_KEY", "")
        if not api_key:
            logger.warning(
                "Neither REMIX_LLM_API_KEY nor ANTHROPIC_API_KEY is set; remix-agent will likely fail."
            )

        model = _strip_provider_prefix(self.model_name)

        env: dict[str, str] = {
            "REMIX_LLM_API_KEY": api_key,
            "REMIX_LLM_MODEL": model,
            "HOME": os.environ.get("HOME", "/root"),
            "PATH": "/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        }

        # Forward base URL if set (e.g., for OpenRouter)
        base_url = os.environ.get("REMIX_LLM_BASE_URL", "")
        if base_url:
            env["REMIX_LLM_BASE_URL"] = base_url

        # System prompt in XML format per Anthropic best practices
        system_prompt = (
            "<environment_discovery>\n"
            "Before starting work, discover your environment:\n"
            "<steps>\n"
            "<step>List the working directory structure (2 levels deep)</step>\n"
            "<step>Check available tools (python --version, node --version, cargo --version, etc.)</step>\n"
            "<step>Read any README, Makefile, or build configuration files</step>\n"
            "<step>Identify the project type and build system</step>\n"
            "</steps>\n"
            "Use this context to inform your approach.\n"
            "</environment_discovery>\n\n"
            "<workflow>\n"
            "Follow this cycle for every task:\n"
            "<phase name=\"PLAN\">Read the task fully. Scan all relevant files. Identify how you will verify your solution. Never skip to BUILD without reading existing code first.</phase>\n"
            "<phase name=\"BUILD\">Implement the solution. Write tests alongside code when possible.</phase>\n"
            "<phase name=\"VERIFY\">Run tests, linters, and type checkers. Compare output against the original requirements — not against your own code.</phase>\n"
            "<phase name=\"FIX\">If anything fails, analyze the error, revisit the original spec, and fix. Return to VERIFY.</phase>\n"
            "</workflow>\n\n"
            "<validation_requirements>\n"
            "<rule>Your solution will be validated programmatically against tests you cannot see.</rule>\n"
            "<rule>Prioritize: exact specification adherence, edge cases, boundary conditions, error handling.</rule>\n"
            "<rule>Do not assume lenient validation.</rule>\n"
            "<rule>Build and run your own tests before finishing.</rule>\n"
            "<rule>If the task provides test or evaluation scripts, run them EARLY and OFTEN during development — not just at the end.</rule>\n"
            "<rule>Always verify edge cases: small inputs, boundary conditions, and performance requirements on ALL input sizes, not just large ones.</rule>\n"
            "</validation_requirements>"
        )

        cmd = (
            "remix-agent run"
            " --no-browser"
            " --no-coordination"
            " --permission-mode bypass_permissions"
            " --timeout 1800"
            " --max-iterations 200"
            " --tool-result-max-bytes 16384"
            " --nudge-on-text-only"
            " --goal-check-on-complete"
            " --action-reminder-interval 8"
            " --deny-tool 'web_fetch'"
            # Loop detection: catch doom-loops early
            " --loop-detection"
            " --loop-detection-max-repeats 3"
            " --loop-detection-window 10"
            # Reasoning stages: plan deep, execute fast, verify carefully
            " --reasoning-stages"
            " --thinking-budget-tokens 10000"
            " --planning-budget-tokens 10000"
            " --execution-budget-tokens 5000"
            " --verification-budget-tokens 10000"
            # Budget warning at 70% of iterations
            " --iteration-budget-warning-threshold 0.7"
            f" --output {OUTPUT_FILE}"
            f" --session-dir {LOGS_DIR / 'sessions'}"
            " --verbose"
            f" --system-prompt {json.dumps(system_prompt)}"
            f" {json.dumps(instruction)}"
        )

        return [
            ExecInput(
                command=cmd,
                env=env,
                timeout_sec=3600,
            ),
        ]

    def populate_context_post_run(self, context: AgentContext) -> None:
        """Parse output.json for token usage and cost, write trajectory."""
        if not OUTPUT_FILE.exists():
            logger.warning("Output file not found at %s", OUTPUT_FILE)
            return

        try:
            raw = OUTPUT_FILE.read_text(encoding="utf-8")
            data: dict[str, object] = json.loads(raw)
        except (json.JSONDecodeError, OSError) as exc:
            logger.error("Failed to parse output file: %s", exc)
            return

        input_tokens = data.get("total_input_tokens")
        output_tokens = data.get("total_output_tokens")
        cost_usd = data.get("total_cost_usd")

        if isinstance(input_tokens, (int, float)):
            context.n_input_tokens = int(input_tokens)
        if isinstance(output_tokens, (int, float)):
            context.n_output_tokens = int(output_tokens)
        if isinstance(cost_usd, (int, float)):
            context.cost_usd = float(cost_usd)

        # Write trajectory as JSONL from the steps array
        steps = data.get("steps")
        if isinstance(steps, list):
            self._write_trajectory(steps)

    @staticmethod
    def _write_trajectory(steps: list[dict[str, object]]) -> None:
        """Write agent steps as JSONL trajectory for Harbor collection."""
        try:
            TRAJECTORY_FILE.parent.mkdir(parents=True, exist_ok=True)
            with TRAJECTORY_FILE.open("w", encoding="utf-8") as fh:
                for step in steps:
                    fh.write(json.dumps(step, default=str) + "\n")
        except OSError as exc:
            logger.error("Failed to write trajectory file: %s", exc)
