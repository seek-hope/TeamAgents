#!/usr/bin/env python3
"""How fast the TUI keeps up with typing (product quality, measured rather than assumed).

The headline-path probe (`tui.py`) types the whole prompt at once, so what it sees on screen is the
render of a *paste*, and its only timing number is the model's answer. Two things follow, and this probe
answers both:

1. a keystroke typed while the client sits in its daemon-poll cycle must appear in the composer quickly —
   that is the interactive property a user feels;
2. a burst write of N characters needs N render passes, so "the answer was late" measured that way is not
   the same as "typing lags".

It attaches the real TUI to the **fake** daemon `tui/scripts/pty_v2_smoke.py` already uses (no credentials,
no model, no session state, so the number is the client's own timer), types one character at a time with an
idle second in between, and then repeats the burst. The bounds are deliberately loose (a loaded machine is
not a defect): a single keystroke must be on screen within a second, the median under a third of a second,
and a ten-character burst within two seconds. It prints the distribution either way.

    make build
    python3 review/dogfood/input_latency.py

Not part of `make check`; it is an observation with a coarse guard, and `make pty` covers the interface
itself deterministically.
"""
import fcntl
import os
import pathlib
import pty
import statistics
import struct
import sys
import tempfile
import termios
import time

REPO = pathlib.Path(__file__).resolve().parents[2]
TUI = REPO / "tui/target/debug/teamagents-tui"
sys.path.insert(0, str(REPO / "tui" / "scripts"))
from pty_screen import Screen, read_all  # noqa: E402  (the smoke's virtual terminal)
from pty_v2_smoke import FakeDaemon  # noqa: E402  (the smoke's scripted session daemon)

SINGLE_BOUND = 1.0
MEDIAN_BOUND = 0.3
BURST_BOUND = 2.0
WORD = "latencyabc"


def main() -> int:
    if not TUI.is_file():
        raise SystemExit(f"{TUI} is missing; build it first (make build)")
    workdir = tempfile.mkdtemp(prefix="ta-latency-")
    daemon = FakeDaemon(os.path.join(workdir, "daemon.sock"))
    daemon.start()
    for _ in range(200):
        if os.path.exists(daemon.sock_path):
            break
        time.sleep(0.02)
    pid, fd = pty.fork()
    if pid == 0:
        os.execvpe(str(TUI), [str(TUI), "--daemon", daemon.sock_path],
                   dict(os.environ, TERM="xterm-256color"))
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 36, 96, 0, 0))
    screen = Screen(96, 36)

    def pump(seconds: float) -> None:
        chunk = read_all(fd, seconds).decode("utf-8", "replace")
        if chunk:
            screen.feed(chunk)

    def painted() -> str:
        return "\n".join(screen.lines())

    def until(needle: str, timeout: float) -> float | None:
        """Seconds until `needle` is on screen, or None; drains in small steps so the timer is data-driven."""
        started = time.time()
        while time.time() - started < timeout:
            pump(0.01)
            if needle in painted():
                return time.time() - started
        return None

    deadline = time.time() + 30
    while time.time() < deadline and "i-leader" not in painted():
        pump(1.0)
    if "i-leader" not in painted():
        print("FAIL: the TUI never attached to the fake daemon")
        os.kill(pid, 9)
        return 1
    print(f"  attached to the scripted daemon ({daemon.sock_path})")

    failures: list[str] = []
    singles: list[float] = []
    typed = ""
    for character in WORD:
        pump(1.0)  # idle: the client has come back to its own poll before the key is typed
        typed += character
        os.write(fd, character.encode())
        latency = until(typed, 5.0)
        if latency is None:
            failures.append(f"keystroke {character!r} never appeared in the composer")
            break
        singles.append(latency)
    if singles:
        ordered = sorted(singles)
        print(f"  single keystrokes ({len(singles)}): min {ordered[0]:.2f}s "
              f"median {statistics.median(singles):.2f}s p95 {ordered[max(0, int(len(ordered) * 0.95) - 1)]:.2f}s "
              f"max {ordered[-1]:.2f}s")
        if ordered[-1] > SINGLE_BOUND:
            failures.append(f"a keystroke took {ordered[-1]:.2f}s (bound {SINGLE_BOUND}s)")
        if statistics.median(singles) > MEDIAN_BOUND:
            failures.append(f"keystroke median {statistics.median(singles):.2f}s (bound {MEDIAN_BOUND}s)")

    os.write(fd, b"\x7f" * len(typed))
    pump(0.5)
    os.write(fd, WORD.encode())
    burst = until(WORD, 5.0)
    if burst is None:
        failures.append("the burst never appeared in the composer")
    else:
        print(f"  burst of {len(WORD)} characters pasted as one write: {burst:.2f}s "
              f"({burst / len(WORD) * 1000:.0f}ms per character)")
        if burst > BURST_BOUND:
            failures.append(f"a {len(WORD)}-character burst took {burst:.2f}s (bound {BURST_BOUND}s)")

    os.write(fd, b"\x03")
    time.sleep(0.5)
    try:
        os.kill(pid, 9)
    except ProcessLookupError:
        pass
    for failure in failures:
        print("FAIL:", failure)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
