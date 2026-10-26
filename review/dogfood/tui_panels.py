#!/usr/bin/env python3
"""D-445: CLI management remains complete beside the minimal TUI.

The historical filename is retained for existing evidence links. The old
D-95 panel controls are now developer CLI verbs. This credential-free probe
creates a worker, pauses/resumes it, cancels a delegated task, verifies the
termination confirmation, and checks that the TUI stays a conversation.

    python3 review/dogfood/tui_panels.py
"""
import argparse
import atexit
import fcntl
import json
import os
import pathlib
import pty
import shutil
import socket as socket_module
import sqlite3
import struct
import subprocess
import sys
import termios
import time
import uuid

REPO = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "review"))   # shared pid-based stop (D-148)
import leak_guard  # noqa: E402
BIN = REPO / "engine/target/debug/teamagents"
sys.path.insert(0, str(REPO / "tui" / "scripts"))
from pty_screen import Screen, read_all  # noqa: E402  (the smoke's virtual terminal)

CONFIG = """skills_paths = []

[models.leader_main]
provider = "deepseek"
protocol = "deepseek"
model = "deepseek-flash"
api_key_env = "DEEPSEEK_API_KEY"
context_window = 1000000
"""

WORKER = "i-worker"
TASK = "t-panel"


def call(bin_args: list[str], env: dict, timeout: int = 60) -> subprocess.CompletedProcess:
    return subprocess.run([str(BIN), *bin_args], capture_output=True, text=True, env=env, timeout=timeout)


def protocol(socket_path: pathlib.Path, method: str, params: dict, command_id: str | None = None) -> dict:
    connection = socket_module.socket(socket_module.AF_UNIX)
    connection.settimeout(30)
    connection.connect(str(socket_path))
    stream = connection.makefile("rw")
    stream.readline()  # greeting
    request = {"protocol_version": 1, "request_id": f"probe-{uuid.uuid4()}", "method": method, "params": params}
    if command_id:
        request["command_id"] = command_id
    stream.write(json.dumps(request) + "\n")
    stream.flush()
    reply = json.loads(stream.readline())
    connection.close()
    return reply


def lifecycle(state_root: pathlib.Path, env: dict, instance: str) -> str:
    listed = call(["instances", "--state-root", str(state_root), "--json"], env)
    if not listed.stdout.strip().startswith("{"):
        return "?"
    rows = json.loads(listed.stdout)["instances"]
    return next((row["lifecycle"] for row in rows if row["id"] == instance), "missing")


def active_goal(state_root: pathlib.Path) -> str | None:
    db = sqlite3.connect(f"file:{state_root}/session.sqlite?mode=ro", uri=True)
    row = db.execute("SELECT id FROM goals WHERE status = 'ACTIVE' ORDER BY rowid LIMIT 1").fetchone()
    return row[0] if row else None


def task_status(state_root: pathlib.Path, env: dict, task_id: str) -> str:
    listed = call(["tasks", "--state-root", str(state_root), "--json"], env)
    if not listed.stdout.strip().startswith("{"):
        return "?"
    rows = json.loads(listed.stdout)["tasks"]
    return next((row["status"] for row in rows if row["id"] == task_id), "missing")


def wait_until(predicate, timeout: float, step: float = 0.25) -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        if predicate():
            return True
        time.sleep(step)
    return predicate()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-panels)")
    args = parser.parse_args()
    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")

    root = pathlib.Path(args.state_dir or "/tmp/ta-panels")
    # The default scratch is not state anyone keeps: remove it at exit, or one copy per run
    # accumulates in TMPDIR (D-138, the defect D-131 fixed for the test suite). An explicit
    # --state-dir is left alone, because the caller asked for it.
    if not args.state_dir:
        atexit.register(shutil.rmtree, root, ignore_errors=True)
    workspace = root / "ws"
    shutil.rmtree(root, ignore_errors=True)
    workspace.mkdir(parents=True)
    (root / "config/teamagents").mkdir(parents=True)
    (root / "config/teamagents/config.toml").write_text(CONFIG)
    state_root = root / "root"
    env = {**os.environ, "XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state")}
    # The daemon refuses to boot without a value for the config's `api_key_env`, and this probe calls no
    # model: supply one when the environment has none, so the probe really needs no credential (D-138).
    env.setdefault("DEEPSEEK_API_KEY", "no-model-called")
    failures: list[str] = []
    daemon = None
    pid = fd = None
    log = open(root / "daemon.log", "w")
    try:
        daemon = subprocess.Popen([str(BIN), "daemon", "--state-root", str(state_root), "--cwd", str(workspace)],
                                  env=env, stdout=log, stderr=subprocess.STDOUT, text=True)
        socket_path = state_root / "daemon.sock"
        if not wait_until(lambda: socket_path.exists() or daemon.poll() is not None, 30):
            failures.append("the daemon never bound its socket")
            return 1
        if not socket_path.exists():
            # read the log by path: `log` is opened for writing, and a failed read here would replace the
            # probe's own finding with a traceback (measured under the credential-free control, D-138)
            failures.append(f"the daemon exited: {(root / 'daemon.log').read_text(errors='replace')[-300:]}")
            return 1
        spawned = protocol(socket_path, "spawn_instance",
                           {"instance_id": WORKER, "workspace_ref": str(workspace)}, "panel-spawn")
        if not spawned.get("ok"):
            failures.append(f"spawn_instance was refused: {spawned.get('error')}")
            return 1
        print(f"  the session has one extra instance: {WORKER} ({lifecycle(state_root, env, WORKER)})")

        # --- the TUI, on the real session ---------------------------------------
        pid, fd = pty.fork()
        if pid == 0:
            os.execvpe(str(BIN), [str(BIN), "--state-root", str(state_root), "--cwd", str(workspace)], env)
        fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 100, 0, 0))
        screen = Screen(100, 40)

        def painted() -> str:
            return "\n".join(screen.lines())

        def pump(seconds: float) -> None:
            chunk = read_all(fd, seconds).decode("utf-8", "replace")
            if chunk:
                screen.feed(chunk)

        def wait_for(needle: str, timeout: float, label: str, step: float = 0.25) -> bool:
            started = time.time()
            while time.time() - started < timeout:
                pump(step)
                if needle in painted():
                    return True
            failures.append(f"{label}: {needle!r} never appeared on screen ({painted()[-200:]!r})")
            return False

        if not wait_for("Message", 30, "minimal conversation"):
            return 1
        os.write(fd, b"\x0e\t")
        pump(0.5)
        if "instances (" in painted():
            failures.append("retired navigation still opens a panel")

        def cli(*args):
            result = call([*args, "--state-root", str(state_root)], env)
            if result.returncode:
                failures.append(f"CLI {args}: {result.stderr}")
            return result

        cli("instances", "pause", "--id", WORKER)
        if lifecycle(state_root, env, WORKER) != "PAUSED":
            failures.append("CLI pause did not land")
        cli("instances", "resume", "--id", WORKER)
        if lifecycle(state_root, env, WORKER) != "ACTIVE":
            failures.append("CLI resume did not land")
        cli("instances", "pause", "--id", WORKER)
        goal = active_goal(state_root)
        delegated = protocol(socket_path, "delegate_task",
                             {"task_id": TASK, "assignee": WORKER, "goal_id": goal,
                              "description": "management probe: a task to cancel"}, "panel-delegate")
        if not delegated.get("ok"):
            failures.append(f"delegate_task refused: {delegated.get('error')}")
            return 1
        cli("tasks", "cancel", "--id", TASK)
        if task_status(state_root, env, TASK) != "CANCELLED":
            failures.append("CLI task cancellation did not land")
        refused = call(["instances", "terminate", "--id", WORKER, "--state-root", str(state_root)], env)
        if refused.returncode != 2 or lifecycle(state_root, env, WORKER) != "PAUSED":
            failures.append("termination without --yes changed state")
        cli("instances", "terminate", "--id", WORKER, "--yes")
        if lifecycle(state_root, env, WORKER) != "TERMINATED":
            failures.append("confirmed CLI termination did not land")
        status = json.loads(cli("status", "--json").stdout)
        if not any(i["id"] == WORKER and i["lifecycle"] == "TERMINATED" for i in status["snapshot"]["instances"]):
            failures.append("status does not report the managed worker")
        cli("history", "--id", WORKER, "--json")
        cli("goals", "--json")
        cli("authority", "--json")
        print("  minimal TUI with CLI pause/resume/cancel/confirmed termination/status/history: exercised")
        os.write(fd, b"\x11")
        pump(0.5)
    finally:
        if pid:
            try:
                os.kill(pid, 9)
            except ProcessLookupError:
                pass
        if fd is not None:
            try:
                os.close(fd)
            except OSError:
                pass
        if daemon is not None and daemon.poll() is None:
            daemon.terminate()
            try:
                daemon.wait(timeout=10)
            except subprocess.TimeoutExpired:
                daemon.kill()
        leak_guard.stop_daemons(state_root)
        log.close()
        for failure in failures:
            print("FAIL:", failure)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
