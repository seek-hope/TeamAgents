#!/usr/bin/env python3
"""Tests that contribute nothing without saying so (the detector behind D-121).

A test that skips has to be *visible*: the GitHub runner has no bubblewrap, so several isolation tests cannot
run there, and the convention is that each one either asserts the half it can still observe (D-113/D-114) or
prints why it is skipping. A bare `if <condition> { return; }` does neither — it turns the test into a no-op and
nobody finds out. Two sites did exactly that for as long as they existed (`tools.rs`'s cancelled-sandbox and
sandbox-build tests), and the loud skips were swept by hand before them.

    python3 review/silent_skips.py

The rule is deliberately narrow, so that a finding is always worth reading: inside a `#[test]`/`#[tokio::test]`
function, an `if ... {` block whose body (comments and blank lines ignored) is *only* `return;`. Guards inside a
nested helper — an `async fn` defined in the test, or a closure handed to `tokio::spawn` (a fake server's writer
loop, a polling helper whose timeout path panics) — are not skips and are not reported. This runs inside
`make hygiene`.

Limits: a skip written as `for ... { continue; }`, a `let _ = test_body()` that returns early, or a test that
quietly asserts nothing are all outside this rule; the first two are rare, the third is what review is for.
"""
import pathlib
import re
import sys

REPO = pathlib.Path(__file__).resolve().parents[1]
ROOTS = ["core/src", "core/tests", "engine/src", "engine/tests", "tui/src", "tui/tests"]
ATTRIBUTE = re.compile(r"#\[(?:tokio::)?test\]")
FUNCTION = re.compile(r"^\s*(?:pub\s+)?(?:async\s+)?fn\s+(\w+)")
NESTED = re.compile(r"^\s*(?:pub\s+)?(?:async\s+)?fn\s+\w+|[.:]spawn\(|spawn_blocking\(")
GUARD = re.compile(r"^(\s*)if\s+.*\{\s*$")


def scan(path, lines):
    """Silent early returns inside the file's test functions."""
    findings = []
    for index, line in enumerate(lines):
        if not ATTRIBUTE.search(line):
            continue
        start = index
        while start < len(lines) and not FUNCTION.match(lines[start]):
            start += 1
        if start >= len(lines):
            continue
        name = FUNCTION.match(lines[start]).group(1)
        depth, end = 0, len(lines)
        for cursor in range(start, len(lines)):
            depth += lines[cursor].count("{") - lines[cursor].count("}")
            if depth == 0 and cursor > start:
                end = cursor
                break
        for cursor in range(start, end):
            guard = GUARD.match(lines[cursor])
            if not guard:
                continue
            indent, body, scan_at = len(guard.group(1)), [], cursor + 1
            while scan_at < end:
                if lines[scan_at].strip() == "}" and len(lines[scan_at]) - len(lines[scan_at].lstrip()) == indent:
                    break
                body.append(lines[scan_at])
                scan_at += 1
            stripped = [line.strip() for line in body if line.strip() and not line.strip().startswith("//")]
            if stripped != ["return;"]:
                continue
            if any(NESTED.search(lines[between]) for between in range(start + 1, cursor)):
                continue  # a helper inside the test, not the test itself
            findings.append((str(path), cursor + 1, name, lines[cursor].strip()))
    return findings


def main():
    findings = []
    for root in ROOTS:
        for path in sorted(pathlib.Path(REPO / root).rglob("*.rs")):
            findings += scan(path.relative_to(REPO), path.read_text(errors="replace").split("\n"))
    for path, line, name, text in findings:
        print(f"{path}:{line}: {name} returns without saying why: {text}")
    if findings:
        print(f"\n{len(findings)} silent skip(s): print why (`eprintln!`) or assert the half that is observable")
        return 1
    print("no test returns early without saying why")
    return 0


if __name__ == "__main__":
    sys.exit(main())
