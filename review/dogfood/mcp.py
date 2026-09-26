#!/usr/bin/env python3
"""A real-model check that a configured MCP service is really bound (A25, D-74).

The probe writes a user config whose `[tools.probe]` entry is a tiny stdio MCP server
(`tools/list` offers `ping`, `tools/call` answers `pong`), then asks the model in one
headless run to call that tool and report its output:

    python3 review/dogfood/mcp.py                    # DeepSeek
    python3 review/dogfood/mcp.py --provider kimi    # over `responses`

It asserts the whole chain the design promises: the server starts with the session, the
model *sees* the tool (as `probe_ping`), it can call it, and the run reports the tool's
answer. Before D-74 nothing bound a configured service at all: the model's surface had
only the built-ins, and no run could ever reach this tool.

It is a real-model check: it needs `DEEPSEEK_API_KEY` (and `KIMI_API_KEY` for kimi), uses
each model's native window (D-36), writes only under `--state-dir`, and is not part of
`make check`.
"""
import argparse
import atexit
import json
import os
import pathlib
import shutil
import sqlite3
import subprocess
import sys
import time

REPO = pathlib.Path(__file__).resolve().parents[2]
BIN = REPO / "engine/target/debug/teamagents"

MODELS = {
    "deepseek": """[models.leader_main]
provider = "deepseek"
protocol = "deepseek"
model = "deepseek-flash"
api_key_env = "DEEPSEEK_API_KEY"
context_window = 1000000
timeout = 180
max_retries = 2
generation_options = { reasoning_effort = "high" }
""",
    "kimi": """[models.leader_main]
provider = "kimi"
protocol = "responses"
model = "k3-256k"
base_url = "https://api.kimi.com/coding/v1"
api_key_env = "KIMI_API_KEY"
timeout = 300
max_retries = 1
generation_options = { reasoning_effort = "low" }
context_window = 262144
""",
}

SERVER = '''#!/usr/bin/env python3
"""A minimal MCP stdio server: one tool, `ping`, answering a token it prints (D-74 probe).

The token is generated at start and written to `server.log`, so a model that answers
without calling the tool cannot know it: the probe tells a real call from a good guess.
"""
import json, os, pathlib, sys, uuid

log_path = pathlib.Path(os.path.join(os.path.dirname(os.path.abspath(__file__)), "server.log"))
TOKEN = "pong-" + uuid.uuid4().hex[:8]
with log_path.open("a") as log:
    log.write("token=" + TOKEN + "\\n")
sys.stdout.reconfigure(line_buffering=True)


def reply(request_id, result):
    print(json.dumps({"jsonrpc": "2.0", "id": request_id, "result": result}), flush=True)


for line in sys.stdin:
    try:
        request = json.loads(line)
    except ValueError:
        continue
    if "id" not in request:
        continue
    method = request.get("method")
    with log_path.open("a") as log:
        log.write(method + "\\n")
    if method == "initialize":
        reply(request["id"], {"protocolVersion": "2025-06-18", "capabilities": {},
                              "serverInfo": {"name": "probe", "version": "1"}})
    elif method == "tools/list":
        reply(request["id"], {"tools": [{"name": "ping", "description": "answers pong",
                                        "inputSchema": {"type": "object", "properties": {}}}]})
    elif method == "tools/call":
        reply(request["id"], {"content": [{"type": "text", "text": TOKEN}]})
    else:
        reply(request["id"], {})
'''

PROMPT = """Call the tool whose name ends in `ping` (it takes no arguments) and then report its output.
Reply with exactly the tool's output and nothing else."""



def stop_daemon(state_root: pathlib.Path) -> None:
    """Stop the daemon this probe started.

    `exec` autostarts one and it is detached on purpose (background work survives a client exit, §9), so
    without this a probe would leave a live session behind on the user's machine. Registered with `atexit`,
    which also covers the early returns above.
    """
    subprocess.run(["pkill", "-f", f"daemon --state-root {state_root}"], capture_output=True)

def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--provider", default="deepseek", choices=sorted(MODELS))
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-mcp)")
    parser.add_argument("--timeout", type=int, default=420, help="exec --timeout in seconds")
    args = parser.parse_args()
    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")
    key_env = "KIMI_API_KEY" if args.provider == "kimi" else "DEEPSEEK_API_KEY"
    if not os.environ.get(key_env, "").strip():
        raise SystemExit(f"{key_env} is not set in this environment")

    root = pathlib.Path(args.state_dir or f"/tmp/ta-mcp-{args.provider}")
    # The default scratch is not state anyone keeps: remove it at exit, or one copy per run
    # accumulates in TMPDIR (D-138, the defect D-131 fixed for the test suite). An explicit
    # --state-dir is left alone, because the caller asked for it.
    if not args.state_dir:
        atexit.register(shutil.rmtree, root, ignore_errors=True)
    workspace = root / "ws"
    shutil.rmtree(root, ignore_errors=True)
    (root / "config/teamagents").mkdir(parents=True)
    workspace.mkdir(parents=True)
    server = root / "probe_server.py"
    server.write_text(SERVER)
    server.chmod(0o755)
    (root / "config/teamagents/config.toml").write_text(
        "# The `[tools.*]` section is the binding (D-74): declaring an mcp service loads it.\n"
        "skills_paths = []\n\n"
        '[tools.probe]\nkind = "mcp"\ncommand = "/usr/bin/python3"\n'
        f'args = ["-u", "{server}"]\nmcp_execution = "host"\n\n' + MODELS[args.provider]
    )
    state_root = root / "root"
    atexit.register(stop_daemon, state_root)
    env = {**os.environ, "XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state")}
    failures: list[str] = []

    started = time.time()
    run = subprocess.run(
        [str(BIN), "exec", "--state-root", str(state_root), "--full-auto", "--json",
         "--timeout", str(args.timeout), "--cwd", str(workspace), PROMPT],
        capture_output=True, text=True, env=env,
    )
    elapsed = round(time.time() - started, 1)
    report = json.loads(run.stdout) if run.stdout.strip().startswith("{") else {}
    print(f"provider={args.provider} exec exit={run.returncode} elapsed={elapsed}s end={report.get('end')} "
          f"goal={report.get('goal_status')} reply={report.get('reply')!r}")
    if run.stderr.strip().startswith("exec:"):
        print("stderr:", run.stderr.strip()[:300])

    # 1. the service really started with the session and was asked for its tools
    log = root / "server.log"
    seen = log.read_text().split() if log.is_file() else []
    token = next((line.split("=", 1)[1] for line in seen if line.startswith("token=")), None)
    print(f"  the probe server was called for: {[line for line in seen if not line.startswith('token=')]}")
    if token is None:
        failures.append("the probe server never started (it prints its token when it does)")
        return 1
    if "tools/list" not in seen:
        failures.append("the configured MCP service never loaded (no tools/list): nothing bound it")
    if "tools/call" not in seen:
        failures.append("the model never called the bound tool (no tools/call); an answer without one is a guess")
    if (report.get("reply") or "").strip() and token not in report.get("reply", ""):
        failures.append(f"the run answered {report.get('reply')!r} instead of the tool's output ({token})")

    # 2. the run succeeded: the model either answered in prose or settled the goal
    if report.get("end") not in {"reply", "completed"}:
        failures.append(f"the run ended {report.get('end')!r} instead of finishing the tool call "
                        f"({report.get('failure')!r})")
    if run.returncode != 0:
        failures.append(f"the run exited {run.returncode}")

    # 3. the call really went through the receipt path: the conversation names the
    #    bound tool (<service>_<tool>) and carries its answer
    db = sqlite3.connect(state_root / "session.sqlite")
    entries = [row[0] for row in db.execute(
        "SELECT message_json FROM context_entries WHERE instance_id = 'i-leader' ORDER BY idx")]
    called = [entry for entry in entries if "probe_ping" in entry]
    if not called:
        failures.append("neither the tool call nor its receipt names probe_ping")
    else:
        print(f"  the conversation names the bound tool: {called[-1][:160]}")
    if not any(token in entry for entry in entries):
        failures.append(f"the tool's answer ({token}) never reached the conversation")

    for failure in failures:
        print("FAIL:", failure)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
