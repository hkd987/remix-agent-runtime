"""Harbor agent adapter for remix-agent runtime."""

from __future__ import annotations

import json
import logging
import os
from pathlib import Path

from harbor.agents.context import AgentContext
from harbor.agents.installed.base import BaseInstalledAgent
from harbor.agents.installed.types import ExecInput

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
        api_key = os.environ.get("ANTHROPIC_API_KEY", "")
        if not api_key:
            logger.warning(
                "ANTHROPIC_API_KEY is not set; remix-agent will likely fail."
            )

        model = _strip_provider_prefix(self.model_name)

        env: dict[str, str] = {
            **os.environ,
            "REMIX_LLM_API_KEY": api_key,
            "REMIX_LLM_MODEL": model,
        }

        cmd = (
            "remix-agent run"
            " --no-browser"
            " --permission-mode bypass-permissions"
            " --max-iterations 200"
            f" --output {OUTPUT_FILE}"
            f" --session-dir {LOGS_DIR / 'sessions'}"
            " --verbose"
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
