#!/usr/bin/env python3
"""Markdown tables whose rows do not match their header (D-199).

D-198 found three rows of `docs/ACCEPTANCE.md`'s two tables that were not rows of their table: a stray `|` inside
a cell (rendered as a spurious extra column) and, worse, a row that had lost its trailing pipe and was cut off
mid-sentence. `review/requirement_trace.py` now checks those two tables, but nothing checked any other table in
the repository — and there are many: the acceptance matrix's siblings, `verification/REPORT.md`'s evidence and
per-item ledgers, the user guide's tables, `docs/DEVELOPMENT.md`'s audit lists, the review registries. A row that
splits or merges there is just as invisible.

    python3 review/markdown_tables.py
    python3 review/markdown_tables.py --root docs   # the control, on a subtree

The rule: inside one table block, the separator row (`|---|---|`) defines the column count, and every row of that
block must have the same number of cells — where a separator is a pipe that is not escaped (`\\|` is how a
literal pipe is written inside a cell). Fenced code blocks are skipped, because a table shown there is an example
and not a table of this document. A block of pipe lines with no separator row is not a table and is skipped.

Ceiling: this is the shape only. A table whose cells are in the wrong order, or whose text is wrong, passes, and
the audit cannot tell a cell boundary meant as content from one meant as structure — it reports the count and the
line.
"""
import argparse
import pathlib
import re
import subprocess
import sys

REPO = pathlib.Path(__file__).resolve().parents[1]
# The frozen evaluation material keeps its original bytes (the manifests pin them), so its markdown is out of
# scope here; nothing else tracked is.
EXCLUDED = ("review/eval/", "review/tmp/")
FENCE = re.compile(r"^\s*(```|~~~)")
CELL = re.compile(r"(?<!\\)\|")
SEPARATOR = re.compile(r"^\s*\|[\s|:-]+\|\s*$")


def tracked_markdown() -> list[pathlib.Path]:
    out = subprocess.run(["git", "ls-files", "*.md"], cwd=REPO, capture_output=True, text=True, check=True).stdout
    return [REPO / name for name in out.split() if name and not name.startswith(EXCLUDED)]


def cells_of(line: str) -> int:
    """How many cells a table row has (the leading and trailing pipes do not count)."""
    return len(CELL.split(line.rstrip())) - 2


def findings_in(path: pathlib.Path, findings: list) -> int:
    """Every row that disagrees with its table's separator row. Returns the number of tables read."""
    lines = path.read_text(errors="replace").split("\n")
    name = str(path.relative_to(REPO))
    tables, fenced, block = 0, False, []
    def flush(block: list) -> None:
        nonlocal tables
        if not block:
            return
        expected = next((cells_of(line) for number, line in block if SEPARATOR.match(line)), None)
        if expected is None:
            return  # pipe lines with no separator row are not a table
        tables += 1
        for number, line in block:
            if cells_of(line) != expected:
                findings.append(
                    f"{name}:{number}: this row has {cells_of(line)} cell(s) where its table has {expected} — an "
                    f"unescaped `|` inside a cell (or a lost one) shifts or splits the row: {line.strip()[:80]}…"
                )
    for number, line in enumerate(lines, 1):
        if FENCE.match(line):
            fenced = not fenced
            continue
        if fenced:
            continue
        if line.lstrip().startswith("|"):
            block.append((number, line))
            continue
        flush(block)
        block = []
    flush(block)
    return tables


def main(argv) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", action="append", help="a directory to scan instead of every tracked markdown file")
    args = parser.parse_args(argv)
    paths = sorted((REPO / root).rglob("*.md")) if args.root else tracked_markdown()
    paths = [p for p in paths if not str(p.relative_to(REPO)).startswith(EXCLUDED)]
    findings, tables = [], 0
    for path in paths:
        tables += findings_in(path, findings)
    for finding in findings:
        print(f"FAIL: {finding}")
    if findings:
        return 1
    print(f"{tables} markdown table(s) across {len(paths)} file(s): every row matches its header's column count")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
