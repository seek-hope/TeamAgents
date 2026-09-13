#!/usr/bin/env python3
"""PTY smoke for teamagents-tui: boots a real terminal session, types a
message, cycles panels, quits. Asserts on what lands on the screen.

Usage: XDG_STATE_HOME=/tmp/ta-pty python3 tui/scripts/pty_smoke.py
Requires: built tui + engine binaries (tui/target/... and engine/target/...).
"""
import fcntl, os, pty, termios, re, select, struct, subprocess, sys, time

BIN = os.path.join(os.path.dirname(__file__), "..", "target", "debug", "teamagents-tui")
ENV = dict(os.environ, TERM="xterm-256color", XDG_STATE_HOME=os.environ.get("XDG_STATE_HOME", "/tmp/ta-pty"))

def read_all(fd, timeout=1.5):
    out = b""
    end = time.time() + timeout
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.1)
        if r:
            try:
                out += os.read(fd, 65536)
            except OSError:
                break
    return out

def screen(raw: bytes) -> str:
    # strip CSI/OSC sequences; keep printable text
    txt = raw.decode("utf-8", "replace")
    txt = re.sub(r"\x1b\][^\x07]*\x07", "", txt)
    txt = re.sub(r"\x1b\[[0-9;?]*[a-zA-Z]", "", txt)
    txt = re.sub(r"\x1b[()][0-9A-B]", "", txt)
    txt = re.sub(r"\x1b[=>]", "", txt)
    return txt

def main():
    pid, fd = pty.fork()
    if pid == 0:
        os.execvpe(BIN, [BIN, "--cwd", "/tmp"], ENV)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 110, 0, 0))
    failures = []

    def expect(needle, label, raw):
        if needle not in screen(raw):
            failures.append(f"{label}: {needle!r} not on screen")

    boot = read_all(fd, 4.0)
    expect("TeamAgents ·", "status bar", boot)
    expect("Team", "team tab", boot)
    expect("›", "composer prefix", boot)
    expect("^q", "footer", boot)
    expect("Ready", "activity", boot)  # en default: "○ Ready · No turns executing"

    # type a message (no model configured → the turn fails; that's fine here)
    os.write(fd, b"hello leader")
    read_all(fd, 0.5)
    os.write(fd, b"\r")
    sent = read_all(fd, 3.0)
    expect("You", "user echo", sent)

    # cycle to tasks panel
    os.write(fd, b"\x14")  # ctrl+t
    tasks = read_all(fd, 1.5)
    expect("Tasks", "tasks tab", tasks)

    # jump to approvals, then settings twice
    os.write(fd, b"\x07")  # ctrl+g
    appr = read_all(fd, 1.5)
    expect("Approvals", "approvals tab", appr)

    # quit (dumb PTYs stall on crossterm's enhancement query; allow a few s)
    os.write(fd, b"\x11")  # ctrl+q
    exited = False
    for _ in range(12):
        time.sleep(0.5)
        try:
            done_pid, _ = os.waitpid(pid, os.WNOHANG)
            if done_pid:
                exited = True
                break
        except ChildProcessError:
            exited = True
            break
    if not exited:
        os.kill(pid, 9)
        failures.append("ctrl+q did not exit")

    if failures:
        print("FAIL:")
        for f in failures:
            print(" -", f)
        return 1
    print("PTY smoke ok: boot, message send, panel cycle, approvals jump, quit")
    return 0

if __name__ == "__main__":
    sys.exit(main())
