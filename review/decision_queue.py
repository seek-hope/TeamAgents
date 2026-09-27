#!/usr/bin/env python3
"""Decisions that ask for the user's word are in the queue, or say what closed them (the detector behind D-195).

The campaign ends every turn with a report whose last section is the user's decision queue, and the queue's
authoritative copies are `docs/ACCEPTANCE.md`'s "Known gaps" section and `docs/PRODUCT-COMPARISON.md` §2 ("what
this comparison suggests, in decision order"). Both are prose, and D-193/D-194 showed what happens to a list
nobody reads: the three newest entries — D-192 (retention's rule is now modeled, which was D-75's stated reason
for deferring it), D-191 (collection runs at a boot and nowhere else) and D-174 (whose deferred half D-191 then
implemented) — were all missing from the queue, so a reader of the queue did not know the analysis had moved.

    python3 review/decision_queue.py
    python3 review/decision_queue.py --decisions PATH --queue PATH   # the control, on copies

The rule: an entry that says "needs the user's word" or names it as "the user's call" must be (a) cited by one
of the two queue documents, or (b) have been closed since, which the entry states by naming a *later* decision
(`D-174`'s "closed by D-191"). The phrase is the contract, so an entry that asks for a decision without using it
is invisible here — the ceiling this audit states rather than hides.
"""
import argparse
import pathlib
import re
import sys

REPO = pathlib.Path(__file__).resolve().parents[1]
DECISIONS = "docs/DECISIONS.md"
QUEUES = ["docs/ACCEPTANCE.md", "docs/PRODUCT-COMPARISON.md"]
# The phrases that ask for a decision. "the user's words" (D-96 quotes an expectation) is deliberately not one.
ASK = re.compile(r"needs the user's word|needs the user's call|is the user's call|the user's call\b|the user's word\b", re.I)
ENTRY = re.compile(r"^## (D-(\d+)) ", re.M)
KNOWN_GAPS = re.compile(r"## Known gaps(.*?)(?=\n## |\Z)", re.S)
COMPARISON_2 = re.compile(r"## 2\.[^\n]*\n(.*?)(?=\n## |\Z)", re.S)


def entries(text: str) -> list[tuple[str, int, str]]:
    """`(name, number, body)` for every decision entry, in file order."""
    out = []
    marks = list(ENTRY.finditer(text))
    for index, mark in enumerate(marks):
        end = marks[index + 1].start() if index + 1 < len(marks) else len(text)
        out.append((mark.group(1), int(mark.group(2)), text[mark.start():end]))
    return out


def queue_text() -> str:
    """The two queue documents' decision sections, concatenated."""
    acceptance = (REPO / QUEUES[0]).read_text()
    comparison = (REPO / QUEUES[1]).read_text()
    parts = []
    gaps = KNOWN_GAPS.search(acceptance)
    if gaps:
        parts.append(gaps.group(1))
    second = COMPARISON_2.search(comparison)
    if second:
        parts.append(second.group(1))
    return "\n".join(parts)


def main(argv) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--decisions", default=DECISIONS)
    parser.add_argument("--queue", default=None, help="read this file instead of the two queue documents")
    args = parser.parse_args(argv)
    text = (REPO / args.decisions).read_text()
    queue = pathlib.Path(args.queue).read_text() if args.queue else queue_text()
    findings, asked = [], 0
    for name, number, body in entries(text):
        if not ASK.search(body):
            continue
        asked += 1
        from_queue = re.search(rf"\b{name}\b", queue) is not None
        # a later decision named in the entry is how a closed item says so (D-174's "closed by D-191")
        closed_by = any(int(later) > number for later in re.findall(r"\bD-(\d+)\b", body))
        if from_queue or closed_by:
            continue
        findings.append(
            f"{name} asks for the user's decision and appears in neither queue document, and it names no later "
            "decision as the one that closed it: add it to docs/ACCEPTANCE.md's known gaps or to "
            "docs/PRODUCT-COMPARISON.md §2 (D-195)"
        )
    for finding in findings:
        print(f"FAIL: {finding}")
    if findings:
        return 1
    print(f"{asked} decision(s) ask for the user's word; every one is in the queue or names what closed it")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
