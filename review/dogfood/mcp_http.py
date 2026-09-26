#!/usr/bin/env python3
"""A streamable-HTTP MCP binding, live: transport, bearer token, tool filter, and a missing secret (A25/D-74).

The `mcp_transport = "http"` half of the MCP surface had **no test at all** — the offline suite drives stdio
(`echo_catalog`), the live probe `mcp.py` drives stdio too, and an `http` binding is where a user meets a
remote service and its credential. This probe stands up a tiny MCP server over HTTP on loopback and drives it
with a real model:

1. the binding declares `mcp_transport = "http"`, `url`, `bearer_token_env_var` and `tool_names`, and the probe
   sets that variable: the model must be offered the one declared tool, its call must come back with the
   server's answer, and the server's own log must show `Authorization: Bearer <the token>` on every request;
2. the same config in a fresh session with the variable **unset**: a required service must fail the daemon's
   boot loudly, naming the variable, instead of starting with an empty secret.

    python3 review/dogfood/mcp_http.py

It needs `DEEPSEEK_API_KEY`, uses the native window (D-36), binds only 127.0.0.1 and writes under `--state-dir`.
"""
import argparse
import http.server
import json
import os
import pathlib
import re
import shutil
import socketserver
import subprocess
import sys
import threading
import time

REPO = pathlib.Path(__file__).resolve().parents[2]
BIN = REPO / "engine/target/debug/teamagents"

MODEL = """[models.leader_main]
provider = "deepseek"
protocol = "deepseek"
model = "deepseek-flash"
api_key_env = "DEEPSEEK_API_KEY"
context_window = 1000000
timeout = 180
max_retries = 2
generation_options = { reasoning_effort = "high" }
"""

PROMPT = ("Call the MCP tool `probe_ping` with the text `ping` and report exactly what it returned, then finish.")

TOOL_NAME = "probe_ping"


class McpServer(http.server.BaseHTTPRequestHandler):
    """Just enough of the streamable-HTTP transport (§2025-06-18) for the engine's client."""

    token = ""
    log_path: pathlib.Path
    calls = 0

    def log_message(self, *args):  # keep the console quiet
        pass

    def record(self, line: str):
        with open(self.log_path, "a") as handle:
            handle.write(line + "\n")

    def do_GET(self):  # the push stream: refusing it is allowed and means "no push"
        self.record(f"GET {self.path} push-stream-refused")
        self.send_response(405)
        self.end_headers()

    def do_POST(self):
        length = int(self.headers.get("content-length", "0"))
        body = json.loads(self.rfile.read(length) or b"{}")
        auth = self.headers.get("authorization", "")
        session = self.headers.get("mcp-session-id", "")
        version = self.headers.get("mcp-protocol-version", "")
        self.record(f"POST {self.path} auth={auth!r} session={session!r} version={version!r} "
                    f"method={body.get('method')!r}")
        if auth != f"Bearer {McpServer.token}":
            self.send_response(401)
            self.send_header("content-type", "application/json")
            self.end_headers()
            self.wfile.write(json.dumps({"jsonrpc": "2.0", "id": body.get("id"),
                                         "error": {"code": -32001, "message": "missing or wrong bearer token"}}
                                        ).encode())
            return
        method = body.get("method")
        if method == "initialize":
            result = {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}},
                      "serverInfo": {"name": "probe-http", "version": "1"}}
        elif method == "tools/list":
            result = {"tools": [{"name": "ping", "description": "answers ping",
                                 "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}}]}
        elif method == "tools/call":
            McpServer.calls += 1
            text = (body.get("params", {}).get("arguments") or {}).get("text", "")
            result = {"content": [{"type": "text", "text": f"probe-pong-{text}-{McpServer.calls}"}]}
        else:
            result = {}
        self.send_response(200 if method != "notifications/initialized" else 202)
        self.send_header("content-type", "application/json")
        self.send_header("mcp-session-id", "session-probe-1")
        self.end_headers()
        if method != "notifications/initialized":
            self.wfile.write(json.dumps({"jsonrpc": "2.0", "id": body.get("id"), "result": result}).encode())


def config_text(port: int, required: bool) -> str:
    return f"""# MCP-over-HTTP dogfood (D-104): a real model on the native context window (D-36).
skills_paths = []

{MODEL}
[tools.probe]
kind = "mcp"
mcp_server = "probe"
mcp_transport = "http"
mcp_execution = "host"
required = {str(required).lower()}
url = "http://127.0.0.1:{port}/mcp"
bearer_token_env_var = "PROBE_MCP_TOKEN"
startup_timeout_s = 10
tool_timeout_s = 30
tool_names = ["ping"]
"""


def call(bin_args: list[str], env: dict, timeout: int = 240) -> subprocess.CompletedProcess:
    return subprocess.run([str(BIN), *bin_args], capture_output=True, text=True, env=env, timeout=timeout)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-mcp-http)")
    args = parser.parse_args()
    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")
    if not os.environ.get("DEEPSEEK_API_KEY", "").strip():
        raise SystemExit("DEEPSEEK_API_KEY is not set in this environment")

    base = pathlib.Path(args.state_dir or "/tmp/ta-mcp-http")
    shutil.rmtree(base, ignore_errors=True)
    base.mkdir(parents=True)
    token = "probe-token-canary-7c1"
    log_path = base / "server.log"
    McpServer.token = token
    McpServer.log_path = log_path
    McpServer.calls = 0
    with socketserver.ThreadingTCPServer(("127.0.0.1", 0), McpServer) as server:
        port = server.server_address[1]
        port = int(port.value) if hasattr(port, "value") else int(port)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        print(f"  the probe's MCP server is on 127.0.0.1:{port}")
        failures: list[str] = []
        try:
            # --- 1. the binding works, with the token from the named env var -------------------
            root = base / "a"
            workspace = root / "ws"
            workspace.mkdir(parents=True)
            (root / "config/teamagents").mkdir(parents=True)
            (root / "config/teamagents/config.toml").write_text(config_text(port, required=True))
            env = {**os.environ, "XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state"),
                   "PROBE_MCP_TOKEN": token}
            started = time.time()
            run = call(["exec", "--state-root", str(root / "root"), "--full-auto", "--json", "--timeout", "180",
                        "--cwd", str(workspace), PROMPT], env)
            report = json.loads(run.stdout) if run.stdout.strip().startswith("{") else {}
            print(f"1. http binding: exit={run.returncode} end={report.get('end')} ({round(time.time() - started, 1)}s)")
            if run.returncode != 0:
                failures.append(f"the run failed: {run.returncode} {run.stderr.strip()[:200]}")
            entries = ""
            import sqlite3
            db = sqlite3.connect(f"file:{root / 'root/session.sqlite'}?mode=ro", uri=True)
            entries = "\n".join(row[0] or "" for row in db.execute("SELECT message_json FROM context_entries"))
            if "probe-pong-ping-1" not in entries:
                failures.append("the model's call did not come back with the server's answer")
            else:
                print("   the tool's answer is in the conversation: probe-pong-ping-1")
            lines = log_path.read_text().splitlines() if log_path.is_file() else []
            posts = [line for line in lines if line.startswith("POST")]
            authed = [line for line in posts if f"auth='Bearer {token}'" in line]
            print(f"   the server saw {len(posts)} POST(s), {len(authed)} of them carrying the token")
            if not posts:
                failures.append("the server was never called over HTTP")
            if len(authed) != len(posts):
                failures.append(f"some requests arrived without the bearer token: {posts}")

            # --- 2. the same binding with the variable unset: a required service must fail loudly -
            root = base / "b"
            workspace = root / "ws"
            workspace.mkdir(parents=True)
            (root / "config/teamagents").mkdir(parents=True)
            (root / "config/teamagents/config.toml").write_text(config_text(port, required=True))
            env = {**os.environ, "XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state")}
            env.pop("PROBE_MCP_TOKEN", None)
            run = call(["exec", "--state-root", str(root / "root"), "--full-auto", "--json", "--timeout", "60",
                        "--cwd", str(workspace), "Say ok."], env, timeout=120)
            words = (run.stdout + run.stderr).strip()
            print(f"2. the token missing: exit={run.returncode} {words[:160]!r}")
            if run.returncode != 2:
                failures.append(f"a required service with a missing secret must fail the session (exit 2), got {run.returncode}")
            # the *reason* lives in the session (the runtime parked the instance with it); the client's message
            # names the parked state and the lever, which is D-82's contract
            if "is PARKED" not in words or "instances resume --id i-leader" not in words:
                failures.append(f"the refusal does not name the parked leader and the lever: {words[:300]}")
            import sqlite3 as sqlite
            db = sqlite.connect(f"file:{root / 'root/session.sqlite'}?mode=ro", uri=True)
            lifecycles = [row[0] for row in db.execute("SELECT lifecycle FROM instances WHERE id = 'i-leader'")]
            reasons = [json.loads(row[0]).get("reason", "") for row in
                       db.execute("SELECT payload_json FROM events WHERE kind = 'instance_lifecycle'")]
            print(f"   the session: leader {lifecycles}, park reason {reasons[-1][:120] if reasons else '(none)'!r}")
            if lifecycles != ["PARKED"]:
                failures.append(f"the leader was not parked: {lifecycles}")
            if not any("PROBE_MCP_TOKEN" in reason for reason in reasons):
                failures.append(f"the park reason does not name the missing variable: {reasons}")
        finally:
            server.shutdown()
    for failure in failures:
        print("FAIL:", failure)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
