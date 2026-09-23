"""Harbor agent adapter: run the TeamAgents harness inside a task container.

The harness is shipped as a single statically linked ``teamagents`` binary
(``x86_64-unknown-linux-musl``) that the adapter uploads into the task
container and drives through the documented headless entry point::

    printf %s <base64 prompt> | base64 -d | teamagents exec --json \
        --full-auto --cwd <workdir> -

``exec --json`` writes one JSON object per line to stdout (session, tool calls,
events, final result) and exits 0 only when the team reports a completed goal.
The leader, the members it spawns, the shell/file tools and every approval stay
inside the container, so the benchmark measures the same harness the product
ships.

Full-auto Shell runs directly inside the task container. It does not require
bubblewrap or extra Docker capabilities. The optional bubblewrap installer and
compose overlay remain available when reproducing the historical binary.

Usage::

    PYTHONPATH=review/eval/terminal-bench \
    harbor run -d terminal-bench/terminal-bench-2-1@latest \
        -m deepseek/deepseek-flash \
        -a teamagents_agent:TeamAgentsAgent \
        -i build-cython-ext
"""

from __future__ import annotations

import base64
import json
import os
import shlex
import subprocess
from pathlib import Path
from typing import Any, override

from pydantic import Field

from harbor.agents.capabilities import AgentCapabilities
from harbor.agents.installed.base import BaseInstalledAgent, with_prompt_template
from harbor.agents.model_connection import (
    PROVIDERS,
    ModelConnectionSpec,
)
from harbor.agents.options import InstalledAgentOptions
from harbor.environments.base import BaseEnvironment
from harbor.models.agent.context import AgentContext, ModelUsage
from harbor.models.trial.paths import EnvironmentPaths

REMOTE_BINARY = "/usr/local/bin/teamagents"
REMOTE_CONFIG = ".config/teamagents/config.toml"
REMOTE_TEAM_SPEC = "/opt/teamagents/team.json"

LOG_NAME = "teamagents.jsonl"
STDERR_NAME = "teamagents.stderr.log"
EXIT_CODE_NAME = "teamagents.exit_code"
SUMMARY_NAME = "teamagents.summary.json"

# teamagents protocol slugs (core/src/models.rs): "openai" | "chat/completions"
# | "responses" | "anthropic" | "deepseek"
PROTOCOLS = {
    "deepseek": "deepseek",
    "anthropic": "anthropic",
    "openai": "responses",
    "openrouter": "chat/completions",
}

_REPO_ROOT = Path(__file__).resolve().parents[3]
_DEFAULT_BINARY = _REPO_ROOT / "engine/target/x86_64-unknown-linux-musl/release/teamagents"


class TeamAgentsOptions(InstalledAgentOptions):
    """Adapter kwargs (``harbor run --ak key=value``)."""

    binary_path: str | None = Field(
        default=None,
        description=(
            "Local static teamagents binary to upload. Defaults to "
            "$TEAMAGENTS_AGENT_BIN or engine/target/<musl>/release/teamagents."
        ),
    )
    workdir: str = Field(default="/app", description="Task working directory.")
    reasoning_effort: str | None = Field(
        default="high",
        description=(
            "DeepSeek reasoning effort (low/high/max). Omit to use the "
            "harness default from the generated profile."
        ),
    )
    context_window: int | None = Field(
        default=1_000_000,
        description=(
            "Native context window written into the generated profile "
            "(DeepSeek Flash 1M, docs/DECISIONS.md D-36)."
        ),
    )
    inner_timeout_sec: int | None = Field(
        default=None,
        description=(
            "Value for `teamagents exec --timeout`. Omit to keep the harness "
            "default (1200s) and let Harbor's agent timeout stop the trial."
        ),
    )
    install_bubblewrap: bool = Field(
        default=False,
        description="Install the bubblewrap package when the image lacks bwrap.",
    )
    team_spec: str | None = Field(
        default=None,
        description="Host path of a TeamSpec (JSON/YAML) uploaded and passed to --team.",
    )


class TeamAgentsAgent(BaseInstalledAgent):
    """TeamAgents (multi-agent leader + members) running inside the container."""

    capabilities = AgentCapabilities()
    MODEL_CONNECTION = ModelConnectionSpec(passthrough=True)
    options_model = TeamAgentsOptions

    @staticmethod
    @override
    def name() -> str:
        return "teamagents"

    @override
    def version(self) -> str | None:
        binary = self._binary_path()
        if binary is None or not binary.is_file():
            return None
        try:
            out = subprocess.run(
                [str(binary), "--version"],
                capture_output=True,
                text=True,
                timeout=30,
                check=False,
            )
        except (OSError, subprocess.SubprocessError):
            return None
        text = (out.stdout or out.stderr).strip()
        try:
            return json.loads(text).get("version")
        except json.JSONDecodeError:
            return text.split()[-1] if text else None

    # -- setup ---------------------------------------------------------------

    @override
    async def install(self, environment: BaseEnvironment) -> None:
        binary = self._binary_path()
        if binary is None or not binary.is_file():
            raise FileNotFoundError(
                "teamagents binary not found: "
                f"{binary or self.options.binary_path}. Build it with "
                "`make check`-style toolchain setup, then: CC_x86_64_unknown_linux_musl=musl-gcc "
                "cargo build --offline --release --manifest-path engine/Cargo.toml "
                "--bin teamagents --target x86_64-unknown-linux-musl"
            )
        await environment.upload_file(binary, REMOTE_BINARY)
        await self.exec_as_root(
            environment,
            command=f"chmod 0755 {REMOTE_BINARY} && {REMOTE_BINARY} --version",
        )
        if self.options.install_bubblewrap:
            await self._ensure_package(environment, "bwrap", "bubblewrap")
        await self._ensure_package(environment, "bash", "bash")
        await self._upload_config(environment)
        if self.options.team_spec:
            spec = Path(self.options.team_spec).expanduser()
            if not spec.is_file():
                raise FileNotFoundError(f"team spec not found: {spec}")
            await environment.upload_file(spec, REMOTE_TEAM_SPEC)

    async def _ensure_package(
        self,
        environment: BaseEnvironment,
        command: str,
        package: str,
    ) -> None:
        probe = await environment.exec(
            command=f"command -v {shlex.quote(command)} >/dev/null 2>&1",
            user="root",
        )
        if probe.return_code == 0:
            return
        # Debian images pinned to an old release can carry a security-pool entry
        # whose .deb was dropped upstream (bullseye's bubblewrap 404s); pinning
        # the release codename falls back to the still-published main version.
        install = (
            "install_package() { "
            "if command -v apt-get >/dev/null 2>&1; then "
            "apt-get update >/dev/null 2>&1 || true; "
            f"DEBIAN_FRONTEND=noninteractive apt-get install -y {package} && return 0; "
            f"DEBIAN_FRONTEND=noninteractive apt-get install -y --fix-missing {package} && return 0; "
            "codename=$(. /etc/os-release 2>/dev/null; echo \"${VERSION_CODENAME:-}\"); "
            f"if [ -n \"$codename\" ]; then DEBIAN_FRONTEND=noninteractive apt-get install -y -t \"$codename\" {package}; "
            "else return 1; fi; "
            "elif command -v dnf >/dev/null 2>&1; then "
            f"dnf install -y {package}; "
            "elif command -v yum >/dev/null 2>&1; then "
            f"yum install -y {package}; "
            "elif command -v apk >/dev/null 2>&1; then "
            f"apk add --no-cache {package} || apk add --no-cache --update {package}; "
            "else echo 'no supported package manager' >&2; return 1; fi; "
            "}; install_package && "
            f"command -v {shlex.quote(command)} >/dev/null 2>&1"
        )
        await self.exec_as_root(environment, command=install, timeout_sec=900)

    async def _upload_config(self, environment: BaseEnvironment) -> None:
        content = self._config_text()
        encoded = base64.b64encode(content.encode("utf-8")).decode("ascii")
        await self.exec_as_root(
            environment,
            command=(
                'mkdir -p "$HOME/'
                + REMOTE_CONFIG.rsplit("/", 1)[0]
                + '" && printf %s '
                + shlex.quote(encoded)
                + ' | base64 -d > "$HOME/'
                + REMOTE_CONFIG
                + '"'
            ),
        )

    # -- run -----------------------------------------------------------------

    @override
    @with_prompt_template
    async def run(
        self,
        instruction: str,
        environment: BaseEnvironment,
        context: AgentContext,
    ) -> None:
        options = self.options
        workdir = options.workdir
        agent_dir = EnvironmentPaths.agent_dir.as_posix()
        log_path = f"{agent_dir}/{LOG_NAME}"
        stderr_path = f"{agent_dir}/{STDERR_NAME}"
        exit_path = f"{agent_dir}/{EXIT_CODE_NAME}"

        env = dict(self.model_connection.env)
        prompt = base64.b64encode(instruction.encode("utf-8")).decode("ascii")
        flags = ["--json", "--full-auto", f"--cwd {shlex.quote(workdir)}"]
        if options.inner_timeout_sec:
            flags.append(f"--timeout {int(options.inner_timeout_sec)}")
        if options.team_spec:
            flags.append(f"--team {shlex.quote(REMOTE_TEAM_SPEC)}")
        # `tee` keeps the stream in Harbor's own logs and in the trial's
        # /logs/agent mount, so an interrupted exec still leaves a transcript;
        # bash is required for PIPESTATUS (docker exec defaults to /bin/sh).
        script = (
            f"mkdir -p {shlex.quote(agent_dir)} && cd {shlex.quote(workdir)} && "
            f"printf %s {shlex.quote(prompt)} | base64 -d | "
            f"{REMOTE_BINARY} exec {' '.join(flags)} - "
            f"2>{shlex.quote(stderr_path)} | tee {shlex.quote(log_path)}; "
            f"printf '%s' \"${{PIPESTATUS[2]:-$?}}\" > {shlex.quote(exit_path)}"
        )
        command = f"bash -c {shlex.quote(script)}"

        try:
            result = await environment.exec(
                command=command,
                cwd=workdir,
                env=env,
                timeout_sec=None,
            )
        except Exception:
            # Harbor cancelled the exec (trial timeout): the JSONL log is already
            # on the mounted agent dir, so keep whatever usage it recorded.
            self._collect_context(context)
            raise
        if result.stdout:
            self.logger.debug("teamagents stdout tail: %s", result.stdout[-2000:])
        if result.stderr:
            self.logger.debug("teamagents stderr tail: %s", result.stderr[-2000:])
        self._collect_context(context)

    @override
    def populate_context_post_run(self, context: AgentContext) -> None:
        # Backfill after a trial timeout: the JSONL log is written through the
        # /logs/agent mount, so it survives an interrupted exec.
        self._collect_context(context)

    # -- helpers -------------------------------------------------------------

    def _binary_path(self) -> Path | None:
        raw = (
            self.options.binary_path
            or os.environ.get("TEAMAGENTS_AGENT_BIN")
            or str(_DEFAULT_BINARY)
        )
        return Path(raw).expanduser() if raw else None

    def _provider(self) -> tuple[str, str, str]:
        """(provider, protocol, api key env name) for the configured model."""
        access = self.model_connection
        model_name = self.model_name or "deepseek/deepseek-flash"
        provider = access.provider or model_name.split("/", 1)[0]
        spec = PROVIDERS.get(provider)
        key_env = spec.api_key_envs[0] if spec and spec.api_key_envs else (
            f"{provider.upper().replace('-', '_')}_API_KEY"
        )
        return provider, PROTOCOLS.get(provider, "chat/completions"), key_env

    def _config_text(self) -> str:
        provider, protocol, key_env = self._provider()
        model = (self.model_name or "deepseek/deepseek-flash").split("/")[-1]
        lines = [
            "# generated by review/eval/terminal-bench/teamagents_agent.py",
            "[models.leader_main]",
            f'provider = "{provider}"',
            f'protocol = "{protocol}"',
            f'model = "{model}"',
            f'api_key_env = "{key_env}"',
            f'base_url = "{self.model_connection.configured_base_url}"'
            if self.model_connection.configured_base_url
            else None,
            "timeout = 120",
            "max_retries = 5",
        ]
        lines = [line for line in lines if line is not None]
        if self.options.context_window:
            lines.append(f"context_window = {int(self.options.context_window)}")
        if self.options.reasoning_effort:
            lines.append(
                "generation_options = { reasoning_effort = "
                f'"{self.options.reasoning_effort}" }}'
            )
        return "\n".join(lines) + "\n"

    def _collect_context(self, context: AgentContext) -> None:
        summary = self.parse_logs()
        prompt_tokens = summary.get("prompt_tokens")
        if prompt_tokens is not None:
            context.n_input_tokens = prompt_tokens
        completion_tokens = summary.get("completion_tokens")
        if completion_tokens is not None:
            context.n_output_tokens = completion_tokens
        cache_tokens = summary.get("cached_input_tokens")
        if cache_tokens:
            context.n_cache_tokens = cache_tokens
        usage = summary.get("model_usage") or {}
        if usage:
            context.model_usage = {
                name: ModelUsage(**values) for name, values in usage.items()
            }
        context.metadata = summary

    def parse_logs(self) -> dict[str, Any]:
        """Summarize the JSONL transcript written into the trial's log dir."""
        log_path = self.logs_dir / LOG_NAME
        summary: dict[str, Any] = {"log": str(log_path)}
        if not log_path.is_file():
            summary["status"] = "no_log"
            return summary
        counts: dict[str, int] = {}
        usage: dict[str, dict[str, int]] = {}
        last_result: dict[str, Any] | None = None
        with log_path.open("r", encoding="utf-8", errors="replace") as handle:
            for line in handle:
                line = line.strip()
                if not line:
                    continue
                try:
                    record = json.loads(line)
                except json.JSONDecodeError:
                    counts["unparsed"] = counts.get("unparsed", 0) + 1
                    continue
                kind = str(record.get("type", "unknown"))
                counts[kind] = counts.get(kind, 0) + 1
                if kind == "result":
                    last_result = record
                for entry in record.get("usage") or []:
                    model = str(entry.get("model") or entry.get("model_profile") or "unknown")
                    values = entry.get("usage") or {}
                    bucket = usage.setdefault(
                        model,
                        {"n_input_tokens": 0, "n_cache_tokens": 0, "n_output_tokens": 0},
                    )
                    bucket["n_input_tokens"] += int(values.get("prompt_tokens") or 0)
                    bucket["n_cache_tokens"] += int(values.get("cached_input_tokens") or 0)
                    bucket["n_output_tokens"] += int(values.get("completion_tokens") or 0)
        exit_code = None
        exit_path = self.logs_dir / EXIT_CODE_NAME
        if exit_path.is_file():
            raw = exit_path.read_text(encoding="utf-8", errors="replace").strip()
            exit_code = int(raw) if raw.lstrip("-").isdigit() else None
        summary["lines"] = counts
        if exit_code is not None:
            summary["exit_code"] = exit_code
        if last_result:
            summary["status"] = last_result.get("status")
            summary["duration_ms"] = last_result.get("duration_ms")
            summary["session_id"] = last_result.get("session_id")
            summary["runtime_errors"] = last_result.get("runtime_errors") or []
            summary["outcome_unknown"] = last_result.get("outcome_unknown") or []
            summary["verification"] = last_result.get("verification") or []
        summary["model_usage"] = usage
        summary["prompt_tokens"] = sum(v["n_input_tokens"] for v in usage.values()) or None
        summary["completion_tokens"] = (
            sum(v["n_output_tokens"] for v in usage.values()) or None
        )
        summary["cached_input_tokens"] = (
            sum(v["n_cache_tokens"] for v in usage.values()) or None
        )
        try:
            (self.logs_dir / SUMMARY_NAME).write_text(
                json.dumps(summary, ensure_ascii=False, indent=2),
                encoding="utf-8",
            )
        except OSError as error:  # a read-only log dir must not fail the trial
            self.logger.warning("cannot write %s: %s", SUMMARY_NAME, error)
        return summary
