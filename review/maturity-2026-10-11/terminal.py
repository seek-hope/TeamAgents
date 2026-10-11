#!/usr/bin/env python3
"""Real PTY, daemon and deterministic HTTP provider: attachment/queue/search input ownership."""
import argparse
import fcntl
import json
import os
from pathlib import Path
import pty
import signal
import struct
import sys
import termios
import threading
import time

from workflows import Fixture, ROOT, finish, wait
sys.path.insert(0, str(ROOT / "tui/scripts"))
from pty_screen import Screen, read_all


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin", type=Path, default=ROOT / "engine/target/debug/teamagents")
    parser.add_argument("--tui", type=Path, default=ROOT / "tui/target/debug/teamagents-tui")
    args = parser.parse_args()
    gate = threading.Event()
    with Fixture(args.bin.resolve(), "terminal", [{"content":"first reply"}, finish()], gate) as f:
        name = "attachment space \u03bb.txt"
        f.ws.joinpath(name).write_text("terminal snapshot original")
        env = {**f.env, "TEAMAGENTS_TUI":str(args.tui.resolve())}
        pid, fd = pty.fork()
        if pid == 0:
            os.chdir(f.ws)
            os.execvpe(str(args.bin.resolve()), f.argv(), env)
        alive = True
        fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH",32,110,0,0))
        screen = Screen(110,32)

        def pump(seconds=0.08):
            screen.feed(read_all(fd,seconds).decode("utf-8","replace"))
            return "\n".join(screen.lines())

        def painted(text):
            deadline = time.monotonic()+10
            while time.monotonic()<deadline:
                if text in pump():
                    return
            raise AssertionError(f"missing {text!r}:\n{pump()}")

        def send(data):
            os.write(fd,data)
            pump()

        def paste(text):
            send(b"\x1b[200~"+text.encode()+b"\x1b[201~")

        try:
            painted("Message")
            first = f.submit("hold terminal request")
            wait(lambda: len(f.seen)==1,"held request")
            paste("/attach att")
            send(b"\t")
            painted(name)
            send(b"\x1b")
            assert f.data("inputs","list")["inputs"] == []
            send(b"\t")
            painted(name)
            send(b"\r")  # fill only
            assert f.data("inputs","list")["inputs"] == []
            send(b"\r")  # stage only
            assert f.data("inputs","list")["inputs"] == []
            paste("queued terminal")
            send(b"\r")
            wait(lambda: len(f.data("inputs","list")["inputs"])==1,"queued terminal input")
            row = f.data("inputs","list")["inputs"][0]
            assert row["payload"]["attachments"][0]["label"] == name
            paste("kept draft")
            send(b"\x11")  # Ctrl-Q
            painted("Pending Leader inputs")
            send(b"\r")
            painted("Edit pending text")
            send(b"\x7f"*len("queued terminal"))
            paste("edited terminal")
            send(b"\r")
            wait(lambda: f.data("inputs","list")["inputs"][0]["revision"]==1,"queue edit")
            send(b"\x1b[3~")
            painted("y confirms")
            send(b"\x1b")
            assert len(f.data("inputs","list")["inputs"])==1
            send(b"\x1b[3~y")
            wait(lambda: not f.data("inputs","list")["inputs"],"queue cancellation")
            send(b"\x1b")
            painted("kept draft")
            send(b"\x7f"*len("kept draft"))
            paste("/attach "+name)
            send(b"\r")
            paste("final queued")
            send(b"\r")
            wait(lambda: len(f.data("inputs","list")["inputs"])==1,"final queued input")
            f.ws.joinpath(name).unlink()
            paste("draft after queue")
            gate.set()
            f.collect(first)
            wait(lambda: len(f.seen)==2,"snapshot delivery")
            wait(lambda: f.sql("SELECT status FROM goals")==[("SUCCEEDED",)],"settlement")
            sent=json.dumps(f.seen[-1]["messages"])
            assert "terminal snapshot original" in sent and "final queued" in sent
            assert "edited terminal" not in sent
            send(b"\x13")  # Ctrl-S
            painted("All sessions")
            painted("History")
            send(b"/")
            paste("no-match-451")
            send(b"\r")
            painted("No matching stored messages")
            send(b"\x1b")
            send(b"/")
            paste("final queued")
            send(b"\r")
            painted("Snippet only")
            painted("final queued")
            # Narrow rendering and cancellation return to the same live conversation draft.
            fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH",12,35,0,0))
            os.kill(pid,signal.SIGWINCH)
            pump()
            send(b"\x1b")
            send(b"\x1b")
            fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH",32,110,0,0))
            os.kill(pid,signal.SIGWINCH)
            painted("draft after queue")
            send(b"\x04")
            _, status = wait(lambda: (result if (result := os.waitpid(pid,os.WNOHANG))[0] else None), "TUI exit")
            alive=False
            assert os.waitstatus_to_exitcode(status)==0
            assert f.run("status","--json").returncode==0, "exit stopped the daemon"
            assert f.sql("SELECT COUNT(*) FROM input_revisions WHERE action='cancelled'")[0][0]==1
            f.evidence("PTY attachment-select-edit-cancel-search-draft")
            print("PASS: real PTY feature path; no real-model call")
        finally:
            gate.set()
            if alive:
                try:
                    os.kill(pid,signal.SIGKILL)
                    os.waitpid(pid,0)
                except (ProcessLookupError, ChildProcessError):
                    pass
            os.close(fd)


if __name__ == "__main__":
    main()
