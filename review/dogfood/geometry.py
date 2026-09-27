#!/usr/bin/env python3
"""The TUI's geometry: an extreme terminal size, and a resize while it runs (product quality, D-233).

`make pty` boots the interface at one size (36×96) and asserts what is on screen; a *user* does two things it
never does: start the TUI in a pane of unusual proportions, and resize the window while it runs (a split, a
maximized terminal, a shrink to a sliver). Rendering code is where those become panics — a subtract with a
floor of zero, a layout that assumes more columns than it has — and a panic in the TUI costs the session's
window, not just a frame.

This probe attaches the real TUI to the same **fake** daemon the pty smoke uses (no credentials, no model, no
session state), boots it at each shape, resizes it mid-run where the shape says so, and requires of every shape:
no panic in its output and a clean exit 0 after the documented quit chord. Two assertions are shape-dependent,
because the status line cannot fit in a one-column terminal: the *control* (and every shape booted at the smoke's
36×96) must show the status line — the control's copy is the harness's own check, so a harness that silently
stopped working fails here instead of passing vacuously — while a tiny boot must at least have written something
to the terminal. Measured 2026-09-27: every shape was already clean — the guard is what makes that a fact that
has to stay true.

    make build
    python3 review/dogfood/geometry.py

Not part of `make check` (it starts a real terminal); the offline probe set runs it with the rest.

Ceiling: it asserts *that* the interface rendered and survived, not the layout — `make pty` checks the text at
one size, and nothing checks the pixels at another. A resize reaches the TUI as SIGWINCH, so a terminal that
changes size without it is not covered. The PTY is Linux's (`pty.fork`), and the sizes are the kernel's.
"""
import atexit
import fcntl
import os
import pathlib
import pty
import shutil
import struct
import sys
import tempfile
import termios
import time

REPO = pathlib.Path(__file__).resolve().parents[2]
TUI = REPO / "tui/target/debug/teamagents-tui"
sys.path.insert(0, str(REPO / "tui" / "scripts"))
from pty_screen import read_all  # noqa: E402  (the smoke's PTY reader)
from pty_v2_smoke import FakeDaemon  # noqa: E402  (the smoke's scripted session daemon)

# The status line `make pty` asserts on: if it does not arrive, the harness is broken, not the TUI.
RENDER_MARK = "s-test"
# The documented quit chord (`tui/src/v2app.rs`); Ctrl+C quits the interface, which is the smoke's own way out.
QUIT = b"\x03"
# (label, boot size, the resizes to apply in order)
SHAPES = [
    ("control 36x96", (36, 96), []),
    ("boot 1x1", (1, 1), []),
    ("boot 60x1", (60, 1), []),
    ("boot 1x60", (1, 60), []),
    ("36x96 then 5x5", (36, 96), [(5, 5)]),
    ("36x96 then 1x1", (36, 96), [(1, 1)]),
    ("36x96 then grow+shrink", (36, 96), [(60, 200), (3, 10)]),
    ("36x96 then one column", (36, 96), [(36, 1)]),
]


def set_size(fd: int, rows: int, cols: int) -> None:
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))


def one_shape(workdir: str, boot: tuple, resizes: list, settle: float = 0.8) -> tuple:
    """Boot a TUI in a pty at `boot`, apply `resizes`, quit, and report `(wrote, rendered, panic, code)`. """
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
    set_size(fd, *boot)
    out = read_all(fd, 2.5)
    rendered = RENDER_MARK in out.decode("utf-8", "replace")
    for rows, cols in resizes:
        set_size(fd, rows, cols)
        os.kill(pid, 28)          # SIGWINCH: what a terminal sends when its window changes
        out += read_all(fd, settle)
    os.write(fd, QUIT)
    out += read_all(fd, 0.6)
    code = None
    for _ in range(30):
        try:
            done, status = os.waitpid(pid, os.WNOHANG)
        except ChildProcessError:
            code = 0
            break
        if done == pid:
            code = os.waitstatus_to_exitcode(status)
            break
        time.sleep(0.1)
    if code is None:
        os.kill(pid, 15)
        try:
            os.waitpid(pid, 0)
        except ChildProcessError:
            pass
        code = "still running after the quit chord"
    text = out.decode("utf-8", "replace")
    return bool(out), rendered, ("panicked" in text or "RUST_BACKTRACE" in text), code


def main() -> int:
    if not TUI.is_file():
        raise SystemExit(f"{TUI} is missing; build it first (make build)")
    workdir = tempfile.mkdtemp(prefix="ta-geometry-")
    # the scratch is not state anyone keeps: one copy per run would accumulate in TMPDIR (D-138)
    atexit.register(shutil.rmtree, workdir, ignore_errors=True)
    failures = []
    for label, boot, resizes in SHAPES:
        wrote, rendered, panic, code = one_shape(workdir, boot, resizes)
        control = label.startswith("control")
        # the status line needs room: it is required where the smoke boots the interface, and elsewhere the
        # interface only has to have started (writing anything to a 1x1 terminal is already more than it must)
        must_render = control or boot == (36, 96)
        if must_render and not rendered:
            what = "the control" if control else "the shape"
            failures.append(f"{label}: {what} did not render {RENDER_MARK!r} at {boot[0]}x{boot[1]}"
                            + (" — the harness, not the TUI" if control else ""))
        if not must_render and not wrote:
            failures.append(f"{label}: the interface wrote nothing at {boot[0]}x{boot[1]}")
        if panic:
            failures.append(f"{label}: the interface panicked")
        if code != 0:
            failures.append(f"{label}: exit {code} instead of 0")
        ok = wrote and (rendered or not must_render) and not panic and code == 0
        print(f"{'ok  ' if ok else 'FAIL'} {label:24s} rendered={rendered} exit={code}")
    if failures:
        for failure in failures:
            print(f"FAIL: {failure}")
        return 1
    print(f"{len(SHAPES)} shapes: the interface rendered where it fits, survived every resize and exited 0")
    return 0


if __name__ == "__main__":
    sys.exit(main())
