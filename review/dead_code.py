#!/usr/bin/env python3
"""Public items nothing calls (the detector behind D-78 and D-86).

`clippy` cannot help here: `dead_code` only fires for private items, so a `pub fn`/`pub struct` that no
caller anywhere in the tree mentions is silent — and this repository has twice found real leftovers that
way (removed writers, a worktree helper reading a layout that no longer exists). This script is that audit,
mechanically and re-runnably:

    python3 review/dead_code.py

It scans the three crates' `src`, their integration tests, `engine/examples` and `engine/benches` for public
item definitions (`fn`, `struct`, `enum`, `const`, `static`, `type`, `trait`), counts every other mention of
each name in Rust files and in the scripts/Makefile that drive the CLI, and reports the names whose only
occurrences are their own definition lines. Prose in `docs/**` and `review/**/*.md` is deliberately *not* a
use: a name that only appears in a document is documented, not called. Two rules of thumb about the limits:
a name mentioned in a Rust doc comment or in a test counts as a use, and a dynamically dispatched call is
still a call by name, so the report is a starting point, not a verdict.

Items the repository knows about are listed in `KNOWN_UNCALLED` with the decision that keeps them, so a new
finding stands out and the exit code stays meaningful.
"""
import argparse
import collections
import pathlib
import re
import sys

REPO = pathlib.Path(__file__).resolve().parents[1]
CODE_ROOTS = ["core/src", "engine/src", "tui/src", "core/tests", "engine/tests", "tui/tests",
              "engine/examples", "engine/benches"]
USER_FILES = ["Makefile", "install.sh", "tui/scripts/pty_v2_smoke.py", "tui/scripts/pty_screen.py",
              "review/install_check.py", "review/dogfood/input_latency.py", "review/dogfood/tui.py"]

# name -> why it is allowed to have no caller
KNOWN_UNCALLED = {
    "member_worktrees": "D-76: the retirement path inspects one recorded workspace; a scan of every member "
                        "worktree belongs to the merge surface that is an open item",
    "wait_idle": "D-63: the parked substrate for interrupt-and-redirect; documented with a ponytail note",
    # DESIGN §4.1 names these row shapes as the minimal data contract; the code reads them through SQL
    # (store.rs owns the schema), so the typed form documents the contract instead of being called.
    "Envelope": "DESIGN §4.1 minimal data contract (Envelope / ContextEntry), read through SQL",
    "ModelRequestRecord": "DESIGN §4.1 minimal data contract (ModelRequest / Attempt), read through SQL",
    "Attempt": "DESIGN §4.1 minimal data contract (ModelRequest / Attempt), read through SQL",
    "Operation": "DESIGN §4.1 minimal data contract (the operation row, §6.1), read through SQL",
}

DEFINITION = re.compile(
    r"^\s*pub(?:\(crate\))? (?:async )?(?:unsafe )?(?:extern \"[^\"]+\" )?(fn|struct|enum|const|static|type|trait) (\w+)")


def rust_files() -> list[pathlib.Path]:
    return [p for root in CODE_ROOTS for p in pathlib.Path(REPO / root).rglob("*.rs")]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--list-known", action="store_true", help="also list the allowed no-caller items")
    args = parser.parse_args()
    files = rust_files()
    definitions: dict[str, list[str]] = collections.defaultdict(list)
    for path in files:
        for number, line in enumerate(path.read_text().splitlines(), 1):
            match = DEFINITION.match(line)
            if match:
                definitions[match.group(2)].append(f"{path.relative_to(REPO)}:{number}")
    code = "\n".join(path.read_text() for path in files)
    drivers = "\n".join((REPO / name).read_text(errors="ignore")
                        for name in USER_FILES if (REPO / name).is_file())
    findings, allowed = [], []
    for name, where in sorted(definitions.items()):
        uses = len(re.findall(r"\b" + re.escape(name) + r"\b", code)) - len(where)
        if uses > 0 or re.search(r"\b" + re.escape(name) + r"\b", drivers):
            continue
        (allowed if name in KNOWN_UNCALLED else findings).append((name, where))
    print(f"{len(definitions)} public items scanned; {len(findings)} uncalled, "
          f"{len(allowed)} known and allowed")
    for name, where in findings:
        print(f"  uncalled: {name} at {where[0]}")
    if args.list_known:
        for name, where in allowed:
            print(f"  allowed: {name} at {where[0]} ({KNOWN_UNCALLED[name]})")
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())
