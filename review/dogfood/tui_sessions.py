#!/usr/bin/env python3
"""D-446: real-terminal session history, drafts, switching and deletion, with no model call."""
import fcntl
import json
import os
import pathlib
import pty
import signal
import sqlite3
import struct
import subprocess
import sys
import tempfile
import termios
import time

REPO = pathlib.Path(__file__).resolve().parents[2]
BIN = REPO / "engine/target/debug/teamagents"
TUI = REPO / "tui/target/debug/teamagents-tui"
sys.path.insert(0, str(REPO / "tui/scripts"))
from pty_screen import Screen, read_all  # noqa: E402


def main():
    with tempfile.TemporaryDirectory(prefix="ta-session-ui-") as directory:
        root = pathlib.Path(directory)
        base = root / "base"
        config = root / "config/teamagents"
        config.mkdir(parents=True)
        workspace = root / "workspace"
        workspace.mkdir()
        (config / "config.toml").write_text('skills_paths = []\n[models.leader_main]\nprovider = "openai"\nmodel = "test"\napi_key_env = "TA_SESSION_UI_KEY"\nbase_url = "http://127.0.0.1:1/v1"\ncontext_window = 128000\n')
        env = {**os.environ, "XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state"),
               "TA_SESSION_UI_KEY": "no-model-called", "TEAMAGENTS_TUI": str(TUI)}
        children = {}
        roots = [base]

        def cli(*args, state=base):
            result = subprocess.run([str(BIN), "--state-root", str(state), *args],
                                    env=env, cwd=workspace, capture_output=True, text=True, timeout=20)
            assert result.returncode == 0, (args, result.stderr)
            return result.stdout

        def seed(state, count, label):
            cli("sessions", "open", "--id", "default" if state == base else state.name, "--json")
            cli("daemon", "--stop", state=state)
            with sqlite3.connect(state / "session.sqlite") as db:
                db.executemany("INSERT INTO context_entries (instance_id,epoch,idx,id,kind,message_json,created) VALUES ('i-leader',0,?,?, 'user',?,0)",
                               [(n, f"fixture-{n}", json.dumps({"content": f"{label} {n}"})) for n in range(1, count + 1)])

        def launch():
            pid, fd = pty.fork()
            if pid == 0:
                os.chdir(workspace)
                os.execvpe(str(BIN), [str(BIN), "--state-root", str(base)], env)
            children[pid] = fd
            fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
            return pid, fd, Screen(120, 40)

        try:
            first = json.loads(cli("sessions", "new", "--name", "First", "--json"))
            archive = json.loads(cli("sessions", "new", "--name", "Archived", "--json"))
            first_root = pathlib.Path(first["path"])
            archive_root = pathlib.Path(archive["path"])
            roots.extend([first_root, archive_root])
            seed(base, 1, "Default history")
            seed(first_root, 450, "First entry")
            seed(archive_root, 1, "Archived history")
            cli("sessions", "archive", "--id", archive["id"])
            # A real failed open must leave the original client and draft intact.
            with sqlite3.connect(first_root / "session.sqlite") as db:
                db.execute("UPDATE instances SET workspace_ref=? WHERE id='i-leader'", (str(root / "missing-workspace"),))
            pid, fd, screen = launch()

            def pump(seconds=0.15):
                screen.feed(read_all(fd, seconds).decode("utf-8", "replace"))
                return "\n".join(screen.lines())

            def wait(text, seconds=15):
                until = time.monotonic() + seconds
                while time.monotonic() < until:
                    if text in pump():
                        return
                pathlib.Path("/tmp/session-ui-pty-failed-screen.txt").write_text(pump())
                raise AssertionError(f"did not paint {text!r}: {pump()}")

            def send(keys):
                os.write(fd, keys)
                pump()

            def exited():
                until = time.monotonic() + 15
                while time.monotonic() < until:
                    pump()
                    ended, status = os.waitpid(pid, os.WNOHANG)
                    if ended:
                        children.pop(pid)
                        os.close(fd)
                        assert os.waitstatus_to_exitcode(status) == 0, status
                        return
                raise AssertionError("successful current-session deletion did not exit the TUI")

            wait("Message")
            send(b"draft-default\x13")
            wait("Default history 1")
            send(b"\x1b[B")
            wait("First entry 251")
            assert not (first_root / "daemon.sock").exists(), "history must not boot the target"
            send(b"[")
            wait("First entry 51")
            send(b"[")
            wait("First entry 1")
            send(b"\r")
            wait("not a directory")
            send(b"\x1b")
            wait("draft-default")
            with sqlite3.connect(first_root / "session.sqlite") as db:
                db.execute("UPDATE instances SET workspace_ref=? WHERE id='i-leader'", (str(workspace),))
            send(b"\x13")
            wait("Default history 1")
            send(b"\x1b[B")
            wait("First entry 251")
            send(b"\r")
            wait("Message")
            assert "draft-default" not in pump(), "drafts may not travel into another session"
            send(b"draft-first\x13")
            wait("First entry 251")
            send(b"\x1b[H")
            wait("Default history 1")
            send(b"\r")
            wait("draft-default")
            # Return to First once: the exact unsent draft comes back.
            send(b"\x13")
            wait("Default history 1")
            send(b"\x1b[B")
            wait("First entry 251")
            send(b"\r")
            wait("draft-first")
            send(b"\x13")
            wait("First entry 251")
            send(b"\x1b[H")
            wait("Default history 1")
            send(b"\r")
            wait("draft-default")
            send(b"\x13")
            wait("Default history 1")
            send(b"\x1b[B")
            wait("First entry 251")
            send(b"d")
            wait("Delete First")
            send(b"\x1b[200~y\x1b[201~\r")
            assert first_root.exists(), "paste/Enter cannot confirm deletion"
            send(b"\x1b")
            protected = first_root / "instances/i-protected/work"
            protected.mkdir(parents=True)
            send(b"dy")
            wait("member workspace")
            assert first_root.exists()
            protected.rmdir()
            send(b"dy")
            wait("Default history 1")
            assert not first_root.exists(), "confirmed deletion removes another running session"
            send(b"\x1b[B")
            wait("Archived history 1")
            send(b"\r")
            wait("Message")
            send(b"\x13")
            wait("Archived history 1")
            send(b"d")
            wait("This is the current session")
            send(b"y")
            exited()
            assert not archive_root.exists()
            assert (base / "session.sqlite").exists(), "other session remains"
            # The default is deletable too, without erasing a named sibling.
            kept = json.loads(cli("sessions", "new", "--name", "Kept", "--json"))
            pid, fd, screen = launch()
            wait("Message")
            send(b"\x13")
            wait("Default history 1")
            send(b"d")
            wait("This is the current session")
            send(b"y")
            exited()
            assert not (base / "session.sqlite").exists()
            assert pathlib.Path(kept["path"]).exists()
            print("session TUI: paged idle/archive history, failed switch, per-session drafts, explicit confirmation, failed deletion, other/current/default deletion: ok")
        finally:
            for child, descriptor in children.items():
                try:
                    os.killpg(child, signal.SIGKILL)
                    os.waitpid(child, 0)
                except ProcessLookupError:
                    pass
                os.close(descriptor)
            for state in roots:
                subprocess.run([str(BIN), "--state-root", str(state), "daemon", "--stop"],
                               env=env, capture_output=True, timeout=15)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
