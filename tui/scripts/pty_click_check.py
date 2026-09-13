#!/usr/bin/env python3
"""PTY check: clicking a table row selects exactly the row that was clicked.

Boots the real TUI (stacked layout so the sidebar starts at column 0), clicks
the row that shows a given member, and asserts the selection bar ▌ moved onto
that row — the display/click alignment the user reported as off by one.

Usage: XDG_STATE_HOME=/tmp/ta-click python3 tui/scripts/pty_click_check.py
"""
import fcntl, os, pty, re, select, struct, subprocess, sys, termios, time

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
BIN = os.path.join(ROOT, "tui", "target", "debug", "teamagents-tui")
ENGINE = os.path.join(ROOT, "engine", "target", "debug", "teamagents")
SPEC = os.path.join(ROOT, "review", "tmp", "click_team.json")
ANSI = re.compile(r"\x1b\][^\x07]*\x07|\x1b\[[0-9;?]*[a-zA-Z]|\x1b[()][0-9A-B]|\x1b[=>]")


def read_all(fd, timeout=1.0):
    out = b""
    end = time.time() + timeout
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.1)
        if r:
            try:
                out += os.read(fd, 1 << 20)
            except OSError:
                break
    return out


class Screen:
    """Tiny ANSI screen: ratatui positions the cursor, so a buffer + CUP is enough."""

    def __init__(self, cols: int, rows: int) -> None:
        self.cols, self.rows = cols, rows
        self.cells = [[" "] * cols for _ in range(rows)]
        self.x = self.y = 0

    def feed(self, data: str) -> None:
        i = 0
        while i < len(data):
            ch = data[i]
            if ch == "\x1b":
                m = re.match(r"\x1b\[([0-9;?]*)([A-Za-z])", data[i:])
                if not m:
                    i += 1
                    continue
                params, cmd = m.group(1), m.group(2)
                nums = [int(p) for p in params.replace("?", "").split(";") if p.isdigit()]
                if cmd == "H":
                    self.y = (nums[0] - 1) if nums else 0
                    self.x = (nums[1] - 1) if len(nums) > 1 else 0
                elif cmd == "J":
                    self.cells = [[" "] * self.cols for _ in range(self.rows)]
                elif cmd == "K":
                    for x in range(self.x, self.cols):
                        self.cells[self.y][x] = " "
                elif cmd in "ABCD":
                    delta = nums[0] if nums else 1
                    if cmd == "A":
                        self.y = max(0, self.y - delta)
                    elif cmd == "B":
                        self.y = min(self.rows - 1, self.y + delta)
                    elif cmd == "C":
                        self.x = min(self.cols - 1, self.x + delta)
                    else:
                        self.x = max(0, self.x - delta)
                i += m.end()
                continue
            if ch == "\n":
                self.y = min(self.rows - 1, self.y + 1)
                i += 1
                continue
            if ch == "\r":
                self.x = 0
                i += 1
                continue
            if self.y < self.rows and self.x < self.cols:
                self.cells[self.y][self.x] = ch
            self.x += 1
            if self.x >= self.cols:
                self.x = 0
                self.y = min(self.rows - 1, self.y + 1)
            i += 1

    def lines(self) -> list[str]:
        return ["".join(row).rstrip() for row in self.cells]


def main() -> int:
    os.makedirs(os.path.dirname(SPEC), exist_ok=True)
    with open(SPEC, "w") as fh:
        fh.write(
            '{"leader_id": "leader", "agents": ['
            '{"id": "leader", "name": "L", "role": "leader", "runtime_kind": "deepagents",'
            ' "model_profile": "leader_main", "tool_bindings": []},'
            '{"id": "alpha", "name": "A", "role": "worker", "runtime_kind": "deepagents",'
            ' "model_profile": "leader_main", "tool_bindings": []},'
            '{"id": "beta", "name": "B", "role": "worker", "runtime_kind": "deepagents",'
            ' "model_profile": "leader_main", "tool_bindings": []},'
            '{"id": "gamma", "name": "G", "role": "worker", "runtime_kind": "deepagents",'
            ' "model_profile": "leader_main", "tool_bindings": []}],'
            ' "channels": [{"source": "leader", "targets": ["alpha", "beta", "gamma"], "mode": "task"}]}'
        )
    pid, fd = pty.fork()
    if pid == 0:
        env = dict(os.environ, TERM="xterm-256color", TEAMAGENTS_ENGINE=ENGINE)
        os.execvpe(BIN, [BIN, "--cwd", "/tmp", "--team", SPEC], env)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 96, 0, 0))
    screen = Screen(96, 30)
    screen.feed(read_all(fd, 4.0).decode("utf-8", "replace"))
    rows = screen.lines()
    target = next((i for i, l in enumerate(rows) if " beta " in l), None)
    if target is None:
        print("FAIL: could not locate the beta row in\n" + "\n".join(rows[:16]))
        os.kill(pid, 9)
        return 1
    # click column 6 (inside the sidebar), the row that shows beta
    seq = f"\x1b[<0;6;{target + 1}M\x1b[<0;6;{target + 1}m"
    os.write(fd, seq.encode())
    screen.feed(read_all(fd, 1.5).decode("utf-8", "replace"))
    rows = screen.lines()
    ok = any("▌beta" in l for l in rows)
    selected = next((l for l in rows if "▌" in l), "")
    os.write(fd, b"\x11")
    time.sleep(0.5)
    os.kill(pid, 9)
    print(f"clicked row {target} (beta); selection now: {selected.strip()[:60]!r}")
    if not ok:
        print("FAIL: the click did not select the row it pointed at")
        return 1
    print("PTY click check ok: clicked row == selected row")
    return 0


if __name__ == "__main__":
    sys.exit(main())
