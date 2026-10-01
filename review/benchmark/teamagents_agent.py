"""Harbor installed-agent adapter for TeamAgents.

Installs the static `teamagents` binary plus a frozen config into the task container and runs one headless
turn (`teamagents exec --json`) against the task instruction. The container *is* the sandbox, so the turn runs
with `--full-auto` (the product's `approved_scope` needs bubblewrap, which a task container cannot promise).

Usage (from the harbor venv, with the repo's binary built):

    PYTHONPATH=/tmp/ta-harbor harbor run -p /tmp/ta-harbor/hello \
        --agent teamagents_agent:TeamAgentsAgent \
        -m DeepSeek-V4.1-Flash \
        --allow-agent-host llmapi.paratera.com -y

Nothing here changes the product: it is the benchmark harness's half of the run.
"""

from __future__ import annotations

import os
import shlex
import tempfile
from pathlib import Path

from harbor.agents.installed.base import BaseInstalledAgent, with_prompt_template
from harbor.environments.base import BaseEnvironment
from harbor.models.agent.context import AgentContext

REPO = Path(os.environ.get("TEAMAGENTS_REPO", "/home/rimuru/Projects/Code/for_fun/TeamAgents"))
BINARY = Path(
    os.environ.get(
        "TEAMAGENTS_BINARY",
        str(REPO / "engine/target/x86_64-unknown-linux-musl/release/teamagents"),
    )
)
CONFIG_DIR = "/opt/teamagents/config"
STATE_DIR = "/opt/teamagents/state"
WORKDIR = os.environ.get("TEAMAGENTS_WORKDIR", "/app")
TURN_TIMEOUT_SEC = int(os.environ.get("TEAMAGENTS_TURN_TIMEOUT_SEC", "3000"))
# The DeepSeek card's Terminal-Bench numbers are at maximum reasoning effort; `high` is the repository's own
# evaluation setting. The effort is a knob here so one fixed sample can be run both ways.
REASONING_EFFORT = os.environ.get("TEAMAGENTS_REASONING_EFFORT", "high")

# The experiment's config, frozen like the repo's eval config: paratera DeepSeek-V4.1-Flash, native 1M window
# (D-36), no ceiling.
CONFIG_TEMPLATE = """\
skills_paths = []

[models.leader_main]
provider = "deepseek"
protocol = "deepseek"
model = "{model}"
base_url = "https://llmapi.paratera.com/v1"
api_key_env = "PARATERA_API_KEY"
context_window = 1000000
timeout = 300
max_retries = 3
generation_options = {{ reasoning_effort = "{effort}" }}
"""


class TeamAgentsAgent(BaseInstalledAgent):
    """One headless `teamagents exec` turn per trial, in the task's own workspace."""

    @staticmethod
    def name() -> str:
        return "teamagents"

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)

    def version(self) -> str | None:
        return "0.2.0"

    async def install(self, environment: BaseEnvironment) -> None:
        model = self.model_name or "DeepSeek-V4.1-Flash"
        await environment.exec(command=f"mkdir -p {CONFIG_DIR}/teamagents {STATE_DIR} {WORKDIR} /logs/agent")
        await environment.upload_file(BINARY, "/usr/local/bin/teamagents")
        await environment.exec(command="chmod +x /usr/local/bin/teamagents")
        with tempfile.NamedTemporaryFile("w", suffix=".toml", delete=False) as handle:
            handle.write(CONFIG_TEMPLATE.format(model=model, effort=REASONING_EFFORT))
            local_config = handle.name
        try:
            await environment.upload_file(local_config, f"{CONFIG_DIR}/teamagents/config.toml")
        finally:
            os.unlink(local_config)

    @with_prompt_template
    async def run(
        self,
        instruction: str,
        environment: BaseEnvironment,
        context: AgentContext,
    ) -> None:
        key = os.environ.get("PARATERA_API_KEY", "")
        # The key travels in the environment, never in the command string: harbor echoes a failed command into its
        # own error message, and a credential must not end up in a log. `_exec` redacts sensitive env values.
        env = {
            "HOME": "/root",
            "XDG_CONFIG_HOME": CONFIG_DIR,
            "XDG_STATE_HOME": STATE_DIR,
            "PARATERA_API_KEY": key,
        }
        command = " ".join(
            [
                "teamagents",
                f"--cwd {shlex.quote(WORKDIR)}",
                f"--state-root {shlex.quote(STATE_DIR + '/v2')}",
                "--full-auto",
                "exec",
                "--json",
                f"--timeout {TURN_TIMEOUT_SEC}",
                shlex.quote(instruction),
            ]
        )
        await self.exec_as_root(environment, command=command, env=env, timeout_sec=TURN_TIMEOUT_SEC + 300)
