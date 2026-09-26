#!/usr/bin/env python3
"""CLI flags the parser stores into a field nothing ever reads (the detector behind D-181).

`review/command_params.py` catches the same shape one layer in — a control command's payload field that the
command layer never reads — and D-180 was the flag-level version of it: `teamagents-tui --engine PATH` was
parsed, documented in the knob table and *never read* (`Args::engine_bin` was written twice and read nowhere),
which is D-73's "accepted and silently ignored". No tool saw it: rustc's `dead_code` does not fire for a field
written but never read in those shapes, and `review/dead_code.py` scans public items only.

    python3 review/flag_fields.py

For each `struct Args` in the CLI parsers it takes the field names, then classifies every *other* mention of
each name in the same file: a mention inside the struct's own declaration and its literal initializers
(`Args { field: … }`, located by brace matching) or a direct assignment (`args.field = …`) counts as a write;
anything else — `args.field`, a pattern, a method call on it — counts as a read. A field with **no** read is a
finding. `--surface PATH` replaces the file list, which is how the control is run.

Limits: only files whose `Args` struct is the CLI parser's are read (the two front-ends); a field read through
a `Deref`/`AsRef` impl or a macro would not be seen as a read (the repo's two parsers have none); and the
distinction is textual, so an unusual assignment shape could be misread as a read (quieter, never louder).
"""
import argparse
import pathlib
import re
import sys

REPO = pathlib.Path(__file__).resolve().parents[1]
SURFACES = ("engine/src/main.rs", "tui/src/main.rs")
STRUCT = re.compile(r"^(?:pub )?struct Args \{$", re.M)
# `pub` fields are the engine's style; `impl Args {` is not a literal, and reading it as one hid every read
# that lives in the impl (the first run missed `self.state_root` and reported the flag as dead)
FIELD = re.compile(r"^\s+(?:pub )?([a-z_][a-z0-9_]*):\s", re.M)
LITERAL = re.compile(r"(?<!impl )\bArgs \{")


def blank_literals(text: str) -> str:
    """`text` with string, char and comment bodies replaced by spaces, so braces can be counted for real.

    The first version balanced braces on the raw text and swallowed nine thousand characters of `main.rs`
    (the reads included) because a `{` inside a string counted as structure. Offsets are preserved.
    """
    out = list(text)
    index, length = 0, len(text)
    while index < length:
        two = text[index:index + 2]
        if two in ("//", "/*"):
            end = text.find("\n" if two == "//" else "*/", index)
            end = length if end < 0 else end + (0 if two == "//" else 2)
            for position in range(index, end):
                if out[position] != "\n":
                    out[position] = " "
            index = end
            continue
        if text[index] in ('"', "'"):
            quote, index = text[index], index + 1
            while index < length:
                if text[index] == "\\":
                    out[index] = " "
                    index += 1
                elif text[index] == quote:
                    break
                out[index] = " "
                index += 1
        index += 1
    return "".join(out)


def fields(text: str):
    """The flag fields of the parser's `struct Args`, in declaration order (empty when there is none)."""
    match = STRUCT.search(text)
    if match is None:
        return []
    return FIELD.findall(text[match.end():text.index("\n}\n", match.end())])


def read_positions(text: str, field: str) -> int:
    """How many mentions of `field` are reads.

    Strings and comments are blanked first (a flag named in a message is not a read), then each mention is
    classified by its context: `field:` with no `.` in front is a declaration or a struct-literal key;
    `.field = …` is an assignment; everything else — `args.field`, `self.field.method()`, a bare name in a
    pattern — is a read.
    """
    scan = blank_literals(text)
    reads = 0
    for match in re.finditer(rf"\b{re.escape(field)}\b", scan):
        before = scan[match.start() - 1:match.start()]
        after = scan[match.end():]
        if after[:1] == ":" and before != ".":
            continue
        if before == "." and re.match(r"\s*=[^=]", after):
            continue
        reads += 1
    return reads


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--surface", action="append", help="a file to read instead of the default parsers")
    args = parser.parse_args(argv)
    read = args.surface or list(SURFACES)
    findings, checked = [], 0
    for name in read:
        path = REPO / name
        if not path.is_file():
            findings.append(f"{name}: no such surface to read")
            continue
        text = path.read_text(errors="replace")
        declared = fields(text)
        if not declared:
            findings.append(f"{name}: no `struct Args` to read; if the parser moved, update this audit")
            continue
        for field in declared:
            checked += 1
            if read_positions(text, field) == 0:
                findings.append(
                    f"{name}: `--{field.replace('_', '-')}` is parsed into `Args::{field}`, which nothing ever "
                    "reads — a flag the product cannot honour (D-73); refuse it by name or use it (D-180/D-181)"
                )
    for finding in findings:
        print(f"FAIL: {finding}")
    if findings:
        return 1
    print(f"{checked} parsed flag field(s) across {len(read)} parser(s): every one is read somewhere")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
