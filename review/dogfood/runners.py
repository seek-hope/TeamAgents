#!/usr/bin/env python3
"""The `runners` lever: what a state root carries, and the safety that makes it usable (D-250).

ACCEPTANCE's known gap said a state root's leftover `jobs-runner` processes could only be retired by reopening
the session. This probe drives the real lever through the three situations it has to get right, on a real
session with a real command:

1. **a command that is genuinely in flight is listed and *not* taken away** — `teamagents runners` shows the
   job, its runner and the child's pid, and `runners stop` is refused by the runner's own gate
   (`active command must stop before shutdown`), with the command still running afterwards;
2. **it works with no session at all**, which is exactly when a leftover matters — SIGTERM the daemon, the
   runner survives (A12), and the report says `session_id: null` while the job is still there;
3. **a job whose work is over is retired** — once the command finishes on its own (the runner journals it
   without any daemon), `runners stop` retires the runner and the *process* is gone, which is the difference
   between a lever and a listing tool.

No credential and no network: a local chat-completions server answers the first request with a `shell` tool
call and the one after the tool result with a `finish`.

    python3 review/dogfood/runners.py
    python3 review/dogfood/runners.py --state-dir /tmp/ta-runners
"""
import argparse
import atexit
import json
import os
import pathlib
import shutil
import signal
import subprocess
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

REPO = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "review"))   # the shared pid-based stop (D-148)
import leak_guard  # noqa: E402

BIN = REPO / "engine/target/debug/teamagents"
KEY_VAR = "TEAMAGENTS_RUNNERS_PROBE_KEY"
HEAD = b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n"
# Long enough for the probe to look at the job while it runs, short enough to keep the probe under twenty
# seconds: the command's own lifetime is what step 3 waits for.
COMMAND = "sh -c 'echo started > marker.txt; sleep 8'"


def stream(*deltas: dict) -> bytes:
    """One scripted chat-completions SSE response from delta objects."""
    body = b"".join(f"data: {json.dumps(delta)}\n\n".encode() for delta in deltas) + b"data: [DONE]\n\n"
    return HEAD + body


def tool_call(name: str, arguments: dict) -> bytes:
    return stream(
        {"choices": [{"delta": {"tool_calls": [{"index": 0, "id": f"call-{name}", "type": "function",
                                                "function": {"name": name,
                                                             "arguments": json.dumps(arguments)}}]}}]},
        {"choices": [{"delta": {}, "finish_reason": "tool_calls"}],
         "usage": {"prompt_tokens": 5, "completion_tokens": 3, "total_tokens": 8}},
    )


SHELL_TURN = tool_call("shell", {"command": COMMAND, "timeout": 60})
FINISH_TURN = tool_call("finish", {"status": "success", "summary": "the command finished"})


class FakeChat(BaseHTTPRequestHandler):
    """The first turn of a conversation asks for a shell call; the turn after the tool result finishes."""

    protocol_version = "HTTP/1.0"

    def do_POST(self):  # noqa: N802 (http.server's name)
        length = int(self.headers.get("content-length") or 0)
        body = self.rfile.read(length) if length else b""
        try:
            conversation = json.loads(body.decode("utf-8", "replace")).get("messages", [])
        except json.JSONDecodeError:
            conversation = []
        reply = FINISH_TURN if any(m.get("role") == "tool" for m in conversation) else SHELL_TURN
        self.wfile.write(reply)
        self.wfile.flush()
        self.close_connection = True

    def log_message(self, *args):  # keep the probe's output clean
        pass


def config(port: int) -> str:
    return f"""# Runners probe (D-250): a local chat-completions server, no credentials, no network.
skills_paths = []

[models.leader_main]
provider = "openai"
protocol = "openai"
model = "runners-probe"
base_url = "http://127.0.0.1:{port}/v1"
api_key_env = "{KEY_VAR}"
context_window = 128000
timeout = 30
max_retries = 0
"""


def runners(state_root: pathlib.Path, env: dict, extra: list | None = None) -> tuple[int, dict, str]:
    """Run the real verb and return `(exit code, parsed JSON report, stderr)`."""
    done = subprocess.run([str(BIN), "runners", *(extra or []), "--json", "--state-root", str(state_root)],
                          capture_output=True, text=True, env=env, timeout=60)
    report = json.loads(done.stdout) if done.stdout.strip().startswith("{") else {}
    return done.returncode, report, done.stderr


def rows(report: dict) -> list:
    return report.get("runners", [])


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-runners)")
    args = parser.parse_args()
    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")
    root = pathlib.Path(args.state_dir or "/tmp/ta-runners")
    if not args.state_dir:
        atexit.register(shutil.rmtree, root, ignore_errors=True)
    shutil.rmtree(root, ignore_errors=True)
    workspace, state_root = root / "ws", root / "root"
    workspace.mkdir(parents=True)
    (root / "config/teamagents").mkdir(parents=True)
    server = ThreadingHTTPServer(("127.0.0.1", 0), FakeChat)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    (root / "config/teamagents/config.toml").write_text(config(server.server_address[1]))
    env = {"XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state"),
           "PATH": os.environ.get("PATH", "/usr/bin:/bin"), KEY_VAR: "not-a-key", "HOME": str(root)}
    atexit.register(leak_guard.stop_daemons, state_root)
    failures: list[str] = []

    # a real session runs a real command
    client = subprocess.Popen(
        [str(BIN), "exec", "--full-auto", "--json", "--state-root", str(state_root), "--cwd", str(workspace),
         "--timeout", "120", "run the command"],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=env)
    deadline = time.time() + 60
    while time.time() < deadline and not (workspace / "marker.txt").exists():
        time.sleep(0.2)
    running = (workspace / "marker.txt").exists()
    print(f"1. the command is running: {running}, daemon(s)={len(leak_guard.daemon_pids(state_root))}")
    if not running:
        failures.append("the scripted command never started (no marker.txt)")

    # 1. listing a live job, and the refusal that keeps it alive
    code, report, err = runners(state_root, env)
    listed = rows(report)
    first = listed[0] if listed else {}
    print(f"2. runners: exit={code} rows={len(listed)} session_id={report.get('session_id')!r} "
          f"state={first.get('state')!r} runner={first.get('runner')!r} child={first.get('child_pid')!r}")
    if code != 0 or len(listed) != 1:
        failures.append(f"the lever must list the one job: exit={code} {report} {err}")
    if first.get("state") != "RUNNING" or first.get("runner") != "live" or not first.get("child_pid"):
        failures.append(f"a running job is listed with its live runner and child: {first}")
    if report.get("session_id") is None:
        failures.append("a live session must be named in the report (it owns the jobs)")
    code, report, err = runners(state_root, env, ["stop"])
    outcome = rows(report)[0].get("outcome", "") if rows(report) else ""
    alive = (workspace / "marker.txt").exists() and client.poll() is None
    print(f"3. stop while it runs: exit={code} outcome={outcome!r} still running={alive}")
    if not outcome.startswith("refused:") or "active command" not in outcome:
        failures.append(f"the runner's own gate must refuse a stop with a child: {outcome!r} {err}")
    if not alive:
        failures.append("the refusal still let the command die: the lever must never take a running command away")

    # 2. the case the verb exists for: no session at all
    for pid, _args in leak_guard.daemon_pids(state_root):
        os.kill(pid, signal.SIGTERM)
    stopped = leak_guard.wait_until(lambda: not leak_guard.daemon_pids(state_root), 30)
    code, report, _ = runners(state_root, env)
    first = rows(report)[0] if rows(report) else {}
    print(f"4. no session: daemon gone={stopped} session_id={report.get('session_id')!r} "
          f"runner={first.get('runner')!r} state={first.get('state')!r}")
    if not stopped or report.get("session_id") is not None:
        failures.append(f"with no daemon the report says so: stopped={stopped} {report}")
    if first.get("runner") != "live":
        failures.append(f"the runner is a leftover precisely because it outlives the daemon: {first}")

    # 3. the work ends on its own (the runner journals it with no daemon), and now the lever retires it
    deadline = time.time() + 60
    finished = False
    while time.time() < deadline:
        code, report, _ = runners(state_root, env)
        if rows(report) and rows(report)[0].get("terminal"):
            finished = True
            break
        time.sleep(0.5)
    print(f"5. the command finished on its own: {finished} ({rows(report)[0].get('state') if rows(report) else None})")
    if not finished:
        failures.append("the runner never journaled the command's own ending")
    code, report, err = runners(state_root, env, ["stop"])
    outcome = rows(report)[0].get("outcome", "") if rows(report) else ""
    left = leak_guard.runner_pids(state_root)
    print(f"6. stop with the work over: exit={code} outcome={outcome!r} runners left={len(left)}")
    if outcome != "retired" or left:
        failures.append(f"the lever must really retire the runner: {outcome!r} left={left} {err}")

    # the id is addressed, and an unknown one is a usage error that names it
    code, _report, err = runners(state_root, env, ["stop", "--id", "no-such-job"])
    print(f"7. an unknown job: exit={code} {err.strip()[:70]!r}")
    if code != 2 or "no-such-job" not in err:
        failures.append(f"an unknown job id is a usage error naming it: {code} {err!r}")
    client.wait(timeout=30)

    leak_guard.stop_daemons(state_root)
    left = (len(leak_guard.daemon_pids(state_root)), len(leak_guard.runner_pids(state_root)))
    print(f"8. cleaned up: daemons/runners left = {left}")
    if left != (0, 0):
        failures.append(f"the probe left processes behind: {left}")
    for failure in failures:
        print("FAIL:", failure)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
