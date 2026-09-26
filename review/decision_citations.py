#!/usr/bin/env python3
"""`D-<n>` citations that resolve to nothing in the current decision log (the detector behind D-170).

`docs/DECISIONS.md` is the index of what is binding, so a document that cites `D-61` is pointing a reader at a
rule and at the entry that records why it holds. Nothing checked those pointers: `citations.py` resolves code
names and paths, `decisions_log.py` checks the log's own shape, and a citation to a decision that is not in the
log — because it belonged to an earlier implementation, or because it was renumbered, or because the number was
simply mis-remembered — reads exactly like a citation that works.

    python3 review/decision_citations.py [--list] [--only PATH]

Two indexes define what a citation may point at:

* every `## D-<n> <title>` heading in `docs/DECISIONS.md` — the live decisions;
* the origin column of that file's **"Earlier rules that still apply"** table — the earlier (v1) decisions that
  still bind and are deliberately *not* re-stated as entries. Those two sets are the whole index; a reader who
  looks up a number in the log finds either its entry or its row.

Every `D-<n>` in the tracked markdown must resolve to one of them, unless its own *paragraph* marks it as
history (a marker word: v1, earlier, removed, archive, history, gone, no longer) — that is how a documented
removal, like "restoring the v1 D-32 contract", stays legal without pretending the entry still exists.

`--list` prints the index; `--only PATH` checks one file instead of the tracked tree, which is how the control
is run (a synthetic file with a bogus number must produce exactly one finding).

Limits, deliberately stated: the earlier-rules index is read as "every `D-<n>` inside that table", so a number
mentioned in a rule's *text* counts as indexed (quieter, never wrong about the numbers that do resolve); the
marker is looked for in the whole blank-line-separated paragraph that carries the citation, so a long paragraph
that says "earlier" somewhere is forgiven (quieter again); the scan is a plain regex over markdown, so a number
split across a line break is invisible; and the frozen
evaluation material (`review/eval/r2-p6/tasks/**`, `runs/**`) is out of scope, because those bytes are pinned
by their manifests and must not be edited to satisfy a gate.
"""
import pathlib
import re
import subprocess
import sys

REPO = pathlib.Path(__file__).resolve().parents[1]
LOG = REPO / "docs" / "DECISIONS.md"
INDEX_HEADING = "## Earlier rules that still apply"
EXCLUDED = ("review/eval/r2-p6/tasks/", "review/eval/r2-p6/runs/", "review/tmp/")
HEADING = re.compile(r"^## D-(\d+) ", re.M)
NUMBER = re.compile(r"\bD-(\d+)\b")
# a citation whose own line says the decision is history is a record, not a broken pointer
HISTORY_MARKERS = ("v1", "earlier", "removed", "archive", "history", "gone", "no longer", "pre-v2")


def index():
    """(headings, earlier) — the numbers the log itself defines."""
    text = LOG.read_text(errors="replace")
    headings = {int(m.group(1)) for m in HEADING.finditer(text)}
    start = text.find(INDEX_HEADING)
    earlier = set()
    if start >= 0:
        end = text.find("\n## ", start + len(INDEX_HEADING))
        section = text[start:end if end > 0 else len(text)]
        earlier = {int(m.group(1)) for m in NUMBER.finditer(section)}
    return headings, earlier


def tracked():
    out = subprocess.run(["git", "ls-files", "--cached", "--others", "--exclude-standard"],
                         cwd=REPO, capture_output=True, text=True, check=True).stdout
    return [path for path in out.split("\n") if path.endswith(".md") and not path.startswith(EXCLUDED)]


def findings(paths, headings, earlier):
    """[(path, line, number, text)] for every citation that resolves to nothing."""
    known = headings | earlier
    out = []
    for path in paths:
        file = REPO / path
        if not file.is_file():
            continue
        for number, line, text, paragraph in scan(file.read_text(errors="replace")):
            if number in known:
                continue
            if any(marker in paragraph.lower() for marker in HISTORY_MARKERS):
                continue
            out.append((path, number, line, text))
    return out


def scan(text):
    """[(number, line, stripped line, paragraph)] for every `D-<n>` in `text`.

    The paragraph is the blank-line-separated block the line belongs to: a citation's history marker is usually
    in the sentence that introduces it, not on the line the number happens to land on.
    """
    lines = text.split("\n")
    blocks = []          # (first 1-based line, last 1-based line, text) per paragraph
    start = None
    for index, raw in enumerate(lines + [""], 1):
        blank = not raw.strip()
        if blank and start is not None:
            blocks.append((start, index - 1, "\n".join(lines[start - 1:index - 1])))
            start = None
        elif not blank and start is None:
            start = index
    paragraph_of = {}
    for first, last, body in blocks:
        for number in range(first, last + 1):
            paragraph_of[number] = body
    for line, raw in enumerate(lines, 1):
        for match in NUMBER.finditer(raw):
            yield int(match.group(1)), line, raw.strip(), paragraph_of.get(line, raw)


def main(argv):
    only = [argv[i + 1] for i, arg in enumerate(argv) if arg == "--only" and i + 1 < len(argv)]
    headings, earlier = index()
    if "--list" in argv:
        print(f"live decisions ({len(headings)}): {', '.join(f'D-{n}' for n in sorted(headings))}")
        print(f"earlier rules still in force ({len(earlier)}): {', '.join(f'D-{n}' for n in sorted(earlier))}")
        return 0
    paths = only or tracked()
    found = findings(paths, headings, earlier)
    for path, number, line, text in found:
        print(f"{path}:{line}: D-{number} is in no heading and no \"Earlier rules\" row of docs/DECISIONS.md")
        print(f"    {text[:120]}")
    checked = sum(1 for path in paths if (REPO / path).is_file())
    print(
        f"{checked} markdown file(s), {len(headings)} live decisions and {len(earlier)} earlier rules indexed: "
        f"{len(found)} unexplained citation(s)"
    )
    return 1 if found else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
