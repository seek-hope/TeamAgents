#!/usr/bin/env python3
"""The minimal TUI survives daemon loss and preserves a draft (A28/D-445).

A real daemon is killed and restarted on the same isolated state root. The
client must report disconnection, retain unsent input, reconnect and show a
fresh lifecycle reason from the restarted daemon. No model is called.

    python3 review/dogfood/tui_reconnect.py
"""
import argparse
import atexit
import fcntl
import json
import os
import pathlib
import pty
import shutil
import signal
import socket as socket_module
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
# the status line is truncated at the terminal width, so the needle is the word the client uses, not
# the whole sentence (" · disconnected, reconnecting...")
DISCONNECTED = "Disconnected"


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


def start_daemon(state_root: pathlib.Path, workspace: pathlib.Path, env: dict, log_path: pathlib.Path) -> subprocess.Popen:
    log = open(log_path, "a")
    return subprocess.Popen([str(BIN), "daemon", "--state-root", str(state_root), "--cwd", str(workspace)],
                            env=env, stdout=log, stderr=subprocess.STDOUT, text=True)


def wait_live(state_root: pathlib.Path, daemon: subprocess.Popen, timeout: float = 30) -> bool:
    """A daemon is up when it *answers*: after a SIGKILL the socket file is still there, so liveness is a
    connection and a reply, not a path (the same rule the CLI uses)."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        if daemon.poll() is not None:
            return False
        try:
            reply = protocol(state_root / "daemon.sock", "checkpoint", {})
            if reply.get("ok"):
                return True
        except OSError:
            pass
        time.sleep(0.1)
    return False


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-reconnect)")
    args = parser.parse_args()
    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")

    root = pathlib.Path(args.state_dir or "/tmp/ta-reconnect")
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
    try:
        daemon = start_daemon(state_root, workspace, env, root / "daemon.log")
        if not wait_live(state_root, daemon):
            failures.append(f"the daemon never bound its socket: {(root / 'daemon.log').read_text()[-300:]}")
            return 1
        spawned = protocol(state_root / "daemon.sock", "spawn_instance",
                           {"instance_id": WORKER, "workspace_ref": str(workspace)}, "reconnect-spawn")
        if not spawned.get("ok"):
            failures.append(f"spawn_instance was refused: {spawned.get('error')}")
            return 1

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
            failures.append(f"{label}: {needle!r} never appeared on screen")
            return False

        if not wait_for("Message", 30, "startup"):
            return 1
        os.write(fd, b"draft-kept-through-reconnect")

        # --- the daemon dies under it -------------------------------------------
        daemon.send_signal(signal.SIGKILL)
        daemon.wait(timeout=10)
        if not wait_for(DISCONNECTED, 20, "the client noticing the dead daemon"):
            print("first lines:", painted().splitlines()[:2])
            print(painted()[-400:])
            return 1
        print("  the client says it is disconnected")

        os.write(fd, b"\r")
        if not wait_for("Message kept in the draft", 15, "disconnected send"):
            return 1

        # --- a new daemon on the same state root --------------------------------
        daemon = start_daemon(state_root, workspace, env, root / "daemon.log")
        if not wait_live(state_root, daemon):
            failures.append("the second daemon never answered")
            return 1
        started = time.time()
        while time.time() - started < 90 and DISCONNECTED in painted():
            pump(0.5)
        reconnected = round(time.time() - started, 1)
        if DISCONNECTED in painted():
            failures.append(f"the client still says disconnected {reconnected}s after the daemon restarted")
            print("status line:", painted().splitlines()[0][:120])
        else:
            print(f"  the status line cleared after {reconnected}s")
        # A new reason proves a fresh checkpoint, not merely a cleared stale marker.
        changed = protocol(state_root / "daemon.sock", "set_lifecycle",
                           {"instance_id": "i-leader", "lifecycle": "PAUSED", "reason": "fresh-after-restart"},
                           "reconnect-pause")
        if not changed.get("ok"):
            failures.append(f"pause after restart refused: {changed.get('error')}")
        wait_for("fresh-after-restart", 15, "fresh state after reconnect")
        if "draft-kept-through-reconnect" not in painted():
            failures.append("reconnect lost the unsent draft")
        print("  reconnected, fresh state visible, unsent draft intact")
        os.write(fd, b"\x04")
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
        for failure in failures:
            print("FAIL:", failure)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
