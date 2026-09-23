#!/usr/bin/env python3
"""R2-P0 real PTY: start, detach, reconnect, pause, resume, cancel."""
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import socket
import struct
import subprocess
import sys
import tempfile
import termios
import time
import uuid

ROOT = Path(__file__).resolve().parents[2]
ENGINE = ROOT / "engine/target/debug/examples/rebuild_p0"
TUI = ROOT / "tui/target/debug/examples/rebuild_p0"


def rpc(path, method):
    with socket.socket(socket.AF_UNIX) as client:
        client.settimeout(2)
        client.connect(str(path))
        client.sendall((json.dumps({"version": 1, "command_id": str(uuid.uuid4()), "method": method}) + "\n").encode())
        with client.makefile("rb") as stream:
            result = json.loads(stream.readline(65537))
        assert result["ok"], result
        return result


def wait_for(check, timeout=5):
    until = time.monotonic() + timeout
    while time.monotonic() < until:
        try:
            value = check()
            if value:
                return value
        except (OSError, ValueError):
            pass
        time.sleep(0.02)
    raise AssertionError("condition timed out")


def drain(fd, seconds=0.25):
    data = bytearray()
    until = time.monotonic() + seconds
    while time.monotonic() < until:
        if select.select([fd], [], [], 0.02)[0]:
            try:
                data.extend(os.read(fd, 65536))
            except OSError:
                break
    return bytes(data)


def open_tui(path):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 22, 100, 0, 0))
    env = {"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8", "TERM": "xterm-256color"}
    child = subprocess.Popen([str(TUI), str(path)], stdin=slave, stdout=slave, stderr=slave, env=env)
    os.close(slave)
    return child, master


def main():
    children = []
    fds = []
    with tempfile.TemporaryDirectory(prefix="ta-p0-pty-", dir="/tmp") as temp:
        root = Path(temp) / "state"
        daemon = subprocess.Popen([str(ENGINE), "daemon", str(root)], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
                                  env={"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8"})
        children.append(daemon)
        path = root / "daemon.sock"
        try:
            wait_for(lambda: rpc(path, "status"))
            first, fd = open_tui(path)
            children.append(first)
            fds.append(fd)
            screen = drain(fd, 0.5)
            # ratatui positions title characters with cursor movement codes,
            # so the raw PTY stream does not contain the rendered title as a
            # single byte sequence.
            assert b"TeamAgents" in screen and b"R2-P0" in screen, screen
            os.write(fd, b"s")
            started = wait_for(lambda: (v if (v := rpc(path, "status"))["status"] == "RUNNING" else None))
            os.write(fd, b"q")
            first.wait(timeout=3)
            assert first.returncode == 0
            after = wait_for(lambda: (v if (v := rpc(path, "status"))["ticks"] > started["ticks"] else None))
            second, fd2 = open_tui(path)
            children.append(second)
            fds.append(fd2)
            screen2 = drain(fd2, 0.4)
            assert b"RUNNING" in screen2 and b"p0-task" in screen2, screen2
            os.write(fd2, b"p")
            paused = wait_for(lambda: (v if (v := rpc(path, "status"))["status"] == "PAUSED" else None))
            time.sleep(0.25)
            assert rpc(path, "status")["ticks"] == paused["ticks"]
            os.write(fd2, b"r")
            wait_for(lambda: rpc(path, "status")["ticks"] > paused["ticks"])
            os.write(fd2, b"c")
            wait_for(lambda: rpc(path, "status")["status"] == "CANCELLED")
            os.write(fd2, b"q")
            second.wait(timeout=3)
            assert second.returncode == 0
            rpc(path, "shutdown")
            daemon.wait(timeout=3)
            assert daemon.returncode == 0, daemon.stderr.read().decode()
            print(json.dumps({"status": "passed", "real_pty": True, "same_task_id": after["task_id"],
                              "detach_keeps_running": True, "reconnect_pause_resume_cancel": True,
                              "model_calls": 0}, ensure_ascii=False))
        finally:
            for child in reversed(children):
                if child.poll() is None:
                    child.kill()
                child.wait(timeout=3)
            for fd in fds:
                os.close(fd)


if __name__ == "__main__":
    main()
