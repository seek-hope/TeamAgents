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
# The model card's Terminal-Bench numbers come from the **official** DeepSeek endpoint, so that is the default;
# every part of the model route is overridable so another provider can be measured the same way. `high` is the
# repository's own evaluation setting, `low`/`medium`/`max` are the other tiers the endpoint accepts.
MODEL = os.environ.get("TEAMAGENTS_MODEL", "deepseek-flash")
BASE_URL = os.environ.get("TEAMAGENTS_BASE_URL", "https://api.deepseek.com")
API_KEY_ENV = os.environ.get("TEAMAGENTS_API_KEY_ENV", "DEEPSEEK_API_KEY")
REASONING_EFFORT = os.environ.get("TEAMAGENTS_REASONING_EFFORT", "high")

# The experiment's config, frozen like the repo's eval config: native 1M window (D-36), no ceiling.
CONFIG_TEMPLATE = """\
skills_paths = []

[models.leader_main]
provider = "deepseek"
protocol = "deepseek"
model = "{model}"
base_url = "{base_url}"
api_key_env = "{key_env}"
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
        # Every built-in harbor agent installs its system dependencies here, and `ca_certificates` is the one that
        # is `always_install`: an image without a CA bundle cannot verify TLS, so a later `apt-get`/`curl` in the
        # *verifier* fails and the trial is scored 0 for the wrong reason. Omitting this call understates the
        # score (measured: 5 of 59 trials in one sample had a verifier that never ran; with the call, one of them
        # passed). Best-effort: a task image whose own apt sources have rotted (`debian:bullseye` 404s) must not
        # turn into an agent-install error.
        try:
            await self.ensure_system_dependencies(environment, ("curl", "ca_certificates"))
        except Exception as exc:  # noqa: BLE001 - the dependency is optional, the trial is not
            self.logger.warning("could not install curl/ca-certificates: %s", exc)
        model = self.model_name or MODEL
        await environment.exec(command=f"mkdir -p {CONFIG_DIR}/teamagents {STATE_DIR} {WORKDIR} /logs/agent")
        await environment.upload_file(BINARY, "/usr/local/bin/teamagents")
        await environment.exec(command="chmod +x /usr/local/bin/teamagents")
        with tempfile.NamedTemporaryFile("w", suffix=".toml", delete=False) as handle:
            handle.write(
                CONFIG_TEMPLATE.format(
                    model=model, base_url=BASE_URL, key_env=API_KEY_ENV, effort=REASONING_EFFORT
                )
            )
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
        key = os.environ.get(API_KEY_ENV, "")
        # The key travels in the environment, never in the command string: harbor echoes a failed command into its
        # own error message, and a credential must not end up in a log. `_exec` redacts sensitive env values.
        env = {
            "HOME": "/root",
            "XDG_CONFIG_HOME": CONFIG_DIR,
            "XDG_STATE_HOME": STATE_DIR,
            API_KEY_ENV: key,
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
