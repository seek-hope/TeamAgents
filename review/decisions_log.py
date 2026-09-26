#!/usr/bin/env python3
"""The shape of `docs/DECISIONS.md`, the file that decides what is binding (the detector behind D-107).

The decision log says which rules are in force and which deviations the user confirmed, so a log that lost an
entry, grew a second copy of one heading or buried an old block above the newest one is a silent documentation
defect: no gate read the file. It happened — an edit cut the tail block (D-48 … D-42), pasted it above D-105 and
left the original `## D-48` heading behind with no body, and D-54 sat between D-61 and D-60. Six hundred lines
of prose moved and every gate stayed green.

    python3 review/decisions_log.py

Rules, each one a shape the file is supposed to have:
  * every heading matches `## D-<n> <title> (<YYYY-MM-DD>)` — a number, a title and a date;
  * a number appears exactly once (the duplicate-heading defect above);
  * headings run newest-first, strictly descending in `n` (the buried-block defect);
  * a heading is followed by a blank line and at least MIN_BODY non-blank body lines before the next heading
    (the dangling-heading defect). MIN_BODY is 5; the shortest real entry has 12.

Deliberate limit: these are *shape* rules. An entry deleted whole leaves no trace, and whether an entry is true
stays a review question. The check is cheap and deterministic, so it runs inside `make hygiene` instead of
being a manual audit like `dead_code.py`/`config_keys.py`.
An optional path checks another copy, which is how the pre-fix control is run:

    git show ead5704:docs/DECISIONS.md > /tmp/DECISIONS.before.md   # the committed (broken) file
    python3 review/decisions_log.py /tmp/DECISIONS.before.md        # -> the four real problems
"""
import pathlib
import re
import sys

REPO = pathlib.Path(__file__).resolve().parents[1]
LOG = REPO / "docs" / "DECISIONS.md"
HEADING = re.compile(r"^## D-(\d+) (\S.*?) \((\d{4}-\d{2}-\d{2})\)$")
MIN_BODY = 5


def check(text, name=LOG.name):
    """Every structural complaint about `text`, each naming `name` and the line it is about."""
    lines = text.split("\n")
    problems = []
    heads = []  # (0-based index of the heading line, number)
    for index, line in enumerate(lines):
        if not line.startswith("## D-"):
            continue
        match = HEADING.match(line)
        if match is None:
            problems.append(f"{name}:{index + 1}: not `## D-<n> <title> (<YYYY-MM-DD>)`: {line!r}")
            continue
        heads.append((index, int(match.group(1))))
    first_seen = {}
    for index, number in heads:
        if number in first_seen:
            problems.append(f"{name}:{index + 1}: D-{number} appears twice (first at line {first_seen[number] + 1})")
        else:
            first_seen[number] = index
    for (index, number), (next_index, next_number) in zip(heads, heads[1:]):
        if number <= next_number:
            problems.append(f"{name}:{index + 1}: D-{number} sits above D-{next_number}; the log is newest-first")
    for position, (index, number) in enumerate(heads):
        next_index = heads[position + 1][0] if position + 1 < len(heads) else len(lines)
        following = lines[index + 1] if index + 1 < len(lines) else ""
        if following.strip() != "":
            problems.append(f"{name}:{index + 2}: a heading must be followed by a blank line")
        body = [line for line in lines[index + 1:next_index] if line.strip()]
        if len(body) < MIN_BODY:
            problems.append(
                f"{name}:{index + 1}: D-{number} has {len(body)} body lines before the next heading "
                f"(at least {MIN_BODY})"
            )
    return problems


def main(argv):
    path = pathlib.Path(argv[1]) if len(argv) > 1 else LOG
    shown = path.relative_to(REPO) if path.is_absolute() and REPO in path.parents else path
    problems = check(path.read_text(), path.name)
    for problem in problems:
        print(problem)
    if problems:
        print(f"\n{len(problems)} structural problem(s) in {shown} (see this script's docstring)")
        return 1
    entries = [line for line in path.read_text().split("\n") if HEADING.match(line)]
    print(f"{shown}: {len(entries)} entries, all unique, newest-first, each with a body")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
