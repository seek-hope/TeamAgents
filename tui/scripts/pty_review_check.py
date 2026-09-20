#!/usr/bin/env python3
"""Real terminal workspace review: external edits, paging, refresh and reopen.

No model calls, credentials or user configuration. Optional plain-text frames:
TEAMAGENTS_REVIEW_PTY_EVIDENCE=/tmp/review-frames python3 tui/scripts/pty_review_check.py
"""
import fcntl
import os
from pathlib import Path
import pty
import signal
import struct
import sys
import tempfile
import termios
import time

sys.dont_write_bytecode = True
from pty_click_check import BIN, ENGINE, Screen, read_all


def run(root, env, resumed=False):
    args = [BIN, "--cwd", str(root)]
    if resumed:
        sessions = list((Path(env["XDG_STATE_HOME"]) / "teamagents" / "sessions").glob("*/team.db"))
        assert len(sessions) == 1, "expected exactly one session to resume"
        args.extend(["--resume", sessions[0].parent.name])
    pid, fd = pty.fork()
    if pid == 0:
        os.execvpe(BIN, args, env)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 28, 110, 0, 0))
    screen = Screen(110, 28)
    exited = False
    evidence = os.environ.get("TEAMAGENTS_REVIEW_PTY_EVIDENCE")

    def expect(needle, name, keys=None):
        if keys is not None:
            os.write(fd, keys)
        deadline = time.monotonic() + 6
        while time.monotonic() < deadline:
            screen.feed(read_all(fd, 0.15).decode("utf-8", "replace"))
            text = "\n".join(screen.lines())
            if needle in text:
                if evidence:
                    Path(evidence).mkdir(parents=True, exist_ok=True)
                    Path(evidence, name + ".txt").write_text(text + "\n")
                return text
        raise AssertionError(f"{name}: {needle!r} missing\n{text}")

    try:
        expect("TeamAgents", "boot-resumed" if resumed else "boot")
        if not resumed:
            (root / "file.txt").write_text("".join(f"new-{n:03}\n" for n in range(260)))
        expect("modified", "files-resumed" if resumed else "files", b"/review\r")
        first = expect("-original input", "diff-resumed" if resumed else "diff-first", b"\r")
        assert "+new-000" in first
        if not resumed:
            expect("+new-114", "diff-page-2", b"n")
            expect("+new-234", "diff-page-3", b"n")
            expect("+new-114", "diff-page-back", b"p")
            info = expect("sha256=", "info", b"i")
            assert "first snapshot" in info and "Shared workspace" in info
            expect("+new-114", "back-from-info", b"\x1b")
            (root / "file.txt").write_text("new-000\nrefreshed\n")
            expect("+refreshed", "refreshed", b"r")
        expect("Approvals", "approvals-resumed" if resumed else "approvals", b"\x07")
        os.write(fd, b"\x11")
        deadline = time.monotonic() + 6
        while time.monotonic() < deadline:
            read_all(fd, 0.1)
            done, status = os.waitpid(pid, os.WNOHANG)
            if done:
                exited = True
                assert os.waitstatus_to_exitcode(status) == 0
                break
        assert exited, "Ctrl+Q did not exit"
    finally:
        if not exited:
            os.write(fd, b"\x11")
            time.sleep(0.5)
            done, _ = os.waitpid(pid, os.WNOHANG)
            if not done:
                os.killpg(pid, signal.SIGKILL)
                os.waitpid(pid, 0)
        os.close(fd)


def main():
    with tempfile.TemporaryDirectory(prefix="teamagents-review-pty-") as home:
        home = Path(home)
        root = home / "project"
        root.mkdir()
        (root / "file.txt").write_text("original input\n")
        config = home / "config" / "teamagents"
        config.mkdir(parents=True)
        (config / "config.toml").write_text(
            "[models.leader_main]\nprovider='local'\nmodel='test'\n"
            "base_url='http://127.0.0.1:1/v1'\nmax_retries=0\n"
        )
        env = dict(os.environ, TERM="xterm-256color", TEAMAGENTS_ENGINE=ENGINE,
                   XDG_CONFIG_HOME=str(home / "config"), XDG_STATE_HOME=str(home / "state"))
        run(root, env)
        run(root, env, resumed=True)
    print("PTY review ok: external edits, paging, metadata, refresh, approvals, quit, resumed baseline")
    return 0


if __name__ == "__main__":
    sys.exit(main())
