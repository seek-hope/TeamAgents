#!/usr/bin/env python3
"""Citations in the documentation that point at nothing in the tree (the detector behind D-109).

`docs/ACCEPTANCE.md` is the evidence ledger and `review/README.md` is the index of it, so their citations —
`` `v2_driver::end_to_end_shell_then_finish` ``, `` `review/dogfood/crash.py` `` — *are* the re-runnable
commands. A citation that names a test that was renamed, a file that was removed or a path that never existed
makes an acceptance claim unverifiable while still reading as evidence; the same class produced D-53
(documented claims corrected to the code), D-78/D-86 (public items nothing calls) and D-102 (a config key
nothing reads), all found by hand.

    python3 review/citations.py

It reads the tracked markdown (docs, review, README.md, AGENTS.md) and checks three kinds of backticked
citation against the tree:

  * a qualified name `` `a::b` `` must resolve to a test, to any `fn`/`const`/`static`/`struct`/`enum`/`type`/
    `trait`/`mod` name, or to a module file (`a::b` resolves if `b.rs` exists);
  * a repository-looking path (an extension of `.rs/.py/.sh/.toml/.md/.json/.jsonl` under `docs/`, `engine/`,
    `tui/`, `core/`, `review/`, `verification/` or `examples/`) must exist in the tracked tree;
  * a backticked `.rs` basename must name a file that exists anywhere in the tree (in Rust sources too, where
    this is the only rule applied).

A line that says the cited thing is *gone* (a marker word, or a markdown table whose header says so) counts as
a note rather than a finding: `docs/DECISIONS.md` keeps tables of removed items on purpose, and a citation
there is how the removal is recorded. The tree definition is `git ls-files --cached --others --exclude-standard`,
so a file that is about to be added resolves (the check runs on a working tree, not on a commit) while the
ignored scratch areas stay invisible.

Limits, deliberately stated: bare basenames (`report.json`, `notify.sh`, `credentials.toml`, `INPUTS.md`) are
out of scope — they are runtime artifacts and user-written files, and in a prose scan they drown the signal;
upstream citations (`docs/loops.md` of another project, a URL) are out of scope too, because the tree cannot
resolve them — D-109 is the entry that found one of those by hand; and prose that names a file without any
backticks is not seen at all.
"""
import pathlib
import re
import subprocess
import sys

REPO = pathlib.Path(__file__).resolve().parents[1]
EXCLUDED = ("review/tmp/", "review/eval/")
PATH_PREFIXES = ("docs/", "engine/", "tui/", "core/", "review/", "verification/", "examples/")
PATH_SUFFIXES = (".rs", ".py", ".sh", ".toml", ".md", ".json", ".jsonl")
QUALIFIED = re.compile(r"`([a-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)+)`")
PATH_RE = re.compile(r"`([A-Za-z0-9_./-]+(?:" + "|".join(re.escape(s) for s in PATH_SUFFIXES) + r"))`")
RS_BASENAME = re.compile(r"`([A-Za-z0-9_-]+\.rs)`")
ITEM_RE = re.compile(r"\b(?:fn|const|static|struct|enum|type|trait|mod)\s+([A-Za-z_][A-Za-z0-9_]*)")
TEST_RE = re.compile(r"#\[(?:tokio::)?test\]\s*(?:#\[[^\]]*\]\s*)*fn\s+([A-Za-z_][A-Za-z0-9_]*)")
# a citation that says the thing is gone is a record of a removal, not a broken reference
GONE_MARKERS = ("deleted", "removed", "gone", "dropped", "no longer exists", "obsolete", "pre-v2", "legacy",
                "does not exist", "do not exist", "doesn't exist", "never existed", "nonexistent",
                "no such file", "404")


def tracked():
    out = subprocess.run(["git", "ls-files", "--cached", "--others", "--exclude-standard"],
                         cwd=REPO, capture_output=True, text=True, check=True).stdout
    return [path for path in out.split("\n") if path]


def universe(files):
    """Every name a qualified citation may resolve to, and every file basename."""
    items, tests, basenames = set(), set(), {}
    for name in files:
        basenames.setdefault(pathlib.Path(name).name, name)
        if not name.endswith(".rs"):
            continue
        text = (REPO / name).read_text(errors="replace")
        items |= {match.group(1) for match in ITEM_RE.finditer(text)}
        tests |= {match.group(1) for match in TEST_RE.finditer(text)}
    return items, tests, basenames


def table_header(line_number, lines):
    """The header row of the markdown table the line belongs to, if that block has one.

    Only the *first* row (the header, recognised by the `|---|` separator under it) is returned: a long table
    has no business lending a word from an unrelated row to a citation far below it — `docs/ACCEPTANCE.md`'s
    A-matrix is one table, and a "removed" in row A12 would otherwise excuse every citation under it.
    """
    if line_number > 0 and not lines[line_number - 1].lstrip().startswith("|"):
        return ""
    index = line_number - 1
    while index >= 0 and lines[index].lstrip().startswith("|"):
        index -= 1
    header, separator = lines[index + 1], lines[index + 2] if index + 2 < len(lines) else ""
    return header if separator.lstrip().startswith("|") and set(separator.strip()) <= set("|-: ") else ""


def scan(name, lines, items, tests, basenames, paths):
    checked, findings, notes = 0, [], []
    for number, line in enumerate(lines, start=1):
        context = (line + " " + table_header(number - 1, lines)).lower()
        gone = any(marker in context for marker in GONE_MARKERS)
        for match in QUALIFIED.finditer(line):
            checked += 1
            chain = match.group(1)
            last = chain.split("::")[-1]
            if last in tests or last in items or last + ".rs" in basenames:
                continue
            (notes if gone else findings).append(f"{name}:{number}: `{chain}` names nothing in the tree")
        for match in PATH_RE.finditer(line):
            path = match.group(1)
            if not path.startswith(PATH_PREFIXES) or path in paths:
                continue
            checked += 1
            (notes if gone else findings).append(f"{name}:{number}: `{path}` does not exist")
        for match in RS_BASENAME.finditer(line):
            basename = match.group(1)
            checked += 1
            if basename not in basenames:
                (notes if gone else findings).append(f"{name}:{number}: `{basename}` does not exist")
    return checked, findings, notes


def main():
    files = tracked()
    items, tests, basenames = universe(files)
    paths = set(files)
    checked = 0
    findings, notes = [], []
    for name in files:
        if not name.endswith((".md", ".rs")) or name.startswith(EXCLUDED):
            continue
        text = (REPO / name).read_text(errors="replace")
        seen, found, noted = scan(name, text.split("\n"), items, tests, basenames, paths)
        checked += seen
        findings += found
        notes += noted
    for note in notes:
        print(f"note: {note}")
    for finding in findings:
        print(finding)
    print(f"\n{checked} citations checked against {len(basenames)} files, {len(tests)} tests and "
          f"{len(items)} items: {len(findings)} unexplained, {len(notes)} recorded as removed")
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())
