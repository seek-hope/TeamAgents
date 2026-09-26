#!/usr/bin/env python3
"""The environment knobs the code reads, and the table that documents them (D-157's sibling).

`docs/DEVELOPMENT.md` lists every `TEAMAGENTS_*` variable the product reads — the diagnostic that turns an
offer into a witness, the deployment knobs that locate a binary, and the test-only ones. Nothing held the list
to the code: a knob added without a row is one nobody can find (the witness D-143 needed was *added* to the
code a day before it appeared anywhere a user would look), and a row naming a variable the code stopped reading
is a promise to a reader who will set it and see nothing happen.

    python3 review/env_knobs.py

A *knob* is a whole string literal that names one (`"TEAMAGENTS_LOG_SURFACE"`). A literal that merely contains
the prefix is not: the `exec --check` wrapper writes a marker `__TEAMAGENTS_CHECK_RC_<uuid>__` into a command's
output, which is not read from the environment at all — the first version of the table said it was, which is
why this audit compares whole literals.

`TEAMAGENTS_BIN_DIR` is read by `install.sh` rather than by Rust, and is documented for users in
`docs/INSTALL.md`; the table row points there, and the audit accepts a knob named in either document.
"""
import pathlib
import re
import sys

REPO = pathlib.Path(__file__).resolve().parents[1]
SOURCES = ("core/src", "engine/src", "tui/src")
READERS = (REPO / "install.sh",)
TABLES = (REPO / "docs/DEVELOPMENT.md", REPO / "docs/INSTALL.md")
LITERAL = re.compile(r'"([A-Z][A-Z0-9_]*)"')
KNOB = re.compile(r"^TEAMAGENTS_[A-Z][A-Z0-9_]*$")


def read_by_code() -> dict:
    """`knob -> [file:line]` for every whole literal that names a `TEAMAGENTS_*` variable."""
    found: dict[str, list[str]] = {}
    rust = [path for source in SOURCES for path in (REPO / source).rglob("*.rs")]
    for path in rust:
        name = str(path.relative_to(REPO))
        for number, line in enumerate(path.read_text(errors="replace").splitlines(), 1):
            for literal in LITERAL.findall(line):
                if KNOB.match(literal):
                    found.setdefault(literal, []).append(f"{name}:{number}")
    # a shell reader writes the variable, it does not quote it as a Rust string literal (`${NAME:-…}`)
    for path in READERS:
        if not path.is_file():
            continue
        for number, line in enumerate(path.read_text(errors="replace").splitlines(), 1):
            for bare in re.findall(r"\b(TEAMAGENTS_[A-Z][A-Z0-9_]*)\b", line):
                found.setdefault(bare, []).append(f"{path.name}:{number}")
    return found


def documented() -> set:
    """The knobs the tables name, in backticks or as bare words."""
    names = set()
    for path in TABLES:
        for literal in re.findall(r"`(TEAMAGENTS_[A-Z][A-Z0-9_]*)", path.read_text(errors="replace")):
            names.add(literal)
    return names


def main() -> int:
    code, table = read_by_code(), documented()
    findings = []
    for knob in sorted(set(code) - table):
        findings.append(f"the code reads {knob} ({code[knob][0]}) and no document names it")
    for knob in sorted(table - set(code)):
        findings.append(f"a document names {knob} and no code reads it")
    for finding in findings:
        print(finding)
    print(f"{len(code)} knob(s) read by the code, {len(table)} named by the documents: {len(findings)} "
          f"unexplained")
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())
