#!/usr/bin/env python3
"""Validate current source, file, link, Make-target and section references.

The scan covers tracked Markdown and Rust sources. Relative links resolve from the
containing document; qualified Rust/TLA names and executable references must exist.
Counts are computed for this run, not copied into prose as historical snapshots.
Regex scans cannot establish semantic correctness or exhaustive reference coverage.
"""

import pathlib
import re
import subprocess
import sys
import urllib.parse

REPO = pathlib.Path(__file__).resolve().parents[1]

sys.path.insert(0, str(REPO / "tools"))   # one Makefile parser, shared with the audit that owns D-196
from build_references import phony_targets  # noqa: E402

EXCLUDED = ("tools/tmp/",)
PATH_PREFIXES = ("docs/", "engine/", "tui/", "core/", "tools/", "verification/", "examples/")
PATH_SUFFIXES = (".rs", ".py", ".sh", ".toml", ".md", ".json", ".jsonl", ".tla", ".cfg")
# an inline markdown link: [label](target), optionally with a title, and never inside a code span or fence
LINK = re.compile(r"\[([^\]]*)\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")
FENCE = re.compile(r"^\s*(```|~~~)")
CODE_SPAN = re.compile(r"`[^`]*`")
# a `make <target>` a reader can paste: the command word, then a target name, then anything (flags, variables)
MAKE_CMD = re.compile(r"^make\s+([A-Za-z][A-Za-z0-9-]*)")
QUALIFIED = re.compile(r"`([a-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)+)`")
# a model citation: a module name that has a `verification/tla/<name>.tla`, and a member of it
TLA_CITE = re.compile(r"`(V2[A-Za-z0-9_]*)::([A-Za-z_][A-Za-z0-9_]*)`")
PATH_RE = re.compile(r"`([A-Za-z0-9_./-]+(?:" + "|".join(re.escape(s) for s in PATH_SUFFIXES) + r"))`")
RS_BASENAME = re.compile(r"`([A-Za-z0-9_-]+\.(?:rs|tla|cfg))`")
ITEM_RE = re.compile(r"\b(?:fn|const|static|struct|enum|type|trait|mod)\s+([A-Za-z_][A-Za-z0-9_]*)")
TEST_RE = re.compile(r"#\[(?:tokio::)?test\]\s*(?:#\[[^\]]*\]\s*)*fn\s+([A-Za-z_][A-Za-z0-9_]*)")
# the numbered documents a `§N.M` citation can point at, and the headings that define those numbers (D-223)
SECTION = re.compile(r"§(\d+(?:\.\d+)*)")
SECTION_DOCS = ("docs/DESIGN.md", "docs/USER-GUIDE.md", "docs/INSTALL.md", "verification/REPORT.md")
HEADING = re.compile(r"^#{2,4}\s+(\d+(?:\.\d+)*)\.?\s", re.M)


def tracked():
    out = subprocess.run(["git", "ls-files", "--cached", "--others", "--exclude-standard"],
                         cwd=REPO, capture_output=True, text=True, check=True).stdout
    # A deleted-but-not-yet-staged path is absent from the working tree, not readable evidence.
    return [path for path in out.split("\n") if path and (REPO / path).is_file()]


def tla_members() -> dict:
    """`{module: its text}` for every `verification/tla/V2*.tla`: what a `V2Module::member` citation resolves in."""
    out = {}
    for path in (REPO / "verification" / "tla").glob("V2*.tla"):
        out[path.stem] = path.read_text(errors="replace")
    return out


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


def sections() -> set:
    """Every section number the numbered documents of this repository define."""
    out = set()
    for doc in SECTION_DOCS:
        out |= set(HEADING.findall((REPO / doc).read_text(errors="replace")))
    return out


def scan(name, lines, items, tests, basenames, paths, modules, numbers):
    checked, findings, section_refs = 0, [], 0
    for number, line in enumerate(lines, start=1):
        for match in SECTION.finditer(line):
            section_refs += 1
            if match.group(1) in numbers:
                continue
            checked += 1
            findings.append(
                f"{name}:{number}: §{match.group(1)} is not a section of any numbered document here "
                f"({', '.join(SECTION_DOCS)})")
        for match in QUALIFIED.finditer(line):
            checked += 1
            chain = match.group(1)
            last = chain.split("::")[-1]
            if last in tests or last in items or last + ".rs" in basenames:
                continue
            findings.append(f"{name}:{number}: `{chain}` names nothing in the tree")
        for match in TLA_CITE.finditer(line):
            module, member = match.groups()
            if module not in modules:
                if module in items and member in items:
                    continue  # A known Rust type and member, rather than a TLA module.
                checked += 1
                findings.append(f"{name}:{number}: `{module}::{member}` names no known module or Rust item")
                continue
            checked += 1
            # a definition, with or without parameters (`FailRequest(i) ==` is an action)
            if not re.search(rf"^{re.escape(member)}\s*(?:\([^)]*\))?\s*==", modules[module], re.M):
                findings.append(
                    f"{name}:{number}: `{module}::{member}` is not defined in verification/tla/{module}.tla")
        for match in PATH_RE.finditer(line):
            path = match.group(1)
            if not path.startswith(PATH_PREFIXES) or path in paths:
                continue
            checked += 1
            findings.append(f"{name}:{number}: `{path}` does not exist")
        for match in RS_BASENAME.finditer(line):
            basename = match.group(1)
            checked += 1
            if basename not in basenames:
                findings.append(f"{name}:{number}: `{basename}` does not exist")
    return checked, findings, section_refs


def prose_without_code(text: str) -> list[tuple[int, str]]:
    """`(line number, text)` for a markdown file with fenced blocks and inline code spans blanked.

    A formula inside a code span can look like a link (`[](Entitled(i, "shell"))` in `V2Authority`'s prose) and a
    table shown in a fence is an example, not this document's; both are dropped before the links are read.
    """
    out, fenced = [], False
    for number, line in enumerate(text.split("\n"), 1):
        if FENCE.match(line):
            fenced = not fenced
            continue
        if fenced:
            continue
        out.append((number, CODE_SPAN.sub("`…`", line)))
    return out


def broken_links(name: str, text: str) -> tuple[int, list]:
    """`(relative links read, findings)` for `name`: a target that does not exist against its own directory."""
    out, read = [], 0
    base = pathlib.Path(name).parent
    for number, line in prose_without_code(text):
        for label, target in LINK.findall(line):
            if target.startswith(("http://", "https://", "mailto:", "#")):
                continue
            path = urllib.parse.unquote(target.split("#", 1)[0])
            if not path:
                continue
            read += 1
            if not (base / path).exists():
                out.append(f"{name}:{number}: `[{label[:40]}]({target})` names nothing in the tree")
    return read, out


def make_citations(name: str, lines: list, declared: set) -> tuple[int, list, set]:
    """Check inline or fenced Make commands against declared phony targets."""
    read, findings, targets = 0, [], set()
    fenced = False
    for number, line in enumerate(lines, start=1):
        if FENCE.match(line):
            fenced = not fenced
            continue
        commands = []
        if fenced:
            source = line.strip()
            if source.startswith("$ "):
                source = source[2:].strip()
            match = MAKE_CMD.match(source)
            if match:
                commands.append(match.group(1))
        else:
            for span in CODE_SPAN.finditer(line):
                match = MAKE_CMD.match(span.group(0).strip("`").strip())
                if match:
                    commands.append(match.group(1))
        if not commands:
            continue
        for target in commands:
            read += 1
            targets.add(target)
            if target in declared:
                continue
            message = f"{name}:{number}: `make {target}` names no `.PHONY` target"
            findings.append(message)
    return read, findings, targets


def self_check():
    def check(text, items=None):
        return scan("fixture.md", text.splitlines(), items or set(), set(), {}, set(),
                    {"V2Current": "Spec == TRUE"}, {"1"})[1]

    assert not check("Current `V2Current::Spec` and §1.")
    assert not check("`V2App::new`", {"V2App", "new"})
    for text in ["`V2Missing::Spec`", "`V2Current::Missing`", "`V2App::missing`",
                 "Legacy `tools/missing.py` is removed.", "Archived §99.",
                 "| Removed |\n|---|\n| `missing.rs` |", "Gone `crate::missing`."]:
        assert check(text), text
    assert not make_citations("fixture.md", ["Run `make check`."], {"check"})[1]
    for lines in [["Legacy `make missing` is obsolete."], ["```bash", "$ make missing", "```"],
                  ["The `.PHONY` list removed `make missing`."]]:
        assert make_citations("fixture.md", lines, {"check"})[1], lines


def main():
    self_check()
    files = tracked()
    items, tests, basenames = universe(files)
    paths = set(files)
    declared = set(phony_targets((REPO / "Makefile").read_text(errors="replace")))
    modules = tla_members()
    numbers = sections()
    cited_targets = set()
    checked, links, made, refs = 0, 0, 0, 0
    findings = []
    for name in files:
        if not name.endswith((".md", ".rs")) or name.startswith(EXCLUDED):
            continue
        text = (REPO / name).read_text(errors="replace")
        seen, found, seen_refs = scan(name, text.split("\n"), items, tests, basenames, paths, modules, numbers)
        checked += seen
        refs += seen_refs
        findings += found
        if name.endswith(".md"):
            read, broken = broken_links(name, text)
            links += read
            findings += broken
            commands, missing, cited = make_citations(name, text.split("\n"), declared)
            made += commands
            cited_targets |= cited
            findings += missing
    for finding in findings:
        print(finding)
    print(f"\n{checked} citations checked against {len(basenames)} files, {len(tests)} tests and "
          f"{len(items)} items, plus {links} relative link(s), {made} `make` command(s) over "
          f"{len(cited_targets)} distinct target(s) and {refs} `§`-section reference(s): {len(findings)} "
          f"unexplained")
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())
