#!/usr/bin/env python3
"""Exercise /model add through a real terminal with isolated config and state."""
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import signal
import struct
import tempfile
import termios
import time
import tomllib

from pty_click_check import Screen

ROOT = Path(__file__).resolve().parents[2]


def main():
    with tempfile.TemporaryDirectory(prefix="ta-provider-pty-") as temporary:
        root = Path(temporary)
        project = root / "project"
        project.mkdir()
        config = root / "config" / "teamagents" / "config.toml"
        config.parent.mkdir(parents=True)
        config.write_text("# retain this comment\n[models.leader_main]\nprovider='openai'\nmodel='test'\nbase_url='http://127.0.0.1:9/v1'\n")
        env = dict(os.environ, TERM="xterm-256color", XDG_CONFIG_HOME=str(root / "config"),
                   XDG_STATE_HOME=str(root / "state"), TEAMAGENTS_ENGINE=str(ROOT / "engine/target/debug/teamagents"))
        binary = str(ROOT / "tui/target/debug/teamagents-tui")
        pid, fd = pty.fork()
        if pid == 0:
            os.execvpe(binary, [binary, "--cwd", str(project)], env)
        fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 36, 120, 0, 0))
        screen = Screen(120, 36)

        def pump(duration=0.15):
            deadline = time.monotonic() + duration
            while time.monotonic() < deadline:
                ready, _, _ = select.select([fd], [], [], 0.05)
                if ready:
                    try:
                        screen.feed(os.read(fd, 65536).decode("utf-8", "replace"))
                    except OSError:
                        break

        def expect(text):
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                pump()
                rendered = "\n".join("".join(row) for row in screen.cells)
                if text in rendered:
                    return
            raise AssertionError(f"missing {text!r}\n{rendered}")

        def send(data):
            os.write(fd, data)
            pump()

        def paste(text):
            send(b"\x1b[200~" + text.encode() + b"\x1b[201~")

        try:
            expect("TeamAgents")
            paste("/model add")
            send(b"\r")
            expect("Add custom provider")
            paste("pty-custom")
            send(b"\r")
            send(b"\x1b[C\x1b[C")  # Responses -> Anthropic -> Chat Completions
            expect("API format: chat/completions")
            send(b"\r")
            for value in ["http://127.0.0.1:9/v1", "pty-model", "", ""]:
                paste(value)
                send(b"\r")
            expect("Enter save")
            send(b"\r")
            expect("Provider saved")
            saved = tomllib.loads(config.read_text())
            profile = saved["models"]["pty-custom"]
            assert profile["protocol"] == "chat/completions", profile
            assert profile["model"] == "pty-model", profile
            assert "api_key_env" not in profile, profile
            assert "# retain this comment" in config.read_text()
            send(b"\r")  # member
            paste("pty-custom")
            send(b"\r")  # provider; failed discovery must retain the configured model
            expect("pty-model (pty-custom)")
            send(b"\r")  # model
            send(b"\r")  # default effort
            expect("Switched leader")
            overrides = list((root / "state/teamagents/sessions").glob("*/model_overrides.json"))
            assert len(overrides) == 1
            assert json.loads(overrides[0].read_text())["leader"]["profile"] == "pty-custom"
            send(b"\x11")
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                done, status = os.waitpid(pid, os.WNOHANG)
                if done:
                    assert os.waitstatus_to_exitcode(status) == 0
                    pid = None
                    break
                pump()
            assert pid is None, "TUI failed to quit"
            print("PTY model provider ok: add, save, select and persist through /model")
        finally:
            if pid is not None:
                os.kill(pid, signal.SIGKILL)
                os.waitpid(pid, 0)
            os.close(fd)


if __name__ == "__main__":
    main()
