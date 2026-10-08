#!/usr/bin/env python3
"""D-448: real PTY graphics, protocol bounds, toggling and resize; no model request."""
import base64
import binascii
import fcntl
import os
import pathlib
import pty
import re
import signal
import struct
import tempfile
import termios
import time
import zlib

from pty_v2_smoke import FakeDaemon, BIN
from pty_screen import Screen, read_all

APC = re.compile(rb"\x1b_G.*?\x1b\\", re.S)


def png():
    def chunk(kind, data):
        return struct.pack("!I", len(data)) + kind + data + struct.pack("!I", binascii.crc32(kind + data))
    pixels = b"".join(b"\0" + bytes([40, 180, 240]) * 80 for _ in range(24))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack("!2I5B", 80, 24, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(pixels)) + chunk(b"IEND", b""))


def run(mode):
    with tempfile.TemporaryDirectory(prefix="graphics-pty-") as directory:
        root = pathlib.Path(directory)
        (root / "plot.png").write_bytes(png())
        daemon = FakeDaemon(str(root / "daemon.sock"))
        code = "```mermaid\nflowchart LR\n A[Build] --> B[Ship]\n```"
        daemon.history = [{"kind": "assistant", "workspace": str(root), "message": {"content": code}}]
        daemon.start()
        until = time.monotonic() + 3
        while not pathlib.Path(daemon.sock_path).exists():
            assert time.monotonic() < until
            time.sleep(.02)
        pid, fd = pty.fork()
        if pid == 0:
            env = dict(os.environ, TERM="xterm-256color", TEAMAGENTS_GRAPHICS=mode)
            os.execvpe(BIN, [BIN, "--daemon", daemon.sock_path], env)
        rows, cols = 30, 90
        fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, cols * 10, rows * 20))
        raw = bytearray()
        reaped = False

        def drain():
            raw.extend(read_all(fd, .1))
            screen = Screen(cols, rows)
            # The text emulator has no image decoder. Protocol fields and PNGs are independently checked below.
            screen.feed(APC.sub(b"", bytes(raw)).decode("utf-8", "replace"))
            return "\n".join(screen.lines())

        def wait(predicate, label):
            deadline = time.monotonic() + 12
            while time.monotonic() < deadline:
                shown = drain()
                if predicate(shown):
                    return shown
            raise AssertionError(f"{mode}: {label}\n{drain()}")

        try:
            if mode == "kitty":
                wait(lambda _: b"a=p,i=" in raw, "Mermaid PNG placement")
                chunks = re.findall(rb"\x1b_G(?:a=t,[^;]+|m=[01]);([^\x1b]*)\x1b\\", bytes(raw))
                decoded = base64.b64decode(b"".join(chunks))
                assert decoded.startswith(b"\x89PNG\r\n\x1a\n")
                width, height = struct.unpack("!II", decoded[16:24])
                assert width > 100 and height > 30
            else:
                wait(lambda s: "Build" in s and "Ship" in s and "flowchart" not in s, "Unicode Mermaid")
                assert b"\x1b_G" not in raw
            os.write(fd, b"unsent-graphics-draft\x07")
            wait(lambda s: "flowchart" in s and "unsent-graphics-draft" in s, "source toggle keeps draft")
            if mode == "kitty":
                assert b"a=d,d=I,i=" in raw
            os.write(fd, b"\x07")
            daemon.history = [{"kind": "assistant", "workspace": str(root), "message": {"content": "![Blue chart](plot.png)"}}]
            daemon.emit("response_imported", "i-leader", {})
            before = len(raw)
            if mode == "kitty":
                wait(lambda _: b"a=t,f=100" in raw[before:], "local image PNG")
            else:
                wait(lambda s: "▀" in s, "half-block image")
            for rows, cols in [(14, 42), (35, 110)]:
                before = len(raw)
                fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, cols * 10, rows * 20))
                os.kill(pid, signal.SIGWINCH)
                if mode == "kitty":
                    wait(lambda _: b"a=p,i=" in raw[before:], "resized image placement")
                    for y, x, command in re.findall(rb"\x1b\[(\d+);(\d+)H\x1b_G(a=p,[^;]+);", bytes(raw[before:])):
                        fields = dict(pair.split(b"=", 1) for pair in command.split(b","))
                        assert int(y) - 1 + int(fields[b"r"]) <= rows - 4, "image overlaps composer"
                        assert int(x) - 1 + int(fields[b"c"]) <= cols - 1, "image crosses transcript"
                else:
                    wait(lambda s: "unsent-graphics-draft" in s and "▀" in s, "resized image and draft")
            assert not any(f.get("command_id") for f in daemon.frames), "rendering emitted a business command"
            os.write(fd, b"\x04")
            for _ in range(60):
                drain()
                done, status = os.waitpid(pid, os.WNOHANG)
                if done:
                    reaped = True
                    assert os.waitstatus_to_exitcode(status) == 0
                    break
            assert reaped, "TUI did not exit"
            if mode == "kitty":
                assert b"a=d,d=I,i=" in raw and b"d=A" not in raw
            assert daemon.error is None, daemon.error
        finally:
            if not reaped:
                os.kill(pid, signal.SIGTERM)
                os.waitpid(pid, 0)
            os.close(fd)
    print(f"graphics PTY ({mode}): diagrams, image bytes, source toggle, draft, resize, bounds, cleanup: ok")


if __name__ == "__main__":
    run("blocks")
    run("kitty")
