#!/usr/bin/env python3
"""Check current documentation counts against Cargo's test lists.

Run without arguments to audit registered statements. --write updates the counts.
Cargo lists tests for the current target without executing the test bodies.
"""
import argparse
import pathlib
import re
import subprocess
import sys
import tempfile

REPO = pathlib.Path(__file__).resolve().parents[1]
LEDGER = REPO / "docs" / "ACCEPTANCE.md"
CRATES = ("core", "engine", "tui")
BASELINE = re.compile(r"Current source inventory: core (\d+) / engine (\d+) / tui (\d+) test targets")
# The documents that state the same three numbers and are held to the ledger's: the published release body and
# the repository's own instruction file. A document that drops the sentence fails too — deleting a statement is
# how the check would otherwise go quiet — and `--write` updates all of them at once.
STATEMENTS = (REPO / ".github" / "release-notes.md", REPO / "AGENTS.md")
COUNTS = re.compile(r"core (\d+) / engine (\d+) / tui (\d+)")
TOTAL = r"(?P<total>\d+)"
TRIPLE = r"core (?P<core>\d+)[^\d\n]+engine (?P<engine>\d+)[^\d\n]+tui (?P<tui>\d+)"
HEADLINES = (
    ("README.md", r"^The source contains \*\*" + TOTAL + r" Rust tests\*\* \(" + TRIPLE + r"\)"),
    ("README.zh-CN.md", r"^\S+ \*\*" + TOTAL + r" [^*\n]*Rust[^*\n]*\*\*[^\d\n]*" + TRIPLE),
    ("AGENTS.md", r"^Current source inventory: \*\*" + TOTAL + r" Rust tests\*\* \(" + TRIPLE + r"\)"),
    (".github/release-notes.md", r"^The source contains \*\*" + TOTAL + r" Rust tests\*\* \(" + TRIPLE + r"\)"),
    ("docs/HANDOFF.md", r"^- \*\*" + TOTAL + r" Rust tests\*\*: " + TRIPLE),
    ("docs/CAPABILITIES.md", r"^- Rust tests: \*\*" + TOTAL + r"\*\* \(" + TRIPLE + r"\)"),
    ("docs/DESIGN.md", r"^The current source has " + TOTAL + r" Rust tests"),
    ("docs/DEVELOPMENT.md", r"^Current ledger: \*\*" + TRIPLE + r" = " + TOTAL + r" tests\*\*"),
    ("docs/ARCHITECTURE.md", r"^plus " + TOTAL + r" Rust tests"),
    ("docs/DECISIONS.md", r"^- Current source contains " + TOTAL + r" Rust tests: " + TRIPLE),
    ("docs/PRODUCT-COMPARISON.md", r"^- \*\*" + TOTAL + r" Rust tests\*\*"),
)
# `make test-aarch64` runs one engine module subset, so its size is quoted separately in four documents. It
# drifted once (the documents said 26 while the suite listed 27) and nothing compared them, so these statements
# are held to Cargo's own list for that selection. Group 1 is the number and group 2 the noun the sentence already
# uses, so an update rewrites the count without rewording the prose around it.
TOOL_STATEMENTS = (REPO / "AGENTS.md", REPO / "docs" / "DEVELOPMENT.md",
                   REPO / "docs" / "PRODUCT-COMPARISON.md", REPO / ".github" / "release-notes.md",
                   REPO / "docs" / "DECISIONS.md")
# Both spellings the tree uses: "27 tool tests" and "the 27-tool subset". Group 2 is the separator plus the noun, so
# an update rewrites the count without rewording the sentence. `\b` keeps "tools/" paths out.
TOOL_COUNT = re.compile(r"(\d+)([ -]tool\b)")
TOOL_SELECTION = ("--lib", "tools::tests::")


def stale_statements(path, wanted):
    """Report every stated triple that differs from the current source."""
    findings = []
    for number, line in enumerate(path.read_text().split("\n"), 1):
        for match in COUNTS.finditer(line):
            stated = f"core {match.group(1)} / engine {match.group(2)} / tui {match.group(3)}"
            if stated != wanted:
                name = path.relative_to(REPO) if path.is_relative_to(REPO) else path
                findings.append(f"{name}:{number} states {stated}, the suites have "
                                f"{wanted}")
    return findings


def current_headlines(root, counts, write=False):
    """Check registered headline counts and preserve surrounding prose when updating."""
    wanted = {**counts, "total": sum(counts.values())}
    findings, changed = [], []
    for relative, pattern in HEADLINES:
        path = root / relative
        text = path.read_text()
        matches = list(re.finditer(pattern, text, re.MULTILINE | re.IGNORECASE))
        if len(matches) != 1:
            findings.append(f"{relative}: expected one current test-count statement, found {len(matches)}")
            continue
        match = matches[0]
        replacements = []
        for field, value in match.groupdict().items():
            if int(value) != wanted[field]:
                replacements.append((*match.span(field), str(wanted[field])))
                if not write:
                    findings.append(f"{relative}: current {field} count is {value}, the suites have {wanted[field]}")
        if write and replacements:
            for start, end, value in sorted(replacements, reverse=True):
                text = text[:start] + value + text[end:]
            path.write_text(text)
            changed.append(f"{relative}: updated current counts")
    return findings, changed


def tool_tests() -> int:
    """The engine tests `make test-aarch64` runs: Cargo's own list for that selection, without executing them."""
    try:
        out = subprocess.run(
            ["cargo", "test", "--offline", "--manifest-path", "engine/Cargo.toml",
             *TOOL_SELECTION, "--", "--list"],
            cwd=REPO, capture_output=True, text=True, check=True).stdout
    except (subprocess.CalledProcessError, FileNotFoundError) as error:
        raise SystemExit(f"cannot list the tool-test subset ({error}); run `make build` first") from error
    return sum(1 for line in out.split("\n") if line.endswith(": test"))


def tool_statement_findings(wanted, files=None) -> tuple:
    """`(statements_read, findings)` for the documents that quote the tool-test count."""
    read, findings = 0, []
    for path in (TOOL_STATEMENTS if files is None else files):
        path = pathlib.Path(path)
        text = path.read_text(encoding="utf-8")
        name = path.relative_to(REPO) if path.is_relative_to(REPO) else path
        matches = TOOL_COUNT.findall(text)
        if len(matches) != 1:
            findings.append(f"{name}: expected one stated tool-test count, found {len(matches)}; a statement "
                            "that disappears is a check that goes quiet (D-221)")
            continue
        read += 1
        if int(matches[0][0]) != wanted:
            findings.append(f"{name} says {matches[0][0]} tool tests, the suite lists {wanted} for "
                            f"`make test-aarch64`")
    return read, findings


def self_check() -> int:
    """The tool-count rule must report a mismatch, a missing statement and a clean document."""
    assert [match.group(1) for match in TOOL_COUNT.finditer("covers 27 tool tests under emulation")] == ["27"]
    assert [match.group(1) for match in TOOL_COUNT.finditer("27 tool regressions")] == ["27"]
    assert [match.group(1) for match in TOOL_COUNT.finditer("runs the 26-tool GNU subset")] == ["26"], \
        "the hyphenated spelling states the same fact and must be checked too"
    assert TOOL_COUNT.findall("926 Rust tests") == [], "the crate ledger is not a tool-test statement"
    assert TOOL_COUNT.findall("`tools/test_counts.py` checks") == [], "a `tools/` path is not a count"
    with tempfile.TemporaryDirectory(prefix="teamagents-toolcount-") as scratch:
        root = pathlib.Path(scratch)
        (root / "ok.md").write_text("covers 27 tool tests, and 926 Rust tests\n")
        (root / "stale.md").write_text("covers 26 tool tests\n")
        (root / "absent.md").write_text("covers the tool subset\n")
        assert tool_statement_findings(27, [root / "ok.md"]) == (1, []), "an agreeing statement passes"
        read, found = tool_statement_findings(27, [root / "stale.md"])
        assert read == 1 and len(found) == 1 and "says 26" in found[0], f"drift must be reported: {found}"
        read, found = tool_statement_findings(27, [root / "absent.md"])
        assert read == 0 and len(found) == 1 and "expected one" in found[0], f"absence must be reported: {found}"
    print("tool-count controls passed: agreement, drift and a vanished statement")
    return 0


def counted() -> dict:
    """`{crate: tests}` from the crates' own test lists (exact, and no test body runs)."""
    counts = {}
    for crate in CRATES:
        try:
            out = subprocess.run(
                ["cargo", "test", "--offline", "--manifest-path", f"{crate}/Cargo.toml", "--", "--list"],
                cwd=REPO, capture_output=True, text=True, check=True).stdout
        except (subprocess.CalledProcessError, FileNotFoundError) as error:
            raise SystemExit(
                f"cannot list {crate}'s tests ({error}); run `make build` first, or `cargo test` by hand"
            ) from error
        counts[crate] = sum(1 for line in out.split("\n") if line.endswith(": test"))
    return counts


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--write", action="store_true", help="update the counts in docs/ACCEPTANCE.md")
    parser.add_argument("--self-check", action="store_true", help="audit the statement rules against fixtures")
    parser.add_argument("--ledger", default=str(LEDGER), help="the ledger to read (a copy is the control)")
    parser.add_argument("--statement", action="append", default=[], metavar="PATH",
                        help="a document that states the counts, instead of the default two (a copy is the "
                             "control)")
    args = parser.parse_args(argv)
    if args.self_check:
        return self_check()
    ledger = pathlib.Path(args.ledger)
    statements = [pathlib.Path(p) for p in args.statement] or list(STATEMENTS)
    check_headlines = ledger.resolve() == LEDGER.resolve() and not args.statement
    text = ledger.read_text()
    match = BASELINE.search(text)
    counts = counted()
    if match is None:
        print(
            f"FAIL: {ledger.name} has no source inventory line with core N / engine N / tui N test targets; "
            "baseline line; if the sentence changed on purpose, update this audit with it"
        )
        return 1
    documented = {crate: int(match.group(index + 1)) for index, crate in enumerate(CRATES)}
    if args.write:
        wanted = f"core {counts['core']} / engine {counts['engine']} / tui {counts['tui']}"
        changed = []
        if documented != counts:
            line = match.group(0)
            for index in reversed(range(3)):
                start, end = match.span(index + 1)
                line = line[:start - match.start()] + str(counts[CRATES[index]]) + line[end - match.start():]
            ledger.write_text(text[:match.start()] + line + text[match.end():])
            changed.append(f"{LEDGER.name}: {documented} -> {counts}")
        for path in statements:
            other = path.read_text()
            if not COUNTS.search(other):
                continue
            updated = COUNTS.sub(lambda _: wanted, other, count=1)
            if updated != other:
                path.write_text(updated)
                changed.append(f"{path.name}: updated to {wanted}")
        if check_headlines:
            findings, headline_changes = current_headlines(REPO, counts, write=True)
            changed.extend(headline_changes)
            if findings:
                for finding in findings:
                    print(f"FAIL: {finding}")
                return 1
        print("; ".join(changed) if changed else f"the counts are current everywhere: {wanted}")
        return 0
    findings = [
        f"{crate}: the ledger says {documented[crate]}, the suite has {counts[crate]}"
        for crate in CRATES
        if documented[crate] != counts[crate]
    ]
    wanted = f"core {counts['core']} / engine {counts['engine']} / tui {counts['tui']}"
    for path in statements:
        stated = COUNTS.search(path.read_text())
        name = path.relative_to(REPO) if path.is_relative_to(REPO) else path
        if stated is None:
            findings.append(f"{name} no longer states the baseline counts: one fact with several statements is "
                            "the shape here, and a statement that disappears is a check that goes quiet "
                            "(D-221)")
        elif f"core {stated.group(1)} / engine {stated.group(2)} / tui {stated.group(3)}" != wanted:
            findings.append(f"{name} states core {stated.group(1)} / engine {stated.group(2)} / "
                            f"tui {stated.group(3)}, the ledger {wanted}: the release body and the instruction "
                            "file are read as the baseline too (D-221)")
    for path in [ledger, *statements]:
        findings.extend(stale_statements(path, wanted))
    tool_total = 0
    if check_headlines:
        headline_findings, _ = current_headlines(REPO, counts)
        findings.extend(headline_findings)
        tool_total = tool_tests()
        _, tool_findings = tool_statement_findings(tool_total)
        findings.extend(tool_findings)
    if findings:
        for finding in findings:
            print(f"FAIL: {finding}")
        print("run `python3 tools/test_counts.py --write` to update the inventory")
        return 1
    print(f"the ledger's counts match the suites: core {counts['core']} / engine {counts['engine']} / "
          f"tui {counts['tui']}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
