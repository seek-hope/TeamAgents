#!/usr/bin/env python3
"""Check the current-rule index and Markdown rule references.

Only IDs explicitly declared in the Rule IDs column of docs/DECISIONS.md define rules.
Every definition needs a topic and current contract; duplicate IDs, an empty/missing index,
and unresolved citations fail. Historical marker words never excuse unresolved references.
This checks reference integrity, not the semantic completeness of a contract.
"""
import argparse
import pathlib
import re
import subprocess

REPO = pathlib.Path(__file__).resolve().parents[1]
LOG = "docs/DECISIONS.md"
HEADING = "## Current rule index"
NUMBER = re.compile(r"\bD-(\d+)\b")


def index(text):
    """Return explicitly defined IDs and structural findings."""
    problems, defined = [], set()
    if text.splitlines().count(HEADING) != 1:
        return defined, ["expected exactly one current-rule index"]
    section = text.split(HEADING, 1)[1].split("\n## ", 1)[0]
    rows = [line.strip() for line in section.splitlines() if line.strip().startswith("|")]
    if len(rows) < 3 or rows[0] != "| Topic | Current contract | Rule IDs |" or rows[1] != "|---|---|---|":
        problems.append("expected a nonempty Topic / Current contract / Rule IDs table")
    for row in rows[2:]:
        cells = [cell.strip() for cell in row.strip("|").split("|")]
        if len(cells) != 3 or not all(cells):
            problems.append(f"incomplete current-rule row: {row}")
            continue
        ids = cells[2].split(", ")
        if any(re.fullmatch(r"D-\d+", value) is None for value in ids):
            problems.append(f"malformed rule IDs: {cells[2]}")
            continue
        for value in ids:
            number = int(value[2:])
            if number in defined:
                problems.append(f"duplicate rule ID: {value}")
            defined.add(number)
    if not defined:
        problems.append("no current rules defined")
    return defined, problems


def findings(text, defined):
    return [(line, match.group(0))
            for line, value in enumerate(text.splitlines(), 1)
            for match in NUMBER.finditer(value) if int(match.group(1)) not in defined]


def self_check():
    clean = HEADING + "\n\n| Topic | Current contract | Rule IDs |\n|---|---|---|\n| State | Atomic writes | D-1, D-2 |\n"
    defined, problems = index(clean)
    assert defined == {1, 2} and not problems
    assert not findings("D-1 and D-2", defined)
    assert findings("History remains readable; D-99 applies.", defined) == [(1, "D-99")]
    for invalid in ["", HEADING, clean.replace("Atomic writes", ""),
                    clean + "| Other | Current rule | D-1 |\n",
                    clean.replace("D-1, D-2", "D-1 through D-2"), clean + HEADING]:
        assert index(invalid)[1], invalid
    misleading = clean.replace("Atomic writes", "D-99 is not a definition")
    assert 99 not in index(misleading)[0]
    print("current-rule audit controls passed")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--only", action="append", help="check this Markdown file instead of the tracked tree")
    parser.add_argument("--list", action="store_true", help="list defined rule IDs")
    parser.add_argument("--self-check", action="store_true", help="exercise structural and citation controls")
    args = parser.parse_args()
    if args.self_check:
        self_check()
        return 0
    defined, problems = index((REPO / LOG).read_text())
    for problem in problems:
        print(f"{LOG}: {problem}")
    if problems:
        return 1
    if args.list:
        print(", ".join(f"D-{number}" for number in sorted(defined)))
        return 0
    paths = args.only
    if paths is None:
        paths = subprocess.check_output(
            ["git", "ls-files", "--cached", "--others", "--exclude-standard"], cwd=REPO, text=True).splitlines()
        paths = sorted({p for p in paths if p.endswith(".md") and not p.startswith("tools/tmp/")})
    checked = failures = 0
    for path in paths:
        file = REPO / path
        if not file.is_file():
            if args.only:
                print(f"{path}: missing input")
                failures += 1
            continue
        checked += 1
        for line, name in findings(file.read_text(), defined):
            print(f"{path}:{line}: {name} has no current contract in {LOG}")
            failures += 1
    print(f"{checked} Markdown files, {len(defined)} current rule IDs, {failures} unresolved references")
    return int(bool(failures))


if __name__ == "__main__":
    raise SystemExit(main())
