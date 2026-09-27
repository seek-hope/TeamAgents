#!/usr/bin/env python3
"""The repository-local config: read by the product, and gated by the user's own opt-in (D-244).

`<cwd>/.teamagents/config.toml` is what a session started in a cloned repository meets. Since D-244 the daemon
reads it under one gate — `[permissions] trust_project = true` in the *user's* config — and the gate is what
keeps a repository from becoming the session's model (a profile carries `base_url`/`api_key_env`), from
installing a tool binding, or from reaching a member's prompt. This probe measures that over the real wire
stack, credential-free:

* a local chat-completions server answers one `finish` call, so a turn settles without a model;
* the project file declares `[tools.repo_tool] kind = "web_search"`, and the probe reads the **surface witness**
  `TEAMAGENTS_LOG_SURFACE=1` writes into the daemon log — the offered tool list, per instance;
* three runs: untrusted (the tool must be absent, the refusal named), trusted (the tool must be offered), and a
  *self-granting* project that sets `[permissions] trust_project = true` in its own file (the user's answer, not
  the repository's, is what counts, so the tool stays absent).

The offered-surface line is the same witness `authority.py` uses (D-143/D-157): what the model was offered, not
what it chose.

    python3 review/dogfood/project_config.py
    python3 review/dogfood/project_config.py --state-dir /tmp/ta-project-config
"""
import argparse
import atexit
import json
import pathlib
import shutil
import subprocess
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

REPO = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "review"))   # shared pid-based stop (D-148)
import leak_guard  # noqa: E402
BIN = REPO / "engine/target/debug/teamagents"
KEY_VAR = "TEAMAGENTS_PROJECT_PROBE_KEY"
PROJECT_TOOL = "web_search"

HEAD = b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n"
FINISH = json.dumps({"status": "success", "summary": "settled without a model"})
COMPLETE = HEAD + (
    'data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"finish-1","function":'
    '{"name":"finish","arguments":' + json.dumps(FINISH) + '}}]}}]}\n\n'
    'data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":5,'
    '"completion_tokens":2,"total_tokens":7}}\n\n'
    'data: [DONE]\n\n'
).encode()


class FakeChat(BaseHTTPRequestHandler):
    """Every POST gets the same finish call; the probe counts what it saw."""

    protocol_version = "HTTP/1.0"
    seen: list = []

    def do_POST(self):  # noqa: N802 (http.server's name)
        length = int(self.headers.get("content-length") or 0)
        body = self.rfile.read(length) if length else b""
        self.seen.append(body)
        self.wfile.write(COMPLETE)
        self.wfile.flush()
        self.close_connection = True

    def log_message(self, *args):  # keep the probe's output clean
        pass


def user_config(port: int, trust: bool) -> str:
    return f"""# Project-config probe (D-244): a local chat-completions server, no credentials, no network.
skills_paths = []

[permissions]
trust_project = {str(trust).lower()}

[models.leader_main]
provider = "openai"
protocol = "openai"
model = "project-probe"
base_url = "http://127.0.0.1:{port}/v1"
api_key_env = "{KEY_VAR}"
context_window = 128000
timeout = 30
"""


def project_config(self_grant: bool) -> str:
    permissions = "[permissions]\ntrust_project = true\n\n" if self_grant else ""
    return f"""# the repository's own config: a model profile and a tool binding
{permissions}[models.repo_model]
provider = "openai"
protocol = "openai"
model = "repo"
base_url = "https://repo.invalid/v1"
api_key_env = "{KEY_VAR}"

[tools.repo_tool]
kind = "{PROJECT_TOOL}"
"""


def offered_tools(state_root: pathlib.Path) -> str:
    """The leader's `driver: surface …` line from the daemon log, or ''."""
    log = state_root / "daemon.log"
    if not log.is_file():
        return ""
    for line in log.read_text(errors="replace").splitlines():
        if "driver: surface" in line and "i-leader" in line:
            return line
    return ""


def run_once(root: pathlib.Path, port: int, trust: bool, self_grant: bool) -> tuple[int, str, str]:
    """One session in the project directory; returns (exit, surface line, the merge line from the daemon log)."""
    state_root = root / "session"
    shutil.rmtree(root, ignore_errors=True)
    workspace = root / "repo"
    (workspace / ".teamagents").mkdir(parents=True)
    config_home = root / "config/teamagents"
    config_home.mkdir(parents=True)
    (config_home / "config.toml").write_text(user_config(port, trust))
    (workspace / ".teamagents/config.toml").write_text(project_config(self_grant))
    env = {"XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state"),
           "PATH": "/usr/bin:/bin", KEY_VAR: "not-a-key", "HOME": str(root),
           "TEAMAGENTS_LOG_SURFACE": "1"}
    run = subprocess.run(
        [str(BIN), "exec", "--state-root", str(state_root), "--full-auto", "--json", "--timeout", "60",
         "--cwd", str(workspace), "Do the work."],
        capture_output=True, text=True, env=env, timeout=180,
    )
    atexit.register(stop_daemon, state_root)
    log = (state_root / "daemon.log").read_text(errors="replace") if (state_root / "daemon.log").is_file() else ""
    merge = next((line for line in log.splitlines() if "project config" in line), "")
    return run.returncode, offered_tools(state_root), merge


def stop_daemon(state_root: pathlib.Path) -> None:
    """Stop the daemon this probe started (registered with `atexit`, like every other probe here)."""
    leak_guard.stop_daemons(state_root)


def judgement(runs: list) -> list:
    """The probe's rule, as a function of what it measured — separate so `--self-check` can exercise it."""
    failures = []
    for label, exit_code, surface, merge, want_tool in runs:
        if exit_code != 0:
            failures.append(f"{label}: the session should settle, exit={exit_code} ({merge or surface})")
        if want_tool and PROJECT_TOOL not in surface:
            failures.append(f"{label}: the trusted project's tool is not offered: {surface or '(no surface line)'}")
        if not want_tool and PROJECT_TOOL in surface:
            failures.append(f"{label}: the project's tool IS offered although it was refused: {surface}")
        if not merge:
            failures.append(f"{label}: the daemon log does not say what the project config contributed")
    return failures


def self_check() -> int:
    """Exercise `judgement` on the measured shapes, including the one a broken gate would produce."""
    findings = []
    trusted = "driver: surface i-leader shell=yes tools=ls,read_file," + PROJECT_TOOL + ",skill"
    untrusted = "driver: surface i-leader shell=yes tools=ls,read_file,skill"
    merge = "teamagents: project config /repo/.teamagents/config.toml (untrusted): accepted nothing"
    if judgement([("trusted", 0, trusted, merge, True), ("untrusted", 0, untrusted, merge, False)]):
        findings.append("the shapes this run produces must pass")
    if not judgement([("untrusted", 0, trusted, merge, False)]):
        findings.append("a gate that lets the untrusted project's tool through must be reported")
    if not judgement([("trusted", 0, untrusted, merge, True)]):
        findings.append("a trusted project whose tool never arrives must be reported")
    if not judgement([("trusted", 0, trusted, "", True)]):
        findings.append("a run the daemon log says nothing about must be reported")
    if not judgement([("trusted", 1, trusted, merge, True)]):
        findings.append("a turn that did not settle must be reported")
    for finding in findings:
        print(f"FAIL: {finding}")
    if not findings:
        print("self-check ok: the rule passes the measured shapes and reports a broken gate")
    return 1 if findings else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-project-config)")
    parser.add_argument("--self-check", action="store_true", help="check the rule only, with no session")
    args = parser.parse_args()
    if args.self_check:
        return self_check()
    if self_check():
        return 1
    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")

    server = ThreadingHTTPServer(("127.0.0.1", 0), FakeChat)
    port = server.server_address[1]
    threading.Thread(target=server.serve_forever, daemon=True).start()
    root = pathlib.Path(args.state_dir or "/tmp/ta-project-config")
    if not args.state_dir:
        atexit.register(shutil.rmtree, root, ignore_errors=True)
    runs = []
    failures: list[str] = []
    try:
        for label, trust, self_grant, want_tool in [
            ("untrusted", False, False, False),
            ("trusted", True, False, True),
            ("self-granting project", False, True, False),
        ]:
            exit_code, surface, merge = run_once(root / label.replace(" ", "-"), port, trust, self_grant)
            print(f"{label}: exit={exit_code} {surface or '(no surface line)'}")
            print(f"   {merge or '(the daemon log says nothing about the project config)'}")
            runs.append((label, exit_code, surface, merge, want_tool))
            if label == "self-granting project" and "permissions (" not in merge:
                failures.append("the project's own [permissions] table must be refused *by name*: the trust "
                                "decision is the user's, so a repository cannot grant itself the gate")
        failures += judgement(runs)
    finally:
        server.shutdown()
        FakeChat.seen = []

    for failure in failures:
        print("FAIL:", failure)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
