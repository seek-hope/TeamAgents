#!/usr/bin/env python3
"""PTY check: clicking a table row selects exactly the row that was clicked.

Boots the real TUI (stacked layout so the sidebar starts at column 0), clicks
the row that shows a given member, and asserts the selection bar ▌ moved onto
that row — the display/click alignment the user reported as off by one.

Usage: XDG_STATE_HOME=/tmp/ta-click python3 tui/scripts/pty_click_check.py
"""
import fcntl, json, os, pty, re, select, struct, subprocess, sys, tempfile, termios, time

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
BIN = os.path.join(ROOT, "tui", "target", "debug", "teamagents-tui")
ENGINE = os.path.join(ROOT, "engine", "target", "debug", "teamagents")
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


def check(spec_path: str) -> int:
    workers = ["alpha", "beta", "gamma"] + [f"w{i}" for i in range(6)]
    agents = [
        '{"id": "leader", "name": "L", "role": "leader", "runtime_kind": "deepagents",'
        ' "model_profile": "leader_main", "tool_bindings": []}'
    ] + [
        f'{{"id": "{w}", "name": "{w[:1].upper()}", "role": "worker", "runtime_kind": "deepagents",'
        f' "model_profile": "leader_main", "tool_bindings": []}}'
        for w in workers
    ]
    with open(spec_path, "w") as fh:
        fh.write(
            '{"leader_id": "leader", "agents": [' + ", ".join(agents) + "],"
            ' "channels": [{"source": "leader", "targets": ' + json.dumps(workers) + ', "mode": "task"}]}'
        )
    pid, fd = pty.fork()
    if pid == 0:
        env = dict(os.environ, TERM="xterm-256color", TEAMAGENTS_ENGINE=ENGINE)
        os.execvpe(BIN, [BIN, "--cwd", "/tmp", "--team", spec_path], env)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 96, 0, 0))
    screen = Screen(96, 30)
    screen.feed(read_all(fd, 4.0).decode("utf-8", "replace"))

    def first_column_key(line: str):
        """The member id in the table's first column (the reach column also lists
        ids, so only a hit near the row start counts)."""
        best = None
        for key in ["leader"] + workers:
            pos = line.find(f"{key} ")
            if pos != -1 and (best is None or pos < best[0]):
                best = (pos, key)
        return best[1] if best and best[0] < 12 else None

    def click(col: int, row: int, timeout: float = 1.5):
        os.write(fd, f"\x1b[<0;{col + 1};{row + 1}M\x1b[<0;{col + 1};{row + 1}m".encode())
        screen.feed(read_all(fd, timeout).decode("utf-8", "replace"))
        return screen.lines()

    rows = screen.lines()
    target = next((i for i, l in enumerate(rows) if " beta " in l), None)
    if target is None:
        print("FAIL: could not locate the beta row in\n" + "\n".join(rows[:16]))
        os.kill(pid, 9)
        return 1
    # click column 6 (inside the sidebar), the row that shows beta
    rows = click(6, target)
    ok = any("▌beta" in l for l in rows)
    selected = next((l for l in rows if "▌" in l), "")
    if not ok:
        os.write(fd, b"\x11")
        time.sleep(0.5)
        os.kill(pid, 9)
        print("FAIL: the click did not select the row it pointed at")
        return 1

    # scrolled table: the wheel moves the selection, the window follows, and a
    # click must still land on the row it points at (the hint wraps at 96 cols)
    for _ in range(5):
        os.write(fd, b"\x1b[<65;6;6M")  # wheel down over the sidebar
        time.sleep(0.08)
    screen.feed(read_all(fd, 1.5).decode("utf-8", "replace"))
    rows = screen.lines()
    marked = next((l for l in rows if "▌" in l), "")
    if "▌beta" in marked:
        os.kill(pid, 9)
        print("FAIL: the wheel did not move the table selection")
        return 1
    data_rows = [i for i, l in enumerate(rows) if first_column_key(l)]
    if len(data_rows) < 3:
        os.kill(pid, 9)
        print("FAIL: too few table rows on screen:", rows[:16])
        return 1
    row = data_rows[-2]
    key = first_column_key(rows[row])
    rows = click(6, row)
    if not any(f"▌{key}" in l for l in rows):
        os.write(fd, b"\x11")
        time.sleep(0.5)
        os.kill(pid, 9)
        print(f"FAIL: after scrolling, clicking the {key} row selected "
              + next((l for l in rows if "▌" in l), "").strip()[:60])
        return 1

    # click a tab: the panel must switch to the one under the pointer
    rows = screen.lines()
    tabs_row = next((i for i, l in enumerate(rows) if "▍Team" in l or ("Team" in l and "Tasks" in l)), None)
    ok_tab = False
    if tabs_row is not None:
        line = rows[tabs_row]
        col = line.index("Log")
        rows = click(col, tabs_row)
        ok_tab = any("▍Log" in l for l in rows)
    os.write(fd, b"\x11")
    time.sleep(0.5)
    os.kill(pid, 9)
    print(f"clicked row {target} (beta); selection now: {selected.strip()[:60]!r}")
    print(f"wheel-scrolled, then clicked the {key} row: selection followed the pointer")
    print("clicked the Log tab:", "switched" if ok_tab else "FAILED")
    if not ok_tab:
        return 1
    print("PTY click check ok: row click == selection (plain + scrolled), tab click == panel")
    return 0


def main() -> int:
    with tempfile.TemporaryDirectory(prefix="ta-click-") as root:
        return check(os.path.join(root, "team.json"))


if __name__ == "__main__":
    sys.exit(main())
