#!/usr/bin/env python3
"""Virtual terminal screen for the PTY smokes (R29): diff-rendered frames are
replayed into a screen buffer so tests assert on what a user would see."""

import re, select, sys, unicodedata


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
    """Ratatui's positioned output, including split CSI and wide Unicode cells."""

    def __init__(self, cols: int, rows: int) -> None:
        self.cols, self.rows = cols, rows
        self.cells = [[" "] * cols for _ in range(rows)]
        self.x = self.y = 0
        self.pending = ""

    def feed(self, data: str) -> None:
        data = self.pending + data
        self.pending = ""
        i = 0
        while i < len(data):
            ch = data[i]
            if ch == "\x1b":
                if i + 1 == len(data):
                    self.pending = data[i:]
                    break
                if data[i + 1] == "]":
                    end = re.search(r"\x07|\x1b\\", data[i + 2:])
                    if end is None:
                        self.pending = data[i:]
                        break
                    i += 2 + end.end()
                    continue
                m = re.match(r"\x1b\[([0-?]*)[ -/]*([@-~])", data[i:])
                if not m:
                    if data[i + 1] == "[":
                        self.pending = data[i:]
                        break
                    if data[i + 1] in "()":
                        if i + 2 == len(data):
                            self.pending = data[i:]
                            break
                        i += 3
                    else:
                        i += 2
                    continue
                params, cmd = m.group(1), m.group(2)
                nums = [int(p) for p in params.replace("?", "").split(";") if p.isdigit()]
                if cmd in "Hf":
                    self.y = min(self.rows - 1, max(0, (nums[0] - 1) if nums else 0))
                    self.x = min(self.cols - 1, max(0, (nums[1] - 1) if len(nums) > 1 else 0))
                elif cmd == "J":
                    mode = nums[0] if nums else 0
                    for y in range(self.rows):
                        for x in range(self.cols):
                            if mode in (2, 3) or (mode == 0 and (y, x) >= (self.y, self.x)) or (mode == 1 and (y, x) <= (self.y, self.x)):
                                self.cells[y][x] = " "
                elif cmd == "K":
                    mode = nums[0] if nums else 0
                    for x in range(self.cols):
                        if mode == 2 or (mode == 0 and x >= self.x) or (mode == 1 and x <= self.x):
                            self.cells[self.y][x] = " "
                elif cmd == "G":
                    self.x = min(self.cols - 1, max(0, (nums[0] if nums else 1) - 1))
                elif cmd == "d":
                    self.y = min(self.rows - 1, max(0, (nums[0] if nums else 1) - 1))
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
            if ch == "\b":
                self.x = max(0, self.x - 1)
                i += 1
                continue
            if ch == "\t":
                self.x = min(self.cols - 1, (self.x // 8 + 1) * 8)
                i += 1
                continue
            if ord(ch) < 32 or ch == "\x7f":
                i += 1
                continue
            if unicodedata.combining(ch):
                x = max(0, self.x - 1)
                while x and self.cells[self.y][x] == "":
                    x -= 1
                self.cells[self.y][x] += ch
                i += 1
                continue
            width = 2 if unicodedata.east_asian_width(ch) in "WF" else 1
            if self.y < self.rows and self.x < self.cols:
                self.cells[self.y][self.x] = ch
                if width == 2 and self.x + 1 < self.cols:
                    self.cells[self.y][self.x + 1] = ""
            self.x += width
            if self.x >= self.cols:
                self.x = 0
                self.y = min(self.rows - 1, self.y + 1)
            i += 1

    def lines(self) -> list[str]:
        return ["".join(row).rstrip() for row in self.cells]
