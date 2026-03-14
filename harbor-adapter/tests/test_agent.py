"""Tests for the RemixAgent Harbor adapter."""

from __future__ import annotations

import json
import os
from pathlib import Path
from unittest.mock import MagicMock, patch

from remix_agent_harbor.agent import (
    LOGS_DIR,
    OUTPUT_FILE,
    TRAJECTORY_FILE,
    RemixAgent,
    _strip_provider_prefix,
)


# ---------------------------------------------------------------------------
# _strip_provider_prefix
# ---------------------------------------------------------------------------

class TestStripProviderPrefix:
    def test_strips_anthropic_prefix(self) -> None:
        assert _strip_provider_prefix("anthropic/claude-sonnet-4-6") == "claude-sonnet-4-6"

    def test_strips_openai_prefix(self) -> None:
        assert _strip_provider_prefix("openai/gpt-4o") == "gpt-4o"

    def test_no_prefix_unchanged(self) -> None:
        assert _strip_provider_prefix("claude-sonnet-4-6") == "claude-sonnet-4-6"

    def test_multiple_slashes_strips_first_only(self) -> None:
        assert _strip_provider_prefix("a/b/c") == "b/c"

    def test_empty_string(self) -> None:
        assert _strip_provider_prefix("") == ""

    def test_slash_only(self) -> None:
        assert _strip_provider_prefix("/") == ""

    def test_trailing_slash(self) -> None:
        assert _strip_provider_prefix("anthropic/") == ""


# ---------------------------------------------------------------------------
# RemixAgent.name
# ---------------------------------------------------------------------------

class TestRemixAgentName:
    def test_name_returns_remix_agent(self) -> None:
        assert RemixAgent.name() == "remix-agent"


# ---------------------------------------------------------------------------
# RemixAgent._install_agent_template_path
# ---------------------------------------------------------------------------

class TestInstallTemplatePath:
    def test_template_path_exists(self) -> None:
        agent = _make_agent()
        path = agent._install_agent_template_path
        assert path.exists()
        assert path.name == "install-remix-agent.sh.j2"
        assert "templates" in str(path)


# ---------------------------------------------------------------------------
# RemixAgent.create_run_agent_commands
# ---------------------------------------------------------------------------

class TestCreateRunAgentCommands:
    def test_basic_command_structure(self) -> None:
        agent = _make_agent(model_name="anthropic/claude-sonnet-4-6")
        with patch.dict(os.environ, {"ANTHROPIC_API_KEY": "sk-test-123"}, clear=False):
            commands = agent.create_run_agent_commands("Navigate to google.com")

        assert len(commands) == 1
        cmd = commands[0].command
        assert isinstance(cmd, str)
        assert cmd.startswith("remix-agent run")
        assert "--no-browser" in cmd
        assert "--permission-mode bypass-permissions" in cmd
        assert "--max-iterations 200" in cmd
        assert "--output" in cmd
        assert "--verbose" in cmd
        # Instruction should be JSON-quoted at the end
        assert '"Navigate to google.com"' in cmd

    def test_api_key_forwarded(self) -> None:
        agent = _make_agent()
        with patch.dict(os.environ, {"ANTHROPIC_API_KEY": "sk-secret"}, clear=False):
            commands = agent.create_run_agent_commands("test task")

        env = commands[0].env
        assert env is not None
        assert env["REMIX_LLM_API_KEY"] == "sk-secret"

    def test_model_name_stripped(self) -> None:
        agent = _make_agent(model_name="anthropic/claude-sonnet-4-6")
        with patch.dict(os.environ, {"ANTHROPIC_API_KEY": "sk-test"}, clear=False):
            commands = agent.create_run_agent_commands("task")

        env = commands[0].env
        assert env is not None
        assert env["REMIX_LLM_MODEL"] == "claude-sonnet-4-6"

    def test_model_name_without_prefix(self) -> None:
        agent = _make_agent(model_name="claude-sonnet-4-6")
        with patch.dict(os.environ, {"ANTHROPIC_API_KEY": "sk-test"}, clear=False):
            commands = agent.create_run_agent_commands("task")

        env = commands[0].env
        assert env is not None
        assert env["REMIX_LLM_MODEL"] == "claude-sonnet-4-6"

    def test_missing_api_key_warns(self) -> None:
        agent = _make_agent()
        env_without_key = {k: v for k, v in os.environ.items() if k != "ANTHROPIC_API_KEY"}
        with patch.dict(os.environ, env_without_key, clear=True):
            commands = agent.create_run_agent_commands("task")

        # Should still produce commands, just with empty key
        assert len(commands) == 1
        env = commands[0].env
        assert env is not None
        assert env["REMIX_LLM_API_KEY"] == ""

    def test_timeout_is_set(self) -> None:
        agent = _make_agent()
        with patch.dict(os.environ, {"ANTHROPIC_API_KEY": "sk-test"}, clear=False):
            commands = agent.create_run_agent_commands("task")

        assert commands[0].timeout_sec == 3600

    def test_output_path_points_to_logs_dir(self) -> None:
        agent = _make_agent()
        with patch.dict(os.environ, {"ANTHROPIC_API_KEY": "sk-test"}, clear=False):
            commands = agent.create_run_agent_commands("task")

        cmd = commands[0].command
        assert str(OUTPUT_FILE) in cmd


# ---------------------------------------------------------------------------
# RemixAgent.populate_context_post_run
# ---------------------------------------------------------------------------

class TestPopulateContextPostRun:
    def test_parses_full_output(self, tmp_path: Path) -> None:
        output_data = {
            "status": "success",
            "result": "done",
            "steps": [
                {
                    "iteration": 1,
                    "tool": "bash",
                    "input": {"command": "ls"},
                    "output": {"result": "file.txt"},
                    "duration_ms": 100,
                }
            ],
            "total_iterations": 1,
            "total_duration_ms": 100,
            "total_input_tokens": 5000,
            "total_output_tokens": 1200,
            "total_cost_usd": 0.0342,
        }
        output_file = tmp_path / "output.json"
        output_file.write_text(json.dumps(output_data), encoding="utf-8")
        trajectory_file = tmp_path / "trajectory.jsonl"

        agent = _make_agent()
        context = MagicMock(spec=["n_input_tokens", "n_output_tokens", "cost_usd"])

        with (
            patch("remix_agent_harbor.agent.OUTPUT_FILE", output_file),
            patch("remix_agent_harbor.agent.TRAJECTORY_FILE", trajectory_file),
        ):
            agent.populate_context_post_run(context)

        assert context.n_input_tokens == 5000
        assert context.n_output_tokens == 1200
        assert abs(context.cost_usd - 0.0342) < 1e-9

    def test_writes_trajectory_jsonl(self, tmp_path: Path) -> None:
        output_data = {
            "status": "success",
            "steps": [
                {"iteration": 1, "tool": "a", "input": {}, "output": {}, "duration_ms": 10},
                {"iteration": 2, "tool": "b", "input": {}, "output": {}, "duration_ms": 20},
            ],
            "total_iterations": 2,
            "total_duration_ms": 30,
        }
        output_file = tmp_path / "output.json"
        output_file.write_text(json.dumps(output_data), encoding="utf-8")
        trajectory_file = tmp_path / "trajectory.jsonl"

        agent = _make_agent()
        context = MagicMock(spec=["n_input_tokens", "n_output_tokens", "cost_usd"])

        with (
            patch("remix_agent_harbor.agent.OUTPUT_FILE", output_file),
            patch("remix_agent_harbor.agent.TRAJECTORY_FILE", trajectory_file),
        ):
            agent.populate_context_post_run(context)

        lines = trajectory_file.read_text(encoding="utf-8").strip().splitlines()
        assert len(lines) == 2
        assert json.loads(lines[0])["tool"] == "a"
        assert json.loads(lines[1])["tool"] == "b"

    def test_missing_output_file(self, tmp_path: Path) -> None:
        missing_file = tmp_path / "does_not_exist.json"

        agent = _make_agent()
        context = MagicMock(spec=["n_input_tokens", "n_output_tokens", "cost_usd"])

        with patch("remix_agent_harbor.agent.OUTPUT_FILE", missing_file):
            # Should not raise
            agent.populate_context_post_run(context)

    def test_malformed_json(self, tmp_path: Path) -> None:
        output_file = tmp_path / "output.json"
        output_file.write_text("{bad json", encoding="utf-8")

        agent = _make_agent()
        context = MagicMock(spec=["n_input_tokens", "n_output_tokens", "cost_usd"])

        with patch("remix_agent_harbor.agent.OUTPUT_FILE", output_file):
            # Should not raise
            agent.populate_context_post_run(context)

    def test_missing_token_fields(self, tmp_path: Path) -> None:
        output_data = {
            "status": "error",
            "steps": [],
            "total_iterations": 0,
            "total_duration_ms": 50,
            "error": "fail",
        }
        output_file = tmp_path / "output.json"
        output_file.write_text(json.dumps(output_data), encoding="utf-8")

        agent = _make_agent()
        context = MagicMock(spec=["n_input_tokens", "n_output_tokens", "cost_usd"])
        # Set defaults that should NOT be overwritten
        context.n_input_tokens = 0
        context.n_output_tokens = 0
        context.cost_usd = 0.0

        with patch("remix_agent_harbor.agent.OUTPUT_FILE", output_file):
            agent.populate_context_post_run(context)

        # Fields were not in output, so defaults should remain
        assert context.n_input_tokens == 0
        assert context.n_output_tokens == 0
        assert context.cost_usd == 0.0

    def test_no_steps_skips_trajectory(self, tmp_path: Path) -> None:
        output_data = {
            "status": "success",
            "total_iterations": 0,
            "total_duration_ms": 10,
        }
        output_file = tmp_path / "output.json"
        output_file.write_text(json.dumps(output_data), encoding="utf-8")
        trajectory_file = tmp_path / "trajectory.jsonl"

        agent = _make_agent()
        context = MagicMock(spec=["n_input_tokens", "n_output_tokens", "cost_usd"])

        with (
            patch("remix_agent_harbor.agent.OUTPUT_FILE", output_file),
            patch("remix_agent_harbor.agent.TRAJECTORY_FILE", trajectory_file),
        ):
            agent.populate_context_post_run(context)

        # Trajectory file should not be created when steps key is missing
        assert not trajectory_file.exists()


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _make_agent(model_name: str = "anthropic/claude-sonnet-4-6") -> RemixAgent:
    """Create a RemixAgent with minimal mocking of BaseInstalledAgent init."""
    agent = RemixAgent.__new__(RemixAgent)
    agent.model_name = model_name  # type: ignore[attr-defined]
    return agent
