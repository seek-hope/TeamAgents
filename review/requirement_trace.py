#!/usr/bin/env python3
"""Every confirmed requirement names its evidence (D-137)

The baseline's §1 lists the requirements the user confirmed (Q1–Q19), and D-42 closed with "the full
requirement mapping and acceptance are in the design baseline" — but there was no mapping. `docs/ACCEPTANCE.md`
carried per-*scenario* evidence for A01–A36, and the requirement list itself was referenced nowhere: fourteen of
the nineteen Q-items appeared in no document but `docs/DESIGN.md`, and the one requirement the evaluation
settled (Q16's collaboration gain) was not connected to the experiment that measured it.

`docs/ACCEPTANCE.md` now has the mapping — a row per requirement, naming the acceptance items, decisions,
probes or measurements that cover it — and this script is what keeps it complete:

* every `Q<n>` in the baseline's §1 table has a row in that section, and every row names a `Q<n>` the baseline
  actually lists (a row for a requirement that no longer exists is the same defect in reverse);
* every row cites something: an acceptance item (`A01`–`A36`), a decision (`D-<n>`), or a path that exists in
  the tree, so a row cannot be a placeholder.
* every row *is* a row of the table it belongs to: exactly three cells, so a stray `|` inside a cell (or a lost
  trailing one) cannot split it — the rendered table would gain a column and the reader would see the row shifted
  (D-198 found `A03`, `A14` and `A25` like that, one of them cut off mid-sentence).

The same two rules hold for the acceptance matrix itself, which is the other half of the definition of done
(§16: "A01–A36 have automated evidence"): every `A<n>` in the baseline's §12 matrix has exactly one row in
`docs/ACCEPTANCE.md`'s "Matrix A01–A36 (per-item evidence)", every row names an item the baseline lists, and no
row is a placeholder (the scenario and evidence cells are both non-empty). A duplicated row is a finding in
either half: the parse would otherwise keep the last one and the first would be invisible.

    python3 review/requirement_trace.py

Both documents can be pointed at copies, which is how the control is run:

    python3 review/requirement_trace.py --baseline /tmp/copy.md --acceptance /tmp/copy.md

Ceiling: this checks *coverage and shape*, not whether the cited evidence really covers the requirement — a row
citing an unrelated A-item passes. The judgement stays with the human who writes the row, which is what the
`--list` output is for.
"""
import argparse
import pathlib
import re
import sys

REPO = pathlib.Path(__file__).resolve().parents[1]
BASELINE = REPO / "docs/DESIGN.md"
ACCEPTANCE = REPO / "docs/ACCEPTANCE.md"
SECTION = "## Requirements (Q1–Q19) and their evidence"
A_SECTION = "## Matrix A01–A36 (per-item evidence)"
A_MATRIX = "## 12. Acceptance matrix"
ROW = re.compile(r"^\| Q(\d+) \| ([^|]*)\| ([^|]*)\|", re.M)
A_ROW = re.compile(r"^\| (A\d\d) \| ([^|]*)\| ([^|]*)\|", re.M)
A_ITEM_ROW = re.compile(r"^\| (A\d\d) \|", re.M)
A_ITEM = re.compile(r"\bA\d\d\b")
# a cell separator is a pipe that is not escaped: `\|` inside a cell is how a literal pipe is written
CELL = re.compile(r"(?<!\\)\|")
D_ITEM = re.compile(r"\bD-\d+\b")
PATH = re.compile(r"`([A-Za-z0-9_./-]+\.(?:md|py|rs|json|toml|sh|jsonl))`")


def section_of(text: str, heading: str) -> str:
    """The text of one `## ` section, or `""` when the heading is absent (a finding, never a crash)."""
    if heading not in text:
        return ""
    return text.split(heading, 1)[1].split("\n## ", 1)[0]


def baseline_requirements(baseline: pathlib.Path) -> list:
    """The Q-numbers the baseline's §1 table lists, in order."""
    section = section_of(baseline.read_text(), "## 1. Confirmed requirements")
    return [int(n) for n in re.findall(r"^\| Q(\d+) \|", section, re.M)]


def trace_rows(acceptance: pathlib.Path) -> dict:
    section = section_of(acceptance.read_text(), SECTION)
    return {int(m.group(1)): (m.group(2).strip(), m.group(3).strip()) for m in ROW.finditer(section)}


def baseline_items(baseline: pathlib.Path) -> list:
    """The A-numbers the baseline's §12 acceptance matrix lists, in order."""
    section = section_of(baseline.read_text(), A_MATRIX)
    return [name for name in A_ITEM_ROW.findall(section)]


def item_rows(acceptance: pathlib.Path) -> dict:
    """`{A<n>: (scenario, evidence)}` from the acceptance matrix, plus the duplicates seen."""
    section = section_of(acceptance.read_text(), A_SECTION)
    return {m.group(1): (m.group(2).strip(), m.group(3).strip()) for m in A_ROW.finditer(section)}


def table_shape(where: str, section: str) -> list:
    """Rows in `section` that are not exactly three cells (a stray or lost `|` splits or merges them)."""
    out = []
    for line in section.split("\n"):
        if not line.startswith("|") or set(line.strip()) <= set("|-: "):
            continue
        cells = len(CELL.split(line)) - 2  # the leading and trailing empty parts are not cells
        if cells != 3:
            out.append(f"{where}: a table row has {cells} cell(s) instead of 3 — a stray `|` inside a cell (or a "
                       f"lost trailing one) shifts the row in the rendered table: {line[:80]}…")
    return out


def duplicates(pattern, text: str, label) -> list:
    """Rows that appear more than once (the dict parse would silently keep the last).

    `label(captured)` renders the finding's subject: the Q pattern captures digits, the A pattern the whole item.
    """
    seen, twice = set(), []
    for match in pattern.finditer(text):
        number = match.group(1)
        if number in seen and number not in twice:
            twice.append(number)
        seen.add(number)
    return [f"{label(number)} has more than one row" for number in sorted(twice)]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--list", action="store_true", help="print every requirement with its evidence cell")
    parser.add_argument("--baseline", default=str(BASELINE), help="the design baseline to read (a copy is the control)")
    parser.add_argument("--acceptance", default=str(ACCEPTANCE), help="the acceptance ledger to read")
    args = parser.parse_args()
    baseline, acceptance_path = pathlib.Path(args.baseline), pathlib.Path(args.acceptance)
    findings = []
    baseline_text, acceptance = baseline.read_text(), acceptance_path.read_text()
    baseline_has_section = "## 1. Confirmed requirements" in baseline_text
    acceptance_has_section = SECTION in acceptance
    if not baseline_has_section:
        findings.append(f"{baseline} has no '## 1. Confirmed requirements' section to read")
    if not acceptance_has_section:
        findings.append(f"{acceptance_path} has no '{SECTION}' section to read")
    if acceptance_has_section:
        findings += table_shape(SECTION, section_of(acceptance, SECTION))
    if A_SECTION in acceptance:
        findings += table_shape(A_SECTION, section_of(acceptance, A_SECTION))
    wanted, rows = baseline_requirements(baseline), trace_rows(acceptance_path)
    if not wanted:
        findings.append("the baseline's §1 table lists no requirements — is the table still a table?")
    if not rows and acceptance_has_section:
        findings.append(f"the '{SECTION}' section has no rows")
    for number in wanted:
        if number not in rows:
            findings.append(f"Q{number} is confirmed in the baseline and has no evidence row in {SECTION}")
    for number in sorted(rows):
        if number not in wanted:
            findings.append(f"Q{number} has an evidence row but the baseline does not list it")
            continue
        short, evidence = rows[number]
        if not short:
            findings.append(f"Q{number}: the row does not say what the requirement is")
        if not (A_ITEM.search(evidence) or D_ITEM.search(evidence) or PATH.search(evidence)):
            findings.append(f"Q{number}: the row names no acceptance item, decision or path: {evidence!r}")
        for path in PATH.findall(evidence):
            if not (REPO / path).exists():
                findings.append(f"Q{number}: the row cites {path}, which does not exist")
    # the acceptance matrix: the other half of §16's definition of done
    items, matrix = baseline_items(baseline), item_rows(acceptance_path)
    if A_MATRIX not in baseline_text:
        findings.append(f"{baseline} has no '{A_MATRIX}' section to read")
    if A_SECTION not in acceptance:
        findings.append(f"{acceptance_path} has no '{A_SECTION}' section to read")
    if not items:
        findings.append("the baseline's acceptance matrix lists no items — is the table still a table?")
    findings += duplicates(ROW, section_of(acceptance, SECTION), lambda number: f"Q{number}")
    findings += duplicates(A_ROW, section_of(acceptance, A_SECTION), lambda number: number)
    for item in items:
        if item not in matrix:
            findings.append(f"{item} is in the baseline's acceptance matrix and has no evidence row")
    for item in sorted(matrix):
        if item not in items:
            findings.append(f"{item} has an evidence row but the baseline's acceptance matrix does not list it")
            continue
        scenario, evidence = matrix[item]
        if not scenario:
            findings.append(f"{item}: the row does not say what the scenario is")
        if not evidence:
            findings.append(f"{item}: the row carries no evidence")
    print(f"{len(wanted)} confirmed requirements and {len(items)} acceptance items in the baseline; "
          f"{len(rows)} requirement rows and {len(matrix)} matrix rows in {acceptance_path.name}")
    if args.list:
        for number in sorted(rows):
            print(f"  Q{number}: {rows[number][1][:150]}")
        for item in sorted(matrix):
            print(f"  {item}: {matrix[item][1][:150]}")
    for finding in findings:
        print("FAIL:", finding)
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())
