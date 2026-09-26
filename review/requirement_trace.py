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

    python3 review/requirement_trace.py

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
ROW = re.compile(r"^\| Q(\d+) \| ([^|]*)\| ([^|]*)\|", re.M)
A_ITEM = re.compile(r"\bA\d\d\b")
D_ITEM = re.compile(r"\bD-\d+\b")
PATH = re.compile(r"`([A-Za-z0-9_./-]+\.(?:md|py|rs|json|toml|sh|jsonl))`")


def section_of(text: str, heading: str) -> str:
    """The text of one `## ` section, or `""` when the heading is absent (a finding, never a crash)."""
    if heading not in text:
        return ""
    return text.split(heading, 1)[1].split("\n## ", 1)[0]


def baseline_requirements() -> list:
    """The Q-numbers the baseline's §1 table lists, in order."""
    section = section_of(BASELINE.read_text(), "## 1. Confirmed requirements")
    return [int(n) for n in re.findall(r"^\| Q(\d+) \|", section, re.M)]


def trace_rows() -> dict:
    section = section_of(ACCEPTANCE.read_text(), SECTION)
    return {int(m.group(1)): (m.group(2).strip(), m.group(3).strip()) for m in ROW.finditer(section)}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--list", action="store_true", help="print every requirement with its evidence cell")
    args = parser.parse_args()
    findings = []
    baseline_has_section = "## 1. Confirmed requirements" in BASELINE.read_text()
    acceptance_has_section = SECTION in ACCEPTANCE.read_text()
    if not baseline_has_section:
        findings.append(f"{BASELINE.relative_to(REPO)} has no '## 1. Confirmed requirements' section to read")
    if not acceptance_has_section:
        findings.append(f"{ACCEPTANCE.relative_to(REPO)} has no '{SECTION}' section to read")
    wanted, rows = baseline_requirements(), trace_rows()
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
    print(f"{len(wanted)} confirmed requirements in the baseline; {len(rows)} evidence rows in "
          f"{ACCEPTANCE.relative_to(REPO)}")
    if args.list:
        for number in sorted(rows):
            print(f"  Q{number}: {rows[number][1][:150]}")
    for finding in findings:
        print("FAIL:", finding)
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())
