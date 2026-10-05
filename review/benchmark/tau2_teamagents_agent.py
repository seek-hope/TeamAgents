#!/usr/bin/env python3
"""Run TeamAgents as the agent inside τ³-bench (D-420): a conversation with a simulated user, graded from the
database state the tools produced.

This is the third benchmark axis, and it is deliberately not another code-editing run. τ³-bench (`tau2`) gives the
agent a **domain policy**, a set of **domain tools** and a **simulated user**; the reward is read from the database
afterwards. TeamAgents is driven through its own surface — one `teamagents exec` per user turn in a persistent
session, with the domain tools exposed as an **MCP stdio server** (`tau2_mcp_server.py`) — so what is measured is
the product (its MCP path, its loop, its model) rather than another in-process LLM loop.

The wiring, and why it is this way:

* the **environment handle matters**: `build_orchestrator` exposes `orchestrator.environment` and
  `orchestrator.agent`, and the agent is handed that environment *after* construction
  (`attach_environment`). The bridge answers tool calls by calling `environment.get_response`, so the tools mutate
  the same database the simulated user and the grader see.
* the **policy goes in the prompt**: τ³-bench's `domain_policy` text is written as the product's
  `instruction_files`, which every member's system text receives (D-102/D-246) — the agent is told the rules in
  the product's own channel, not through a harness-specific prompt.
* each user turn is a **new goal** in the same session (`goals open` before `exec`), because a settled goal cannot
  be reopened; the conversation context is the session's, so the model sees the whole dialogue.
* nothing here judges the result: the reward is τ³-bench's own evaluation of the database.

Usage:
    python3 review/benchmark/tau2_teamagents_agent.py --domain retail --tasks 5 --out /tmp/tau3 [--timeout 600]
"""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import socket
import subprocess
import sys
import tempfile
import threading
import time

REPO = pathlib.Path(__file__).resolve().parents[2]
BINARY = os.environ.get("TEAMAGENTS_BINARY", REPO / "engine/target/x86_64-unknown-linux-musl/release/teamagents")
SERVER = REPO / "review/benchmark/tau2_mcp_server.py"


class Bridge:
    """The environment side of the MCP bridge: a Unix socket that answers `list` and `call`."""

    def __init__(self, environment, path: str):
        self.environment = environment
        self.path = path
        self.calls: list[dict] = []
        self.server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.server.bind(path)
        self.server.listen(8)
        self.thread = threading.Thread(target=self._serve, daemon=True)
        self.thread.start()

    def _serve(self) -> None:
        while True:
            try:
                connection, _ = self.server.accept()
            except OSError:
                return
            with connection:
                buffer = b""
                while not buffer.endswith(b"\n"):
                    chunk = connection.recv(65536)
                    if not chunk:
                        break
                    buffer += chunk
                try:
                    answer = self._answer(json.loads(buffer.decode()))
                except Exception as error:
                    answer = {"content": f"bridge error: {error}", "isError": True}
                connection.sendall((json.dumps(answer) + "\n").encode())

    def _answer(self, request: dict) -> dict:
        if request.get("op") == "list":
            tools = []
            for tool in self.environment.get_tools():
                schema = tool.openai_schema["function"]
                tools.append({"name": schema["name"], "description": schema.get("description", ""),
                              "inputSchema": schema.get("parameters", {"type": "object", "properties": {}})})
            return {"tools": tools}
        if request.get("op") == "call":
            from tau2.data_model.message import ToolCall

            call = ToolCall(
                id=f"call-{len(self.calls)}",
                name=request["name"],
                arguments=request.get("arguments") or {},
                requestor="assistant",
            )
            response = self.environment.get_response(call)
            self.calls.append({"name": call.name, "arguments": call.arguments})
            return {"content": response.content, "isError": bool(response.error)}
        return {"content": f"unknown op {request.get('op')!r}", "isError": True}


class TeamAgentsAgent:
    """The τ³-bench agent: it owns a TeamAgents session and turns each user message into one `exec`."""

    def __init__(self, tools, domain_policy: str, timeout: int = 600, out: pathlib.Path | None = None, **kwargs):
        self.tools = tools
        self.domain_policy = domain_policy
        self.timeout = timeout
        self.out = out
        self.environment = None
        self.bridge: Bridge | None = None
        self.work: pathlib.Path | None = None
        self.session = None
        self.turns = 0

    # -- τ³-bench interface ---------------------------------------------------------------------------------
    def get_init_state(self, message_history=None):
        return {"messages": list(message_history or [])}

    def attach_environment(self, environment, out: pathlib.Path) -> None:
        self.environment = environment
        self.out = out
        self.work = pathlib.Path(tempfile.mkdtemp(prefix="ta-tau3-"))
        self.bridge = Bridge(environment, str(self.work / "bridge.sock"))
        (self.work / "policy.md").write_text(self.domain_policy)
        (self.work / "config").mkdir(exist_ok=True)
        (self.work / "config" / "teamagents").mkdir(exist_ok=True)
        (self.work / "config" / "teamagents" / "config.toml").write_text(
            # `instruction_files` is a *top-level* key (it is appended to every member's system text); writing it
            # inside the model table is a load error, which is how the first smoke run failed.
            "instruction_files = [\"" + str(self.work / "policy.md") + "\"]\n\n"
            "[models.leader_main]\nprovider = \"deepseek\"\nmodel = \"deepseek-flash\"\n"
            "api_key_env = \"DEEPSEEK_API_KEY\"\n\n"
            "[tools.tau3]\nkind = \"mcp\"\nmcp_transport = \"stdio\"\n"
            "command = \"" + sys.executable + "\"\n"
            "args = [\"" + str(SERVER) + "\", \"--socket\", \"" + str(self.work / "bridge.sock") + "\"]\n"
            "mcp_execution = \"host\"\nrequired = true\nstartup_timeout_s = 60\ntool_timeout_s = 120\n"
        )

    def stop(self, message=None, state=None) -> None:
        return None

    @classmethod
    def is_stop(cls, message) -> bool:
        """TeamAgents never ends the conversation: a customer-service agent answers until the *user* is done, and
        in τ³-bench the user simulator is the side that says so. Returning True here would cut every dialogue
        short."""
        return False

    def set_seed(self, seed) -> None:
        """The τ³-bench orchestrator sets a seed on both participants; this agent's own loop has none to set
        (the product is driven as a process, and its sessions are already per-turn), but the protocol expects the
        method."""
        self.seed = seed

    def generate_next_message(self, message, state):
        from tau2.data_model.message import AssistantMessage

        text = getattr(message, "content", "") or ""
        reply = self._turn(text)
        self.turns += 1
        return AssistantMessage(role="assistant", content=reply), state

    # -- the product ----------------------------------------------------------------------------------------
    def _env(self) -> dict:
        env = dict(os.environ)
        env["XDG_CONFIG_HOME"] = str(self.work / "config")
        env["XDG_STATE_HOME"] = str(self.work / "state")
        return env

    def _run(self, args: list[str], stdin: str | None = None) -> subprocess.CompletedProcess:
        return subprocess.run(
            [str(BINARY), *args], env=self._env(), input=stdin, capture_output=True, text=True,
            timeout=self.timeout + 120,
        )

    def _turn(self, user_message: str) -> str:
        state_root = str(self.work / "state" / "teamagents" / "v2")
        # every turn is its own goal: a settled goal cannot be reopened, and the session keeps the conversation
        self._run(["init"])
        self._run(["goals", "open", "--id", f"turn-{self.turns}", "--state-root", state_root])
        report = self._run(
            ["exec", "--json", "--full-auto", "--timeout", str(self.timeout), "--state-root", state_root, "--", user_message]
        )
        if self.out is not None:
            (self.out / f"turn-{self.turns}.json").write_text(report.stdout + report.stderr)
        try:
            payload = json.loads(report.stdout.split("\n{")[0] if report.stdout.startswith("{") else report.stdout)
        except Exception:
            return f"(the product did not answer: rc={report.returncode}) {report.stdout[-200:]}"
        return payload.get("reply") or "(no reply)"


def create_agent(tools, domain_policy, **kwargs):
    return TeamAgentsAgent(tools, domain_policy, **kwargs)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--domain", default="retail")
    parser.add_argument("--tasks", type=int, default=5, help="how many tasks to run")
    parser.add_argument("--timeout", type=int, default=600, help="per-turn bound in seconds")
    parser.add_argument("--out", required=True)
    parser.add_argument("--task-ids", nargs="*", default=[])
    args = parser.parse_args()

    from tau2.data_model.simulation import TextRunConfig
    from tau2.evaluator.evaluator import EvaluationType
    from tau2.registry import registry
    from tau2.runner import build_orchestrator, get_tasks, run_simulation

    out = pathlib.Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    registry.register_agent_factory(create_agent, "teamagents")
    tasks = get_tasks(args.domain, task_ids=args.task_ids or None)
    tasks = tasks[: args.tasks]
    rewards = []
    for index, task in enumerate(tasks):
        run_dir = out / f"{task.id}"
        run_dir.mkdir(exist_ok=True)
        # the agent field selects *our* factory (registered above); the user simulator is the only LLM tau2
        # drives here, so it is the one that needs a model — the same DeepSeek endpoint the product uses.
        config = TextRunConfig(
            domain=args.domain,
            agent="teamagents",
            llm_agent="deepseek/deepseek-chat",
            llm_user=os.environ.get("TAU2_USER_MODEL", "deepseek/deepseek-chat"),
            timeout=args.timeout,
            num_trials=1,
            seed=20261005,
            max_steps=12,
        )
        started = time.time()
        try:
            orchestrator = build_orchestrator(config, task)
            orchestrator.agent.attach_environment(orchestrator.environment, run_dir)
            # The **DB check** is τ³-bench's own reward — the database state the tools produced. The extra
            # `NLAssertionsEvaluator` is experimental and calls a hard-coded OpenAI model (`gpt-4.1`), which is
            # both non-deterministic and unreachable here; leaving it out is the standard metric, not a shortcut.
            simulation = run_simulation(orchestrator, evaluation_type=EvaluationType.ENV)
            reward = simulation.reward_info.reward if simulation.reward_info else None
        except Exception as error:  # a task that cannot run is reported, never silently skipped
            import traceback

            reward = None
            (run_dir / "error.txt").write_text(f"{type(error).__name__}: {error}\n\n{traceback.format_exc()}")
        rewards.append(reward)
        # The trajectory is evidence, not decoration (D-419's lesson): the tool calls the bridge really served, the
        # conversation, and the reward. A run with zero tool calls did not test the product's tool path at all.
        reward_detail = None
        if "simulation" in locals() and simulation is not None and simulation.reward_info is not None:
            reward_detail = simulation.reward_info.model_dump()
            (run_dir / "reward_detail.json").write_text(json.dumps(reward_detail, indent=2, default=str))
        calls = getattr(getattr(orchestrator, "agent", None), "bridge", None)
        call_log = calls.calls if calls is not None else []
        conversation = []
        if "simulation" in locals() and simulation is not None:
            for message in getattr(simulation, "messages", []) or []:
                conversation.append({"role": getattr(message, "role", ""), "content": getattr(message, "content", None),
                                     "tool_calls": [getattr(call, "name", str(call)) for call in
                                                    (getattr(message, "tool_calls", None) or [])]})
        (run_dir / "calls.json").write_text(json.dumps(call_log, indent=2))
        (run_dir / "conversation.json").write_text(json.dumps(conversation, indent=2))
        print(f"{index + 1}/{len(tasks)} {task.id}: reward={reward} tool_calls={len(call_log)} "
              f"messages={len(conversation)} ({time.time() - started:.0f}s)", flush=True)
        (run_dir / "reward.json").write_text(json.dumps({"task": task.id, "reward": reward,
                                                         "tool_calls": len(call_log)}))
    passed = sum(1 for r in rewards if r == 1.0)
    print(f"\n{args.domain}: {passed}/{len(rewards)} solved (rewards {rewards})")
    (out / "summary.json").write_text(json.dumps({"domain": args.domain, "passed": passed,
                                                  "tasks": len(rewards), "rewards": rewards}, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
