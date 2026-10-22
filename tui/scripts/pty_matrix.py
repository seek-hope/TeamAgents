#!/usr/bin/env python3
"""D-472: terminal-manager matrix for the PTY smokes.

Each profile gets a fresh pty with its own TERM, so the result does not depend
on the terminal that invoked `make pty`.  `--describe` prints the live profile
instead of running the matrix; the matrix proves the client's protocol and
screen, not a GUI terminal's pixels.
"""
import os
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
PY = sys.executable

# (TERM, label, extra env) — the expected mode is decided by the TUI itself.
PROFILES = [
    ("xterm-256color", "Alacritty, xterm, GNOME Terminal, iTerm2, VS Code", {}),
    ("screen-256color", "tmux, GNU screen", {}),
    ("tmux-256color", "tmux with modern terminfo", {}),
    ("xterm-kitty", "Kitty", {}),
    ("xterm-kitty", "Kitty inside tmux (blocks fallback)", {"TMUX": "1"}),
]
# Terminal-specific variables must not leak from the invoking terminal.
SANITIZED = (
    "KITTY_WINDOW_ID",
    "WEZTERM_EXECUTABLE",
    "WEZTERM_PANE",
    "ALACRITTY_SOCKET",
    "ALACRITTY_WINDOW_ID",
    "ZELLIJ",
    "TMUX",
)


def env_for(term, extra=None):
    env = dict(os.environ, TERM=term, TA_PTY_TERM=term, TEAMAGENTS_GRAPHICS="auto")
    for key in SANITIZED:
        env.pop(key, None)
    if extra:
        env.update(extra)
    return env


def run(script, term, extra=None):
    proc = subprocess.run([PY, str(HERE / script)], env=env_for(term, extra), text=True, capture_output=True)
    if proc.returncode != 0:
        print(proc.stdout, end="")
        print(proc.stderr, end="")
        raise SystemExit(f"{script} failed for TERM={term} {extra or ''}")
    lines = proc.stdout.strip().splitlines()
    return lines[-1] if lines else "ok"


def describe():
    for key in ("TERM", "TERM_PROGRAM", "COLORTERM", "KITTY_WINDOW_ID", "WEZTERM_EXECUTABLE",
                "ALACRITTY_SOCKET", "TMUX", "ZELLIJ"):
        print(f"{key}={os.environ.get(key, '')}")
    term = os.environ.get("TERM", "")
    native = term == "xterm-kitty" and "TMUX" not in os.environ and "ZELLIJ" not in os.environ
    print(f"resolved graphics mode: {'kitty' if native else 'blocks'}")


def main():
    if "--describe" in sys.argv:
        describe()
        return
    print("PTY matrix: one pty per terminal profile")
    for term, label, extra in PROFILES:
        run("pty_v2_smoke.py", term, extra)
        gfx = run("pty_graphics_smoke.py", term, extra)
        print(f"  {term} [{label}]: v2 ok; {gfx}")
    print("PTY matrix: protocol bounds (forced modes)")
    for mode in ("blocks", "kitty"):
        proc = subprocess.run(
            [PY, str(HERE / "pty_graphics_smoke.py"), mode],
            env=env_for("xterm-256color"),
            text=True,
            capture_output=True,
        )
        if proc.returncode != 0:
            print(proc.stdout, end="")
            print(proc.stderr, end="")
            raise SystemExit(f"protocol {mode} failed")
        print(f"  {proc.stdout.strip().splitlines()[-1]}")
    print("pty matrix: ok")


if __name__ == "__main__":
    main()
