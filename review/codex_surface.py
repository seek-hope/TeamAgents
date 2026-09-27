#!/usr/bin/env python3
"""The Codex column of the comparison, against the installed binary (D-209).

`docs/PRODUCT-COMPARISON.md`'s §1 says what Codex CLI offers — `resume`/`fork`/`archive`/`delete`/
`migrate-rollouts`, `mcp add/remove/login`, `apply`, `review`, `cloud`, `doctor`, `debug`, … — and its strength
table says the row was "re-checked locally: every claimed verb appears in that help output". That was a hand
check on a date, and nothing re-derived it. This script does: it reads the backticked tokens of that column and
requires each one to appear in the help output the strength table names.

    python3 review/codex_surface.py
    python3 review/codex_surface.py --doc /tmp/copy.md      # the control, on a copy

Only *commands and flags* are checked — a token is kept when it is `verb`, `verb-verb`, `--flag` or `-f` after
splitting on `/` and whitespace and dropping a leading `codex`; paths (`~/.codex/config.toml`) and assignments
(`key=value`) are proverbs about where things live, not surfaces, and are skipped. The version the strength table
names is checked too, because it is what makes the rest of the row a statement about a *specific* binary: an
upgraded Codex CLI is a finding until the row is re-dated.

It is not part of `make check`: it needs a `codex` binary, which CI does not have. It is listed as run by hand
in the hygiene catalogue for that reason, and the strength table carries the date it was last run.
"""
import argparse
import pathlib
import re
import shutil
import subprocess
import sys

REPO = pathlib.Path(__file__).resolve().parents[1]
DOC = REPO / "docs/PRODUCT-COMPARISON.md"
# the help surfaces the strength table names; the row's claims live in them
HELPS = [(), ("exec",), ("mcp",), ("sandbox",), ("debug",), ("app-server",)]
VERSION = re.compile(r"`codex-cli ([0-9][0-9.]*)`")
TOKEN = re.compile(r"^[a-z][a-z-]*$")
FLAG = re.compile(r"^--?[a-z][a-z-]*$")


def codex_column(doc: str) -> list:
    """The backticked tokens the §1 table attributes to Codex, in row order."""
    section = doc.split("## 1. By dimension")[1].split("## 2.")[0]
    tokens = []
    for line in section.split("\n"):
        if not line.startswith("|") or line.startswith("|---") or "Dimension" in line:
            continue
        cells = [cell.strip() for cell in line.strip("|").split("|")]
        if len(cells) < 2:
            continue
        for chunk in re.findall(r"`([^`]+)`", cells[1]):
            for part in re.split(r"[/\s]+", chunk.strip()):
                part = part.strip(".,;:()")
                if part.startswith("codex"):
                    continue
                if "=" in part or "~" in part or "/" in part:
                    continue
                if TOKEN.match(part) or FLAG.match(part):
                    tokens.append(part)
    return tokens


def main(argv) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--doc", default=str(DOC), help="the comparison to read (a copy is the control)")
    parser.add_argument("--codex", default="codex", help="the binary to ask")
    args = parser.parse_args(argv)
    if not shutil.which(args.codex):
        print(f"note: no `{args.codex}` on PATH, so the comparison's Codex column cannot be re-derived here "
              "(the strength table carries the date it last was)")
        return 0
    doc = pathlib.Path(args.doc).read_text()
    helps = []
    for extra in HELPS:
        done = subprocess.run([args.codex, *extra, "--help"], capture_output=True, text=True)
        helps.append(done.stdout + done.stderr)
    surface = "\n".join(helps)
    version = subprocess.run([args.codex, "--version"], capture_output=True, text=True)
    # the binary may warn on stderr (a read-only PATH, say): the version is the line that names itself
    reported = next((line.strip() for line in version.stdout.splitlines() if "codex-cli" in line), "").strip()
    findings, checked = [], 0
    named = VERSION.search(doc)
    if named is None:
        findings.append(f"{args.doc} no longer names the Codex version its row was checked against, so the row "
                        "cannot be dated")
    elif named.group(1) not in reported:
        findings.append(f"{args.doc} names `codex-cli {named.group(1)}`, the binary here reports "
                        f"{reported!r}: re-check the row against this binary and re-date it")
    seen = []
    for token in codex_column(doc):
        checked += 1
        if token in seen:
            continue
        seen.append(token)
        if not re.search(r"(?<![\w-])" + re.escape(token) + r"(?![\w-])", surface):
            findings.append(f"{args.doc}'s Codex column claims `{token}`, which appears in none of the "
                            f"{len(HELPS)} help outputs this row names ({reported})")
    for finding in findings:
        print("FAIL:", finding)
    if findings:
        return 1
    print(f"{checked} Codex token(s) across the comparison's §1 rows appear in {len(HELPS)} help outputs; "
          f"the binary reports {reported}, the version the row names")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
