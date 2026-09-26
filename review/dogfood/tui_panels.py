#!/usr/bin/env python3
"""The instances panel's keys against a real daemon (A21/D-68/D-82, the human surface).

The PTY smoke drives the panels against a *scripted* daemon: it asserts the frames the keys produce
(`set_lifecycle`, `cancel_task`). What it cannot say is whether the panel acts on the row a user selected and
whether the daemon really changes state. This probe attaches the real TUI to a real daemon and presses the
keys a user presses:

1. `Ctrl+N` opens the instances view, which shows the session's rows;
2. `p` pauses the selected instance, `r` resumes it — each must be visible in `teamagents instances --json`
   *and* on screen (the panel refreshes from the daemon's own events);
3. the tasks view's `c` cancels the task the user selected (the probe delegates one itself, as the user);
4. `t` only *asks* ("terminate this instance? y confirm / n cancel"): the instance must still be `ACTIVE`
   after `t`, still `ACTIVE` after `n`, and only `y` may retire it (D-82's finality, D-68's `--yes`).

No model is involved: the session is started, one extra instance is created through the documented protocol
(no user surface creates one without a Leader, and the panel's subject is the row, not the model), and every
assertion is about what the daemon recorded and what the panel paints.

    make build
    python3 review/dogfood/tui_panels.py
    python3 review/dogfood/tui_panels.py --state-dir /tmp/ta-panels

It needs no credential and no network; it stops the TUI and the daemon it started, and writes only under
`--state-dir`.
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

        def wait_gone(needle: str, timeout: float, label: str, step: float = 0.25) -> bool:
            started = time.time()
            while time.time() - started < timeout:
                pump(step)
                if needle not in painted():
                    return True
            failures.append(f"{label}: {needle!r} is still on screen")
            return False

        if not wait_for("i-leader", 60, "startup"):
            print(painted()[-600:])
            return 1
        print("  the TUI is attached; the status line names the leader it talks to")

        # --- the instances view --------------------------------------------------
        os.write(fd, b"\x0e")  # Ctrl+N: conversation -> instances
        if not wait_for("instances (", 20, "the instances view"):
            return 1
        if not wait_for(f"i-leader · ACTIVE", 20, "the leader's row"):
            return 1
        if not wait_for(f"{WORKER} · ACTIVE", 20, "the worker's row"):
            print(painted()[-800:])
            return 1
        print(f"  the instances panel lists both rows ({WORKER} · ACTIVE among them)")

        # the panel's selection starts on the conversation target (the leader), so the keys below must first
        # move to the worker: `▶  <id>` is the selected, non-target row the renderer paints
        os.write(fd, b"\x1b[B")  # Down
        if not wait_for(f"▶  {WORKER}", 15, "the selection moving to the worker"):
            print(painted()[-800:])
            return 1
        print("  Down moves the selection to the worker's row")

        # --- p pauses, and the panel follows the daemon --------------------------
        os.write(fd, b"p")
        if not wait_until(lambda: lifecycle(state_root, env, WORKER) == "PAUSED", 15):
            failures.append(f"the `p` key did not pause the instance: {lifecycle(state_root, env, WORKER)}")
        if not wait_for(f"{WORKER} · PAUSED", 15, "the panel follows the pause"):
            print(painted()[-400:])
        else:
            print("  `p` paused it in the daemon and the panel shows PAUSED")

        # --- r resumes -----------------------------------------------------------
        os.write(fd, b"r")
        if not wait_until(lambda: lifecycle(state_root, env, WORKER) == "ACTIVE", 15):
            failures.append(f"the `r` key did not resume the instance: {lifecycle(state_root, env, WORKER)}")
        if not wait_for(f"{WORKER} · ACTIVE", 15, "the panel follows the resume"):
            print(painted()[-400:])
        else:
            print("  `r` resumed it and the panel shows ACTIVE")

        # --- the tasks view: `c` cancels the selected row ------------------------
        goal = active_goal(state_root)
        if goal is None:
            failures.append("the session has no ACTIVE goal to charge a task to")
            return 1
        # The tasks view needs a *row*, not a running turn: hold the member while the task is delegated, or the
        # delegation starts a model call the probe's subject does not need (measured with a live credential: the
        # request began and the `c` key then abandoned it, D-138).
        held = protocol(socket_path, "set_lifecycle", {"instance_id": WORKER, "lifecycle": "PAUSED"}, "panel-hold")
        if not held.get("ok"):
            failures.append(f"the probe could not hold the member before delegating: {held.get('error')}")
            return 1
        delegated = protocol(socket_path, "delegate_task",
                             {"task_id": TASK, "assignee": WORKER, "goal_id": goal,
                              "description": "panel probe: a task to cancel"}, "panel-delegate")
        if not delegated.get("ok"):
            failures.append(f"delegate_task was refused: {delegated.get('error')}")
            return 1
        os.write(fd, b"\x0e")  # Ctrl+N: instances -> tasks
        if not wait_for("▶ " + TASK, 20, "the tasks view selecting the new task"):
            print(painted()[-800:])
            return 1
        status = task_status(state_root, env, TASK)
        if status not in ("PENDING", "RUNNING"):
            failures.append(f"the delegated task is {status}, so there is nothing to cancel")
            return 1
        print(f"  the tasks panel shows {TASK} · {status} (selected)")
        os.write(fd, b"c")
        if not wait_until(lambda: task_status(state_root, env, TASK) == "CANCELLED", 15):
            failures.append(f"the `c` key did not cancel the task: {task_status(state_root, env, TASK)}")
        if not wait_for(f"{TASK} · CANCELLED", 15, "the panel follows the cancellation"):
            print(painted()[-400:])
        else:
            print("  `c` cancelled it in the daemon and the panel shows CANCELLED")
        os.write(fd, b"\x1b")  # Esc: back to the conversation
        if not wait_for("to i-leader", 15, "the conversation view after Esc"):
            return 1
        os.write(fd, b"\x0e")  # and back into the instances view for the termination stage
        if not wait_for("instances (", 15, "the instances view again"):
            return 1
        os.write(fd, b"\x1b[B")  # Down: the selection starts on the conversation target again
        if not wait_for(f"▶  {WORKER}", 15, "the worker's row selected again"):
            return 1

        # --- t only asks; n cancels the prompt ----------------------------------
        # the member is held PAUSED since the delegation (no turn may start, D-138), so the point of these two
        # keys is that they change *nothing* — captured here and compared after each key
        before_keys = lifecycle(state_root, env, WORKER)
        os.write(fd, b"t")
        if not wait_for("terminate this instance? y confirm / n cancel", 15, "the confirmation prompt"):
            print(painted()[-400:])
        else:
            print("  `t` asked for confirmation")
        if lifecycle(state_root, env, WORKER) != before_keys:
            failures.append(f"`t` alone changed the instance: {before_keys} -> {lifecycle(state_root, env, WORKER)}")
        os.write(fd, b"n")
        if not wait_gone("terminate this instance?", 15, "the cancelled prompt"):
            print(painted()[-400:])
        else:
            print("  `n` cancelled the prompt")
        if lifecycle(state_root, env, WORKER) != before_keys:
            failures.append(f"`n` changed the instance anyway: {before_keys} -> {lifecycle(state_root, env, WORKER)}")

        # --- t then y retires it ------------------------------------------------
        os.write(fd, b"t")
        if not wait_for("terminate this instance?", 15, "the confirmation prompt (second time)"):
            return 1
        os.write(fd, b"y")
        if not wait_until(lambda: lifecycle(state_root, env, WORKER) == "TERMINATED", 30):
            failures.append(f"`y` did not retire the instance: {lifecycle(state_root, env, WORKER)}")
        if not wait_for(f"{WORKER} · TERMINATED", 20, "the panel follows the termination"):
            print(painted()[-400:])
        else:
            print("  `y` retired it and the panel shows TERMINATED")

        os.write(fd, b"\x03")  # quit the TUI
        time.sleep(0.5)
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
